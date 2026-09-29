//! macOS / Linux host backend: CUPS, Poppler, SANE and optional LibreOffice.
//! Commands receive separate arguments, never shell-interpolated upload names.
use crate::model::*;
use image::DynamicImage;
use serde::Serialize;
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

pub fn finish_worker(code: i32) -> ! {
    std::process::exit(code)
}
pub struct Apartment;
pub fn initialize() -> AppResult<Apartment> {
    Ok(Apartment)
}
pub fn show_error(message: &str) {
    eprintln!("LanPrint: {message}");
}
pub fn replace_file(from: &Path, to: &Path) -> AppResult<()> {
    std::fs::rename(from, to).map_err(|e| e.to_string())
}
pub fn single_instance(root: &Path) -> AppResult<Option<std::fs::File>> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join("instance.lock"))
        .map_err(|e| e.to_string())?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(e) => Err(format!("无法锁定数据目录：{e}")),
    }
}
pub fn lan_ip() -> String {
    (|| {
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.connect("192.0.2.1:80").ok()?;
        Some(socket.local_addr().ok()?.ip().to_string())
    })()
    .unwrap_or_else(|| "127.0.0.1".into())
}

fn executable(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let extra = [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
    ];
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .chain(extra.into_iter().map(PathBuf::from))
        .map(|dir| dir.join(name))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}
