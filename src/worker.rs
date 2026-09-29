use crate::store::PendingDirectory;
use crate::{model::*, platform};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[cfg(unix)]
struct ProcessGroup(u32);
#[cfg(unix)]
impl ProcessGroup {
    fn terminate(&self) {
        // Each worker leads a new process group. Kill its CUPS/Poppler/SANE/Office
        // descendants too, including when the supervisor future is dropped.
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}
#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct Request {
    #[serde(default)]
    pub license: String,
    pub operation: String,
    pub path: String,
    pub output: String,
    pub page: u32,
    pub print: Option<PrintOptions>,
    pub scan: Option<ScanOptions>,
    pub show_virtual: bool,
    pub demo: bool,
    #[serde(default)]
    pub profile: Option<crate::profiles::DriverProfile>,
}
#[derive(Default, Serialize, Deserialize)]
pub struct Response {
    pub error: Option<String>,
    pub devices: Option<Devices>,
    pub pages: u32,
    pub profile: Option<crate::profiles::DriverProfile>,
}

pub fn entry(args: &[String]) -> i32 {
    if args.len() != 2 {
        return 2;
    }
    let result = (|| -> AppResult<Response> {
        let req: Request =
            serde_json::from_slice(&std::fs::read(&args[0]).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        crate::licensing::verify(&req.license, &crate::licensing::current_machine()?)?;
        if req.operation == "office" {
            platform::convert_office(&req.path, &req.output).map_err(|e| format!("Office/WPS 转换失败：{e}。请确认文档未损坏、未加密，且主机办公软件已完成初始化。"))?;
            return Ok(Response::default());
        }
        if req.operation == "profile" {
            return Ok(Response {
                profile: Some(platform::capture_driver_profile(&req.path)?),
                ..Default::default()
            });
        }
        let _apartment = platform::initialize()?;
        match req.operation.as_str() {
            "devices" => Ok(Response {
                devices: Some(if req.demo {
                    demo_devices()
                } else {
                    platform::devices(req.show_virtual)?
                }),
                ..Default::default()
            }),
            "inspect" => Ok(Response {
                pages: platform::document_pages(&req.path)?,
                ..Default::default()
            }),
            "preview" => {
                platform::preview(&req.path, req.page, &req.output)?;
                Ok(Response::default())
            }
            "print" => {
                let opts = req.print.ok_or("缺少打印参数")?;
                if req.demo {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                } else {
                    platform::print(&req.path, &opts, req.profile.as_ref())?;
                }
                Ok(Response::default())
            }
            "scan" => {
                let opts = req.scan.ok_or("缺少扫描参数")?;
                if req.demo {
                    demo_scan(&req.output, &opts)?;
                } else {
                    platform::scan(&req.output, &opts)?;
                }
                Ok(Response {
                    pages: 1,
                    ..Default::default()
                })
            }
            _ => Err("未知操作".into()),
        }
    })();
    let response = result.unwrap_or_else(|e| Response {
        error: Some(e),
        ..Default::default()
    });
    let ok = response.error.is_none();
    if std::fs::write(&args[1], serde_json::to_vec(&response).unwrap()).is_err() {
        return 3;
    }
    if ok { 0 } else { 1 }
}

pub async fn call(root: &Path, req: Request, seconds: u64) -> AppResult<Response> {
    let destination = PathBuf::from(&req.output);
    // A dropped HTTP request signals this supervisor; it still reaps the child
    // before removing its scratch directory (important for Windows file locks).
    let (cancel, cancelled) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(run_worker(root.to_path_buf(), req, seconds, cancelled));
    let (response, _work, output) = task.await.map_err(|e| e.to_string())??;
    drop(cancel);
    // No await between publishing the output and returning it to the caller's
    // PendingFile guard. Cancelled requests never publish worker output.
    if let Some(output) = output {
        platform::replace_file(&output, &destination)?;
    }
    Ok(response)
}

async fn run_worker(
    root: PathBuf,
    mut req: Request,
    seconds: u64,
    mut cancelled: tokio::sync::oneshot::Receiver<()>,
) -> AppResult<(Response, PendingDirectory, Option<PathBuf>)> {
    req.license = crate::licensing::saved_code(&root)?;
    crate::licensing::verify(&req.license, &crate::licensing::current_machine()?)?;
    let work = root.join("work").join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let pending = PendingDirectory(work.clone());
    let staged_output = if req.output.is_empty() {
        None
    } else {
        let extension = Path::new(&req.output)
            .extension()
            .ok_or("输出文件缺少扩展名")?;
        let output = work.join("output").with_extension(extension);
        req.output = output.to_string_lossy().into();
        Some(output)
    };
    let input = work.join("request.json");
    let output = work.join("response.json");
    let result = async {
        tokio::fs::write(&input, serde_json::to_vec(&req).map_err(|e| e.to_string())?)
            .await
            .map_err(|e| e.to_string())?;
        let mut cmd =
            tokio::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        cmd.arg("--worker")
            .arg(&input)
            .arg(&output)
            .current_dir(&work)
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd.spawn().map_err(|e| format!("无法启动设备进程：{e}"))?;
        #[cfg(unix)]
        let group = ProcessGroup(child.id().ok_or("无法读取设备进程标识。")?);
        let waited = tokio::select! {
            result = tokio::time::timeout(std::time::Duration::from_secs(seconds), child.wait()) => result,
            _ = &mut cancelled => {
                #[cfg(unix)]
                group.terminate();
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err("请求已取消。".into());
            }
        };
        match waited {
            Ok(status) => {
                let status = status.map_err(|e| e.to_string())?;
                if !status.success() && status.code() != Some(1) {
                    return Err(format!(
                        "设备进程异常结束（{status}）。如为打印，请检查主机队列，避免重复提交。"
                    ));
                }
            }
            Err(_) => {
                #[cfg(unix)]
                group.terminate();
                let _ = child.kill().await;
                let _ = child.wait().await;
                if req.operation == "office" {
                    return Err("Office/WPS 转换超时，已停止转换进程。文档可能需要密码、首次激活或交互操作，请在主机检查后重试，或先导出 PDF。".into());
                }
                if req.operation == "profile" {
                    return Err("驱动设置窗口已超时关闭，请重新创建预设。".into());
                }
                return Err(
                    "设备响应超时。打印任务可能已送达，请检查主机队列和出纸情况，勿直接重复提交。"
                        .into(),
                );
            }
        }
        let bytes = tokio::fs::read(&output)
            .await
            .map_err(|_| "设备进程异常退出，请检查文件和驱动。")?;
        let response: Response = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if let Some(e) = response.error {
            return Err(e);
        }
        Ok(response)
    }
    .await;
    result.map(|response| (response, pending, staged_output))
}

fn demo_devices() -> Devices {
    Devices {
        printers: vec![Printer {
            id: "demo-printer".into(),
            name: "演示 · 办公室打印机".into(),
            is_default: true,
            duplex: true,
            color: true,
            papers: vec!["A4".into(), "A5".into(), "Letter".into()],
        }],
        scanners: vec![Scanner {
            id: "demo-scanner".into(),
            name: "演示 · 平板扫描仪".into(),
        }],
        warnings: vec!["演示模式：不会连接真实设备或消耗纸张。".into()],
    }
}
fn demo_scan(output: &str, options: &ScanOptions) -> AppResult<()> {
    std::thread::sleep(std::time::Duration::from_secs(2));
    let mut img = image::RgbImage::from_pixel(1240, 1754, image::Rgb([248, 248, 246]));
    for (x, y, p) in img.enumerate_pixels_mut() {
        if x > 100 && x < 1140 && y > 160 && y < 260 {
            *p = image::Rgb([30, 91, 80]);
        }
        if x > 100 && x < 1050 && (400..1450).contains(&y) && y % 100 < 12 {
            *p = image::Rgb([180, 188, 183]);
        }
    }
    crate::imaging::save_scan(image::DynamicImage::ImageRgb8(img), output, options)
}
