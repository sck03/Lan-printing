//! Office/WPS automation runs only in a disposable STA worker.
use super::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::JobObjects::*;

fn candidates(ext: &str) -> &'static [&'static str] {
    match ext {
        "doc" | "docx" => &["Word.Application", "KWPS.Application", "WPS.Application"],
        "xls" | "xlsx" => &["Excel.Application", "KET.Application", "ET.Application"],
        "ppt" | "pptx" => &[
            "PowerPoint.Application",
            "KWPP.Application",
            "WPP.Application",
        ],
        "wps" => &["KWPS.Application", "WPS.Application"],
        "et" => &["KET.Application", "ET.Application"],
        "dps" => &["KWPP.Application", "WPP.Application"],
        _ => &[],
    }
}

pub fn office_formats() -> Vec<&'static str> {
    [
        "doc", "docx", "xls", "xlsx", "ppt", "pptx", "wps", "et", "dps",
    ]
    .into_iter()
    .filter(|ext| {
        candidates(ext)
            .iter()
            .any(|id| unsafe { CLSIDFromProgID(PCWSTR(wide(id).as_ptr())).is_ok() })
    })
    .collect()
}

struct Sta;
impl Drop for Sta {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

// A timeout/cancel kills the worker, closing this job and its dedicated Office
// process. Never attach an application process that predates this conversion:
// PowerPoint and some WPS versions may return the user's existing application.
fn application_processes(names: &[&str]) -> AppResult<Vec<u32>> {
    unsafe {
        let snapshot = Handle(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(err)?);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = Vec::new();
        if Process32FirstW(snapshot.0, &mut entry).is_ok() {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
                if names.contains(&name.as_str()) {
                    found.push(entry.th32ProcessID);
                }
                if Process32NextW(snapshot.0, &mut entry).is_err() {
                    break;
                }
            }
        }
        Ok(found)
    }
}

fn own_application(pid: u32, started: u64) -> AppResult<Handle> {
    unsafe {
        let process = Handle(
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_QUOTA | PROCESS_TERMINATE,
                false,
                pid,
            )
            .map_err(err)?,
        );
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user)
            .map_err(err)?;
        let created = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
        if created < started {
            return Err("办公软件正在被其他窗口使用，无法建立独立转换进程。请保存并关闭主机上的对应 Office/WPS 程序后重试，或先导出 PDF。".into());
        }
        let job = Handle(CreateJobObjectW(None, None).map_err(err)?);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .map_err(err)?;
        AssignProcessToJobObject(job.0, process.0)
            .map_err(|e| format!("无法隔离办公软件进程：{e}"))?;
        Ok(job)
    }
}

fn put(app: &IDispatch, name: &str, value: Var) -> AppResult<()> {
    invoke(app, name, DISPATCH_PROPERTYPUT, vec![value]).map(|_| ())
}
fn missing() -> Var {
    let mut value = VARIANT::default();
    unsafe {
        (*value.Anonymous.Anonymous).vt = VT_ERROR;
        (*value.Anonymous.Anonymous).Anonymous.scode = DISP_E_PARAMNOTFOUND.0;
    }
    Var(value)
}