fn command(name: &str) -> AppResult<Command> {
    let path =
        executable(name).ok_or_else(|| format!("缺少 {name}，请按跨平台部署说明安装主机依赖。"))?;
    let mut cmd = Command::new(path);
    cmd.env("LC_ALL", "C").stdin(Stdio::null());
    Ok(cmd)
}
fn run(cmd: &mut Command) -> AppResult<Output> {
    let output = cmd.output().map_err(|e| format!("无法运行主机工具：{e}"))?;
    if !output.status.success() {
        return Err(format!(
            "主机工具失败（{}）：{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(1500)
                .collect::<String>()
        ));
    }
    Ok(output)
}
pub fn open(value: &str) {
    let name = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Ok(mut cmd) = command(name) {
        cmd.arg(value).stdout(Stdio::null()).stderr(Stdio::null());
        if let Ok(mut child) = cmd.spawn() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkStatus {
    pub port: u16,
    pub address: String,
    pub listening: bool,
    pub state: String,
    pub message: String,
    pub can_repair: bool,
    pub rule_configured: bool,
    pub profiles: Vec<String>,
    pub checked_at: u64,
}
impl NetworkStatus {
    pub fn demo(port: u16) -> Self {
        Self {
            port,
            address: lan_ip(),
            listening: true,
            state: "demo".into(),
            message: "演示模式不检查或修改防火墙。".into(),
            can_repair: false,
            rule_configured: false,
            profiles: Vec::new(),
            checked_at: now(),
        }
    }
}
pub fn inspect_network(port: u16) -> AppResult<NetworkStatus> {
    let listening = std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_secs(2),
    )
    .is_ok();
    Ok(NetworkStatus { port, address: lan_ip(), listening,
        state: if listening { "manual" } else { "not_listening" }.into(),
        message: if listening {
            "服务端口正在监听；请在系统防火墙允许此程序的局域网访问，再从另一台电脑验证。此平台不自动检查或修改防火墙规则。"
        } else { "未检测到服务端口监听，请检查主机配置。" }.into(),
        can_repair: false, rule_configured: false, profiles: Vec::new(), checked_at: now() })
}
pub fn repair_network_elevated(_: u16) -> AppResult<()> {
    Err("请由主机管理员在 macOS / Linux 系统防火墙中配置局域网访问权限。".into())
}
pub fn repair_network_entry(_: &[String]) -> AppResult<()> {
    repair_network_elevated(0)
}
pub fn capture_driver_profile(_: &str) -> AppResult<crate::profiles::DriverProfile> {
    Err("原厂驱动预设窗口仅在 Windows 提供；请在 CUPS / 系统打印机设置中管理默认选项。".into())
}

fn load_image(path: &str) -> AppResult<DynamicImage> {
    let mut reader = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16000);
    limits.max_image_height = Some(16000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().map_err(|e| format!("图片无法打开：{e}"))
}
fn white_rgb(img: DynamicImage) -> image::RgbImage {
    let rgba = img.to_rgba8();
    image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let p = rgba.get_pixel(x, y).0;
        let a = p[3] as u16;
        image::Rgb([0, 1, 2].map(|i| ((p[i] as u16 * a + 255 * (255 - a)) / 255) as u8))
    })
}
fn pdf_pages(info: &str) -> AppResult<u32> {
    let pages = info
        .lines()
        .find_map(|line| line.strip_prefix("Pages:")?.trim().parse().ok())
        .ok_or("无法读取 PDF 页数。")?;
    page_range("", pages)?;
    Ok(pages)
}
pub fn document_pages(path: &str) -> AppResult<u32> {
    if extension(path)? == "pdf" {
        let output = run(command("pdfinfo")?.arg(path))?;
        pdf_pages(&String::from_utf8_lossy(&output.stdout))
    } else {
        load_image(path)?;
        Ok(1)
    }
}
pub fn preview(path: &str, page: u32, output: &str) -> AppResult<()> {
    if page >= document_pages(path)? {
        return Err("页码超出范围。".into());
    }
    if extension(path)? == "pdf" {
        let prefix = Path::new(output).with_extension("");
        let number = (page + 1).to_string();
        run(command("pdftoppm")?
            .args([
                "-f",
                &number,
                "-l",
                &number,
                "-singlefile",
                "-scale-to",
                "1400",
                "-png",
            ])
            .arg(path)
            .arg(&prefix))?;
        let mut generated = prefix.into_os_string();
        generated.push(".png");
        let generated = PathBuf::from(generated);
        if generated != Path::new(output) {
            let _pending = crate::store::PendingFile(generated.clone());
            white_rgb(load_image(&generated.to_string_lossy())?)
                .save(output)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    } else {
        white_rgb(load_image(path)?.resize(1400, 1400, image::imageops::FilterType::Lanczos3))
            .save(output)
            .map_err(|e| e.to_string())
    }
}

// lpoptions -l lists PPD and driverless options in the same keyword/label form.
fn choices<'a>(text: &'a str, keys: &[&str]) -> Vec<&'a str> {
    text.lines()
        .filter_map(|line| {
            let (key, values) = line.split_once(':')?;
            keys.contains(&key.split('/').next()?).then_some(values)
        })
        .flat_map(str::split_whitespace)
        .map(|s| s.trim_start_matches('*'))
        .collect()
}
fn printer(id: &str, default: &str, options: &str) -> Printer {
    let media = choices(options, &["PageSize", "media", "media-supported"]);
    let papers = ["A4", "A5", "Letter"]
        .into_iter()
        .filter(|paper| {
            media.iter().any(|value| match *paper {
                "A4" => value.eq_ignore_ascii_case("A4") || value.starts_with("iso_a4_"),
                "A5" => value.eq_ignore_ascii_case("A5") || value.starts_with("iso_a5_"),
                _ => value.eq_ignore_ascii_case("Letter") || value.starts_with("na_letter_"),
            })
        })
        .map(str::to_string)
        .collect();
    let duplex = choices(options, &["Duplex", "sides"])
        .iter()
        .any(|v| v.starts_with("Duplex") || v.starts_with("two-sided"));
    let color = choices(
        options,
        &["ColorModel", "ColorMode", "print-color-mode", "OutputMode"],
    )
    .iter()
    .any(|v| ["rgb", "cmyk", "color", "cmy", "srgb"].contains(&v.to_ascii_lowercase().as_str()));
    Printer {
        id: id.into(),
        name: id.into(),
        is_default: id == default,
        duplex,
        color,
        papers,
    }
}
pub fn devices(show_virtual: bool) -> AppResult<Devices> {
    let mut result = Devices::default();
    match command("lpstat").and_then(|mut c| run(c.args(["-p"]))) {
        Ok(output) => {
            let default = command("lpstat")
                .and_then(|mut c| run(c.arg("-d")))
                .ok()
                .and_then(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .split_once(':')
                        .map(|(_, v)| v.trim().to_owned())
                })
                .unwrap_or_default();
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let Some(id) = line
                    .strip_prefix("printer ")
                    .and_then(|l| l.split_whitespace().next())
                else {
                    continue;
                };
                if !show_virtual && ["pdf", "fax"].iter().any(|s| id.to_lowercase().contains(s)) {
                    continue;
                }
                match command("lpoptions").and_then(|mut c| run(c.args(["-p", id, "-l"]))) {
                    Ok(o) => {
                        let p = printer(id, &default, &String::from_utf8_lossy(&o.stdout));
                        if p.papers.is_empty() {
                            result.warnings.push(format!(
                                "{id} 未报告 A4/A5/Letter 纸张能力，请检查 CUPS 驱动配置。"
                            ));
                        }
                        result.printers.push(p);
                    }
                    Err(e) => result
                        .warnings
                        .push(format!("无法读取 {id} 的打印能力：{e}")),
                }
            }
        }
        Err(e) => result.warnings.push(format!("CUPS 打印机：{e}")),
    }
    match command("scanimage").and_then(|mut c| run(c.arg("--formatted-device-list=%d\t%v %m%n"))) {
        Ok(output) => {
            result.scanners = String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| {
                    let (id, name) = line.split_once('\t')?;
                    Some(Scanner {
                        id: id.into(),
                        name: name.trim().into(),
                    })
                })
                .collect();
        }
        Err(e) => result.warnings.push(format!("SANE 扫描仪：{e}")),
    }
    Ok(result)
}
fn print_arguments(options: &PrintOptions, pages: u32) -> AppResult<Vec<String>> {
    if !(1..=99).contains(&options.copies)
        || ![300, 600].contains(&options.render_dpi)
        || !["A4", "A5", "Letter"].contains(&options.paper.as_str())
    {
        return Err("无效的打印参数。".into());
    }
    let sides = match options.duplex.as_str() {
        "simplex" => "one-sided",
        "long" => "two-sided-long-edge",
        "short" => "two-sided-short-edge",
        _ => return Err("无效的双面设置。".into()),
    };
    let ranges = page_range(&options.pages, pages)?
        .iter()
        .map(|p| (p + 1).to_string())
        .collect::<Vec<_>>()
        .join(",");
    Ok(vec![
        "-d".into(),
        options.printer.clone(),
        "-n".into(),
        options.copies.to_string(),
        "-t".into(),
        "LanPrint".into(),
        "-o".into(),
        format!("media={}", options.paper),
        "-o".into(),
        format!("sides={sides}"),
        "-o".into(),
        format!(
            "orientation-requested={}",
            if options.landscape { 4 } else { 3 }
        ),
        "-o".into(),
        format!(
            "print-color-mode={}",
            if options.color { "color" } else { "monochrome" }
        ),
        "-o".into(),
        format!("ColorModel={}", if options.color { "RGB" } else { "Gray" }),
        "-o".into(),
        format!("printer-resolution={}dpi", options.render_dpi),
        "-o".into(),
        "fit-to-page".into(),
        "-o".into(),
        format!("page-ranges={ranges}"),
    ])
}
pub fn print(
    path: &str,
    options: &PrintOptions,
    profile: Option<&crate::profiles::DriverProfile>,
) -> AppResult<()> {
    if profile.is_some() || !options.profile_id.is_empty() {
        return Err("当前平台不支持 Windows 驱动预设。".into());
    }
    let pages = document_pages(path)?;
    let args = print_arguments(options, pages)?;
    let mut cmd = command("lp")?;
    cmd.args(args);
    // Convert BMP and transparent images to a portable PDF for all CUPS drivers.
    let temporary = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .join(format!("lanprint-{}.pdf", uuid::Uuid::new_v4()));
    let _pending = crate::store::PendingFile(temporary.clone());
    let input = if extension(path)? == "pdf" {
        Path::new(path)
    } else {
        save_scan(
            load_image(path)?,
            &temporary.to_string_lossy(),
            &ScanOptions {
                dpi: options.render_dpi,
                color: options.color,
                ..Default::default()
            },
        )?;
        &temporary
    };
    run(cmd.arg("--").arg(input))?;
    Ok(())
}

fn scan_source(help: &str, requested: &str) -> AppResult<Option<String>> {
    let Some(values) = help
        .lines()
        .find_map(|l| l.trim().strip_prefix("--source "))
    else {
        return if requested == "flatbed" {
            Ok(None)
        } else {
            Err("此扫描仪未报告自动进纸器。".into())
        };
    };
    let values = values.split(" [").next().unwrap_or(values);
    values
        .split('|')
        .find(|v| {
            let v = v.trim().to_lowercase();
            if requested == "flatbed" {
                v.contains("flatbed") || v == "normal"
            } else {
                (v.contains("adf") || v.contains("automatic document feeder"))
                    && !v.contains("duplex")
            }
        })
        .map(|v| Some(v.trim().to_owned()))
        .ok_or_else(|| "扫描仪不支持所选纸张来源，请在主机检查 SANE 设备能力。".into())
}
pub fn scan(output: &str, options: &ScanOptions) -> AppResult<()> {
    if ![100, 200, 300].contains(&options.dpi)
        || !["flatbed", "feeder"].contains(&options.source.as_str())
        || !["pdf", "png", "jpg"].contains(&options.format.as_str())
    {
        return Err("无效的扫描参数。".into());
    }
    let help =
        run(command("scanimage")?.args(["--device-name", &options.scanner, "--all-options"]))?;
    let help = format!(
        "{}\n{}",
        String::from_utf8_lossy(&help.stdout),
        String::from_utf8_lossy(&help.stderr)
    );
    let raw = Path::new(output).with_file_name("scan-source.png");
    let _pending = crate::store::PendingFile(raw.clone());
    let mut cmd = command("scanimage")?;
    cmd.args([
        "--device-name",
        &options.scanner,
        "--format=png",
        "--mode",
        if options.color { "Color" } else { "Gray" },
        "--resolution",
        &options.dpi.to_string(),
    ]);
    if let Some(source) = scan_source(&help, &options.source)? {
        cmd.args(["--source", &source]);
    }
    run(cmd.arg("--output-file").arg(&raw))?;
    save_scan(load_image(&raw.to_string_lossy())?, output, options)
}