pub fn convert_office(input: &str, output: &str) -> AppResult<()> {
    let ext = extension(input)?;
    let family = match ext.as_str() {
        "doc" | "docx" | "wps" => "word",
        "xls" | "xlsx" | "et" => "excel",
        "ppt" | "pptx" | "dps" => "slides",
        _ => return Err("不是支持的 Office/WPS 文档。".into()),
    };
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(err)?;
    }
    let _sta = Sta;
    let process_names: &[&str] = match family {
        "word" => &["winword.exe", "wps.exe"],
        "excel" => &["excel.exe", "et.exe"],
        _ => &["powerpnt.exe", "wpp.exe"],
    };
    let existing = application_processes(process_names)?;
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(err)?
        .as_nanos() as u64
        / 100
        + 116_444_736_000_000_000;
    let mut application = None;
    for id in candidates(&ext) {
        let result: windows::core::Result<IDispatch> = unsafe {
            CLSIDFromProgID(PCWSTR(wide(id).as_ptr()))
                .and_then(|clsid| CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER))
        };
        if let Ok(app) = result {
            application = Some(app);
            break;
        }
    }
    let app = application.ok_or("主机未安装或无法启动兼容的 Office/WPS。请安装并首次打开对应组件完成初始化，或将文档导出为 PDF 后上传。")?;
    let processes: Vec<u32> = application_processes(process_names)?
        .into_iter()
        .filter(|pid| !existing.contains(pid))
        .collect();
    if processes.len() != 1 {
        return Err("办公软件未创建独立转换进程。请先保存并关闭主机上的对应 Office/WPS 组件后重试，或先导出 PDF。".into());
    }
    let _job = own_application(processes[0], started)?;
    // Fail closed when the installed suite cannot disable document macros.
    put(&app, "AutomationSecurity", Var::int(3)).map_err(|_| {
        "此 Office/WPS 版本无法禁用文档宏，请升级办公软件或先导出 PDF。".to_string()
    })?;
    put(
        &app,
        "DisplayAlerts",
        Var::int(if family == "slides" { 1 } else { 0 }),
    )?;
    let input = automation_path(input);
    let output = automation_path(output);
    let document = match family {
        "word" => {
            put(&app, "Visible", Var::int(0))?;
            let options = get(&app, "Options")?.object()?;
            put(&options, "UpdateLinksAtOpen", Var::int(0))?;
            let documents = get(&app, "Documents")?.object()?;
            invoke(
                &documents,
                "Open",
                DISPATCH_METHOD,
                vec![
                    Var::string(&input),
                    Var::int(0),
                    Var::int(-1),
                    Var::int(0),
                    Var::string(&uuid::Uuid::new_v4().to_string()),
                    missing(),
                    Var::int(0),
                    missing(),
                    missing(),
                    missing(),
                    missing(),
                    Var::int(0),
                ],
            )?
            .object()?
        }
        "excel" => {
            put(&app, "Visible", Var::int(0))?;
            put(&app, "EnableEvents", Var::int(0))?;
            put(&app, "AskToUpdateLinks", Var::int(0))?;
            let books = get(&app, "Workbooks")?.object()?;
            invoke(
                &books,
                "Open",
                DISPATCH_METHOD,
                vec![
                    Var::string(&input),
                    Var::int(0),
                    Var::int(-1),
                    missing(),
                    Var::string(&uuid::Uuid::new_v4().to_string()),
                    missing(),
                    Var::int(-1),
                ],
            )?
            .object()?
        }
        _ => {
            let presentations = get(&app, "Presentations")?.object()?;
            invoke(
                &presentations,
                "Open",
                DISPATCH_METHOD,
                vec![Var::string(&input), Var::int(-1), Var::int(0), Var::int(0)],
            )?
            .object()?
        }
    };
    let exported = match family {
        "word" => invoke(
            &document,
            "ExportAsFixedFormat",
            DISPATCH_METHOD,
            vec![Var::string(&output), Var::int(17), Var::int(0)],
        ),
        "excel" => invoke(
            &document,
            "ExportAsFixedFormat",
            DISPATCH_METHOD,
            vec![Var::int(0), Var::string(&output)],
        ),
        _ => invoke(
            &document,
            "SaveAs",
            DISPATCH_METHOD,
            vec![Var::string(&output), Var::int(32)],
        ),
    };
    let _ = invoke(
        &document,
        "Close",
        DISPATCH_METHOD,
        if family == "slides" {
            vec![]
        } else {
            vec![Var::int(0)]
        },
    );
    drop(document);
    let _ = invoke(&app, "Quit", DISPATCH_METHOD, vec![]);
    exported.map_err(|e| {
        format!("文档转 PDF 失败：{e}。请确认文档未损坏、未加密，且办公软件支持 PDF 导出。")
    })?;
    let metadata = std::fs::metadata(&output).map_err(|_| "办公软件未生成 PDF 文件。")?;
    if metadata.len() == 0 || metadata.len() > 200 * 1024 * 1024 {
        return Err("转换后的 PDF 为空或超过 200 MB。".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_wps_formats_only_use_wps() {
        for ext in ["wps", "et", "dps"] {
            assert!(candidates(ext).iter().all(|id| !id.starts_with("Word.")
                && !id.starts_with("Excel.")
                && !id.starts_with("PowerPoint.")));
        }
        assert!(candidates("exe").is_empty());
    }
}