pub fn save_scan(img: DynamicImage, output: &str, options: &ScanOptions) -> AppResult<()> {
    let img = if options.color { img } else { img.grayscale() };
    if options.format != "pdf" {
        return img.save(output).map_err(|e| e.to_string());
    }
    let rgb = white_rgb(img);
    let (w, h) = rgb.dimensions();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode_image(&rgb)
        .map_err(|e| e.to_string())?;
    let width = w as f64 * 72.0 / options.dpi as f64;
    let height = h as f64 * 72.0 / options.dpi as f64;
    let content = format!("q {width:.3} 0 0 {height:.3} 0 0 cm /Im0 Do Q");
    let mut objects = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width:.3} {height:.3}] /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>").into_bytes()];
    let mut stream = format!("<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n", jpeg.len()).into_bytes();
    stream.extend(jpeg);
    stream.extend(b"\nendstream");
    objects.push(stream);
    objects.push(
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        )
        .into_bytes(),
    );
    let mut bytes = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = vec![0];
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        writeln!(bytes, "{} 0 obj", i + 1).map_err(|e| e.to_string())?;
        bytes.extend(obj);
        bytes.extend(b"\nendobj\n");
    }
    let xref = bytes.len();
    write!(bytes, "xref\n0 {}\n0000000000 65535 f \n", offsets.len()).map_err(|e| e.to_string())?;
    for offset in offsets.iter().skip(1) {
        writeln!(bytes, "{offset:010} 00000 n ").map_err(|e| e.to_string())?;
    }
    write!(
        bytes,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        offsets.len()
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(output, bytes).map_err(|e| e.to_string())
}

fn libreoffice() -> Option<PathBuf> {
    executable("libreoffice")
        .or_else(|| executable("soffice"))
        .or_else(|| {
            let path = PathBuf::from("/Applications/LibreOffice.app/Contents/MacOS/soffice");
            path.is_file().then_some(path)
        })
}
pub fn office_formats() -> Vec<&'static str> {
    if libreoffice().is_some() {
        vec!["doc", "docx", "xls", "xlsx", "ppt", "pptx"]
    } else {
        Vec::new()
    }
}
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(b) {
            uri.push(*b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}
pub fn convert_office(path: &str, output: &str) -> AppResult<()> {
    let program = libreoffice().ok_or("请安装 LibreOffice，或先将文档导出为 PDF。")?;
    let work = Path::new(output)
        .parent()
        .ok_or("无效的转换输出目录。")?
        .join("libreoffice");
    std::fs::create_dir_all(work.join("profile/user")).map_err(|e| e.to_string())?;
    let _pending = crate::store::PendingDirectory(work.clone());
    let work = std::fs::canonicalize(&work).map_err(|e| e.to_string())?;
    // A fresh profile prevents attaching to a user's open Office process and disables macros.
    std::fs::write(work.join("profile/user/registrymodifications.xcu"), br#"<?xml version="1.0" encoding="UTF-8"?>
<oor:items xmlns:oor="http://openoffice.org/2001/registry">
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="DisableMacrosExecution" oor:op="fuse"><value>true</value></prop></item>
</oor:items>"#).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(program);
    run(cmd
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .arg(format!(
            "-env:UserInstallation={}",
            file_uri(&work.join("profile"))
        ))
        .args([
            "--headless",
            "--nologo",
            "--nodefault",
            "--nofirststartwizard",
            "--convert-to",
            "pdf",
            "--outdir",
        ])
        .arg(&work)
        .arg(path))?;
    let mut filename = Path::new(path)
        .file_stem()
        .ok_or("无效的文档文件名。")?
        .to_os_string();
    filename.push(".pdf");
    let pdf = work.join(filename);
    if !pdf.is_file() {
        return Err("LibreOffice 未生成 PDF；请检查文档是否损坏或需要密码。".into());
    }
    document_pages(&pdf.to_string_lossy())?;
    replace_file(&pdf, Path::new(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn office_conversion_uses_isolated_profile_and_preserves_dotted_names() {
        if libreoffice().is_none() {
            assert!(
                std::env::var_os("LANPRINT_TEST_OFFICE").is_none(),
                "CI requires LibreOffice"
            );
            return;
        }
        let root = std::env::temp_dir().join(format!("lanprint-office-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let _pending = crate::store::PendingDirectory(root.clone());
        let source = root.join("example.document.doc");
        let output = root.join("converted.pdf");
        std::fs::write(&source, br"{\rtf1\ansi LanPrint conversion test\par}").unwrap();
        convert_office(&source.to_string_lossy(), &output.to_string_lossy()).unwrap();
        assert_eq!(document_pages(&output.to_string_lossy()).unwrap(), 1);
        assert!(!root.join("libreoffice").exists());
    }
    #[test]
    fn cups_capabilities_and_arguments() {
        let p = printer(
            "Office",
            "Office",
            "PageSize/Media: *A4 A5 Letter\nDuplex/Two-Sided: *None DuplexNoTumble DuplexTumble\nColorModel/Color: *Gray RGB\n",
        );
        assert!(p.is_default && p.duplex && p.color);
        assert_eq!(p.papers, ["A4", "A5", "Letter"]);
        let o = PrintOptions {
            printer: "Office; touch /tmp/no".into(),
            pages: "3,1-2".into(),
            copies: 2,
            duplex: "long".into(),
            ..Default::default()
        };
        let args = print_arguments(&o, 5).unwrap();
        assert_eq!(args[1], o.printer);
        assert!(args.contains(&"page-ranges=1,2,3".into()));
        assert!(args.contains(&"sides=two-sided-long-edge".into()));
        assert!(
            print_arguments(
                &PrintOptions {
                    pages: "0".into(),
                    ..o
                },
                5
            )
            .is_err()
        );
        assert!(!printer("mono", "", "PageSize: *A4\nColorModel: Gray\n").color);
    }
    #[test]
    fn source_selection_never_silently_uses_flatbed_for_adf() {
        let help = "  --source Flatbed|ADF Front|ADF Duplex [Flatbed]";
        assert_eq!(
            scan_source(help, "feeder").unwrap().as_deref(),
            Some("ADF Front")
        );
        assert_eq!(
            scan_source(help, "flatbed").unwrap().as_deref(),
            Some("Flatbed")
        );
        assert!(scan_source("", "feeder").is_err());
        assert!(scan_source("", "flatbed").unwrap().is_none());
    }
    #[test]
    fn pdf_info_rejects_empty_and_oversized_documents() {
        assert_eq!(pdf_pages("Title: A\nPages: 4\n").unwrap(), 4);
        for info in ["Pages: 0", "Pages: 1001", "Pages: bad", ""] {
            assert!(pdf_pages(info).is_err());
        }
        assert_eq!(file_uri(Path::new("/tmp/a b#c")), "file:///tmp/a%20b%23c");
    }
    #[test]
    fn lock_prevents_second_host_and_is_released() {
        let root = std::env::temp_dir().join(format!("lanprint-lock-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let _pending = crate::store::PendingDirectory(root.clone());
        let first = single_instance(&root).unwrap().unwrap();
        assert!(single_instance(&root).unwrap().is_none());
        drop(first);
        assert!(single_instance(&root).unwrap().is_some());
    }
    #[test]
    fn scan_pdf_renders_with_poppler() {
        let root = std::env::temp_dir().join(format!("lanprint-pdf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let _pending = crate::store::PendingDirectory(root.clone());
        let pdf = root.join("scan.pdf");
        let png = root.join("preview.png");
        save_scan(
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                100,
                200,
                image::Rgb([90, 130, 180]),
            )),
            &pdf.to_string_lossy(),
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(document_pages(&pdf.to_string_lossy()).unwrap(), 1);
        preview(&pdf.to_string_lossy(), 0, &png.to_string_lossy()).unwrap();
        let rendered = image::open(png).unwrap();
        assert_eq!((rendered.width(), rendered.height()), (700, 1400));
        let jpg = root.join("preview.page.jpg");
        preview(&pdf.to_string_lossy(), 0, &jpg.to_string_lossy()).unwrap();
        assert_eq!(
            image::ImageReader::open(&jpg)
                .unwrap()
                .decode()
                .unwrap()
                .width(),
            700
        );
        assert!(!root.join("preview.page.png").exists());
        assert!(
            preview(
                &pdf.to_string_lossy(),
                1,
                &root.join("invalid.png").to_string_lossy()
            )
            .is_err()
        );
    }
}
