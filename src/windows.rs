pub use crate::imaging::save_scan;
use crate::imaging::{load_image, white_rgb};
use crate::model::*;
#[path = "firewall.rs"]
mod firewall;
#[path = "office.rs"]
mod office;
pub use firewall::{NetworkStatus, inspect_network, repair_network_elevated, repair_network_entry};
use image::DynamicImage;
pub use office::{convert_office, office_formats};
use std::{
    mem::{ManuallyDrop, size_of},
    path::Path,
};
use windows::{
    Data::Pdf::{PdfDocument, PdfPageRenderOptions},
    Storage::{
        StorageFile,
        Streams::{DataReader, InMemoryRandomAccessStream},
    },
    Win32::{
        Foundation::*,
        Graphics::{Gdi::*, Printing::*},
        Storage::{FileSystem::*, Xps::*},
        System::{Com::*, LibraryLoader::*, Power::*, Threading::*, Variant::*, WinRT::*},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{BSTR, GUID, HSTRING, Interface, PCWSTR, w},
};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// MAKEINTRESOURCEW(1): Win32 treats the low word as a resource ID, not an address.
#[allow(clippy::manual_dangling_ptr)]
const APP_ICON_RESOURCE: PCWSTR = PCWSTR(1usize as *const u16);

/// Called only after worker::entry has returned, closed output files and dropped
/// COM/GDI resources. ExitProcess invokes third-party DLL detach hooks; some GPU
/// drivers crash there after WinRT PDF rendering. No device DLL is loaded into
/// the server process. End this disposable worker without running those hooks.
pub fn finish_worker(code: i32) -> ! {
    unsafe {
        let _ = TerminateProcess(GetCurrentProcess(), code as u32);
    }
    std::process::exit(code)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
// Rust canonicalize returns verbatim paths; WinRT/WIA Automation expects DOS/UNC paths.
fn automation_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else {
        path.strip_prefix("\\\\?\\").unwrap_or(path).to_string()
    }
}
pub struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}
pub fn initialize() -> AppResult<Apartment> {
    unsafe {
        RoInitialize(RO_INIT_MULTITHREADED).map_err(err)?;
    }
    Ok(Apartment)
}
pub fn replace_file(from: &Path, to: &Path) -> AppResult<()> {
    unsafe {
        MoveFileExW(
            PCWSTR(wide(&from.to_string_lossy()).as_ptr()),
            PCWSTR(wide(&to.to_string_lossy()).as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(err)
    }
}
pub fn single_instance(root: &Path) -> AppResult<Option<std::fs::File>> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .share_mode(0)
        .open(root.join("instance.lock"))
        .map(Some)
        .or_else(|e| {
            if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION.0 as i32) {
                Ok(None)
            } else {
                Err(e)
            }
        })
        .map_err(|e| format!("无法锁定数据目录；程序可能已在运行：{e}"))
}
pub fn lan_ip() -> String {
    (|| {
        let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        s.connect("192.0.2.1:80").ok()?;
        Some(s.local_addr().ok()?.ip().to_string())
    })()
    .unwrap_or_else(|| "127.0.0.1".into())
}
pub fn show_error(message: &str) {
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(wide(message).as_ptr()),
            w!("局域打印站"),
            MB_OK | MB_ICONERROR,
        );
    }
}
pub fn open(value: &str) {
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide(value).as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}

fn create_desktop_shortcut(startup: &crate::startup::Startup, demo: bool) -> AppResult<()> {
    let _apartment = initialize()?;
    unsafe {
        let desktop =
            SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None).map_err(err)?;
        let folder = desktop.to_string().map_err(err);
        CoTaskMemFree(Some(desktop.0.cast()));
        let destination = std::path::PathBuf::from(folder?).join(if demo {
            "局域打印站（演示）.lnk"
        } else {
            "局域打印站.lnk"
        });
        save_shortcut(startup, &destination)
    }
}

// Caller owns the COM apartment. Separate destination allows isolated tests.
fn save_shortcut(startup: &crate::startup::Startup, destination: &Path) -> AppResult<()> {
    unsafe {
        let exe = std::env::current_exe().map_err(err)?;
        let exe_path = wide(&automation_path(&exe.to_string_lossy()));
        let link: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(err)?;
        link.SetPath(PCWSTR(exe_path.as_ptr())).map_err(err)?;
        link.SetArguments(PCWSTR(wide(&startup.launch_arguments).as_ptr()))
            .map_err(err)?;
        link.SetDescription(w!("打开局域打印站，打印文件与扫描原稿"))
            .map_err(err)?;
        link.SetIconLocation(PCWSTR(exe_path.as_ptr()), 0)
            .map_err(err)?;
        if let Some(parent) = exe.parent() {
            link.SetWorkingDirectory(PCWSTR(
                wide(&automation_path(&parent.to_string_lossy())).as_ptr(),
            ))
            .map_err(err)?;
        }
        let file: IPersistFile = link.cast().map_err(err)?;
        file.Save(PCWSTR(wide(&destination.to_string_lossy()).as_ptr()), true)
            .map_err(err)
    }
}

unsafe extern "system" fn tray_window_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    unsafe {
        // Shell callbacks can be sent directly to WndProc, bypassing the
        // GetMessage queue. Forward them so all tray actions reach the loop.
        if msg == WM_APP + 1 {
            let _ = PostMessageW(Some(hwnd), WM_APP + 3, wp, lp);
            return LRESULT(0);
        }
        if msg == RegisterWindowMessageW(w!("TaskbarCreated")) {
            let _ = PostMessageW(Some(hwnd), WM_APP + 2, WPARAM(0), LPARAM(0));
            return LRESULT(0);
        }
        if msg == WM_DESTROY {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}
pub fn tray(
    url: &str,
    share: &str,
    root: &Path,
    demo: bool,
    background: bool,
    startup: &crate::startup::Startup,
    stop: tokio::sync::oneshot::Sender<()>,
) -> AppResult<()> {
    unsafe {
        let instance = GetModuleHandleW(None).map_err(err)?;
        // Resource 1 is the same multi-resolution icon displayed by Explorer.
        let app_icon = LoadIconW(Some(instance.into()), APP_ICON_RESOURCE).map_err(err)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(tray_window_proc),
            hInstance: instance.into(),
            hIcon: app_icon,
            lpszClassName: w!("LanPrintTray"),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err("创建托盘窗口失败".into());
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class.lpszClassName,
            w!("局域打印站"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(err)?;
        let mut icon = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_APP + 1,
            hIcon: app_icon,
            ..Default::default()
        };
        for (d, s) in icon.szTip.iter_mut().zip(wide(if demo {
            "局域打印站 · 演示"
        } else {
            "局域打印站"
        })) {
            *d = s;
        }
        if !Shell_NotifyIconW(NIM_ADD, &icon).as_bool() {
            return Err("创建系统托盘图标失败".into());
        }
        if !background {
            open(url);
        }
        let mut keep_awake = false;
        let mut msg = MSG::default();
        loop {
            let received = GetMessageW(&mut msg, None, 0, 0).0;
            if received <= 0 {
                break;
            }
            if msg.message == WM_APP + 2 {
                let _ = Shell_NotifyIconW(NIM_ADD, &icon);
            }
            if msg.message == WM_APP + 3 {
                if msg.lParam.0 as u32 == WM_LBUTTONDBLCLK {
                    open(url);
                }
                if matches!(msg.lParam.0 as u32, WM_RBUTTONUP | WM_CONTEXTMENU) {
                    let menu = CreatePopupMenu().map_err(err)?;
                    AppendMenuW(menu, MF_STRING, 1, w!("打开打印站")).map_err(err)?;
                    AppendMenuW(menu, MF_STRING, 2, w!("查看同事访问地址")).map_err(err)?;
                    AppendMenuW(menu, MF_STRING, 12, w!("检查与修复局域网访问")).map_err(err)?;
                    AppendMenuW(menu, MF_STRING, 3, w!("打开配置和数据目录")).map_err(err)?;
                    AppendMenuW(menu, MF_STRING, 9, w!("创建桌面快捷方式")).map_err(err)?;
                    AppendMenuW(menu, MF_SEPARATOR, 0, None).map_err(err)?;
                    let startup_state = startup.enabled();
                    let startup_flags = match &startup_state {
                        Ok(true) => MF_STRING | MF_CHECKED,
                        Ok(false) => MF_STRING,
                        Err(_) => MF_STRING | MF_GRAYED,
                    };
                    let startup_label = match &startup_state {
                        Ok(true) => "开机启动：已开启（当前用户登录后）",
                        Ok(false) => "开机启动：已关闭（当前用户登录后）",
                        Err(_) => "开机启动：无法读取设置",
                    };
                    AppendMenuW(menu, startup_flags, 5, PCWSTR(wide(startup_label).as_ptr()))
                        .map_err(err)?;
                    AppendMenuW(
                        menu,
                        MF_STRING | if keep_awake { MF_CHECKED } else { MF_UNCHECKED },
                        6,
                        w!("本次运行阻止自动睡眠"),
                    )
                    .map_err(err)?;
                    AppendMenuW(menu, MF_STRING, 7, w!("打开 Windows 打印机设置")).map_err(err)?;
                    AppendMenuW(menu, MF_STRING, 11, w!("管理照片打印预设")).map_err(err)?;
                    AppendMenuW(menu, MF_STRING, 10, w!("打开 Windows 启动应用设置"))
                        .map_err(err)?;
                    AppendMenuW(menu, MF_STRING, 8, w!("关于与兼容性")).map_err(err)?;
                    AppendMenuW(menu, MF_SEPARATOR, 0, None).map_err(err)?;
                    AppendMenuW(menu, MF_STRING, 4, w!("退出打印站")).map_err(err)?;
                    let mut point = POINT::default();
                    GetCursorPos(&mut point).map_err(err)?;
                    let _ = SetForegroundWindow(hwnd);
                    let selected = TrackPopupMenu(
                        menu,
                        TPM_RETURNCMD | TPM_RIGHTBUTTON,
                        point.x,
                        point.y,
                        Some(0),
                        hwnd,
                        None,
                    )
                    .0;
                    let _ = DestroyMenu(menu);
                    let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
                    match selected {
                        1 => open(url),
                        12 => open(&format!("{url}/#network-card")),
                        2 => {
                            MessageBoxW(Some(hwnd),PCWSTR(wide(&format!("同事在浏览器打开：\n{share}\n\n需连接同一内网，Windows 防火墙允许本程序访问专用网络。")).as_ptr()),w!("共享地址"),MB_OK);
                        }
                        3 => open(&root.to_string_lossy()),
                        4 => break,
                        5 => {
                            if let Ok(enabled) = startup_state
                                && let Err(e) = startup.set_enabled(!enabled)
                            {
                                show_error(&e);
                            }
                        }
                        6 => {
                            let flags = ES_CONTINUOUS
                                | if keep_awake {
                                    EXECUTION_STATE(0)
                                } else {
                                    ES_SYSTEM_REQUIRED
                                };
                            if SetThreadExecutionState(flags).0 == 0 {
                                show_error("无法修改本次运行的睡眠设置。");
                            } else {
                                keep_awake = !keep_awake;
                            }
                        }
                        7 => open("ms-settings:printers"),
                        11 => open(&format!("{url}/#driver-profiles")),
                        9 => match create_desktop_shortcut(startup, demo) {
                            Ok(()) => {
                                MessageBoxW(
                                    Some(hwnd),
                                    w!(
                                        "已创建桌面快捷方式。请将程序保留在当前位置，避免快捷方式失效。"
                                    ),
                                    w!("局域打印站"),
                                    MB_OK | MB_ICONINFORMATION,
                                );
                            }
                            Err(e) => show_error(&format!("创建桌面快捷方式失败：{e}")),
                        },
                        10 => open("ms-settings:startupapps"),
                        8 => {
                            let message = format!(
                                "局域打印站 {}\n\nWindows 10/11 x64 · Rust 原生程序\nPDF：Windows 系统渲染；图片：CPU 处理\n无 AMD / NVIDIA / Intel 专用依赖。\n不同显卡与设备驱动需实机验收。\n\n自动启动在当前用户登录后生效，默认关闭。\n如被系统禁用，请检查 Windows 启动应用。\n防睡眠仅本次运行有效，不阻止手动睡眠。",
                                env!("CARGO_PKG_VERSION")
                            );
                            MessageBoxW(
                                Some(hwnd),
                                PCWSTR(wide(&message).as_ptr()),
                                w!("关于局域打印站"),
                                MB_OK,
                            );
                        }
                        _ => {}
                    }
                }
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = Shell_NotifyIconW(NIM_DELETE, &icon);
        SetThreadExecutionState(ES_CONTINUOUS);
        let _ = DestroyWindow(hwnd);
    }
    let _ = stop.send(());
    Ok(())
}

fn pdf(path: &str) -> AppResult<PdfDocument> {
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(automation_path(path)))
        .map_err(err)?
        .join()
        .map_err(err)?;
    PdfDocument::LoadFromFileAsync(&file)
        .map_err(err)?
        .join()
        .map_err(|e| format!("PDF 无法打开（可能损坏或有密码）：{e}"))
}
pub fn document_pages(path: &str) -> AppResult<u32> {
    if extension(path)? == "pdf" {
        let n = pdf(path)?.PageCount().map_err(err)?;
        page_range("", n)?;
        Ok(n)
    } else {
        load_image(path)?;
        Ok(1)
    }
}
fn render(path: &str, page: u32, max: u32) -> AppResult<DynamicImage> {
    if extension(path)? != "pdf" {
        if page != 0 {
            return Err("页码超出范围".into());
        }
        return Ok(load_image(path)?.resize(max, max, image::imageops::FilterType::Lanczos3));
    }
    let doc = pdf(path)?;
    let p = doc.GetPage(page).map_err(err)?;
    let size = p.Size().map_err(err)?;
    if size.Width <= 0.0 || size.Height <= 0.0 {
        return Err("PDF 页面尺寸无效".into());
    }
    let options = PdfPageRenderOptions::new().map_err(err)?;
    let scale = max as f32 / size.Width.max(size.Height);
    options
        .SetDestinationWidth((size.Width * scale).max(1.0) as u32)
        .map_err(err)?;
    options
        .SetDestinationHeight((size.Height * scale).max(1.0) as u32)
        .map_err(err)?;
    let stream = InMemoryRandomAccessStream::new().map_err(err)?;
    p.RenderWithOptionsToStreamAsync(&stream, &options)
        .map_err(err)?
        .join()
        .map_err(err)?;
    let count = stream.Size().map_err(err)?;
    if count > 128 * 1024 * 1024 {
        return Err("页面渲染过大".into());
    }
    let reader =
        DataReader::CreateDataReader(&stream.GetInputStreamAt(0).map_err(err)?).map_err(err)?;
    reader
        .LoadAsync(count as u32)
        .map_err(err)?
        .join()
        .map_err(err)?;
    let mut bytes = vec![0; count as usize];
    reader.ReadBytes(&mut bytes).map_err(err)?;
    let result = image::load_from_memory(&bytes).map_err(err);
    let _ = p.Close();
    result
}
pub fn preview(path: &str, page: u32, output: &str) -> AppResult<()> {
    white_rgb(render(path, page, 1400)?)
        .save(output)
        .map_err(err)
}

pub fn devices(show_virtual: bool) -> AppResult<Devices> {
    let mut result = Devices::default();
    unsafe {
        let mut needed = 0;
        let mut count = 0;
        let _ = EnumPrintersW(
            PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS,
            None,
            4,
            None,
            &mut needed,
            &mut count,
        );
        let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
        if needed > 0 {
            let bytes =
                std::slice::from_raw_parts_mut(buffer.as_mut_ptr() as *mut u8, needed as usize);
            EnumPrintersW(
                PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS,
                None,
                4,
                Some(bytes),
                &mut needed,
                &mut count,
            )
            .map_err(err)?;
            let mut default = vec![0u16; 1024];
            let mut len = default.len() as u32;
            let _ = GetDefaultPrinterW(Some(windows::core::PWSTR(default.as_mut_ptr())), &mut len);
            let default = String::from_utf16_lossy(
                &default[..default.iter().position(|c| *c == 0).unwrap_or(0)],
            );
            for p in std::slice::from_raw_parts(
                buffer.as_ptr() as *const PRINTER_INFO_4W,
                count as usize,
            ) {
                let name = p.pPrinterName.to_string().map_err(err)?;
                let lower = name.to_lowercase();
                if !show_virtual
                    && ["pdf", "xps", "onenote", "fax"]
                        .iter()
                        .any(|v| lower.contains(v))
                {
                    continue;
                }
                let n = wide(&name);
                let cap = |c| DeviceCapabilitiesW(PCWSTR(n.as_ptr()), None, c, None, None);
                let paper_count = cap(DC_PAPERS);
                let mut papers = Vec::new();
                if paper_count > 0 && paper_count < 10000 {
                    let mut ids = vec![0u16; paper_count as usize];
                    DeviceCapabilitiesW(
                        PCWSTR(n.as_ptr()),
                        None,
                        DC_PAPERS,
                        Some(windows::core::PWSTR(ids.as_mut_ptr())),
                        None,
                    );
                    for (id, label) in [(9, "A4"), (11, "A5"), (1, "Letter")] {
                        if ids.contains(&id) {
                            papers.push(label.into());
                        }
                    }
                }
                result.printers.push(Printer {
                    id: name.clone(),
                    name: name.clone(),
                    is_default: name == default,
                    duplex: cap(DC_DUPLEX) > 0,
                    color: cap(DC_COLORDEVICE) > 0,
                    papers,
                });
            }
        }
    }
    match scanners() {
        Ok(s) => result.scanners = s,
        Err(e) => result.warnings.push(format!("扫描设备读取失败：{e}")),
    }
    Ok(result)
}

struct PrinterHandle(PRINTER_HANDLE);
impl Drop for PrinterHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = ClosePrinter(self.0);
        }
    }
}

fn printer_defaults(printer: &str) -> AppResult<(PrinterHandle, Vec<usize>)> {
    unsafe {
        let name = wide(printer);
        let mut handle = PRINTER_HANDLE::default();
        OpenPrinterW(PCWSTR(name.as_ptr()), &mut handle, None).map_err(err)?;
        let handle = PrinterHandle(handle);
        let size = DocumentPropertiesW(None, handle.0, PCWSTR(name.as_ptr()), None, None, 0);
        if size < size_of::<DEVMODEW>() as i32 || size > 1024 * 1024 {
            return Err("无法读取打印机设置。".into());
        }
        let mut data = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        if DocumentPropertiesW(
            None,
            handle.0,
            PCWSTR(name.as_ptr()),
            Some(data.as_mut_ptr().cast()),
            None,
            DM_OUT_BUFFER.0,
        ) != IDOK.0
        {
            return Err("读取打印机设置失败。".into());
        }
        Ok((handle, data))
    }
}

fn driver_signature(handle: PRINTER_HANDLE) -> AppResult<String> {
    unsafe {
        let mut needed = 0;
        let _ = GetPrinterDriverW(handle, None, 6, None, &mut needed);
        if needed == 0 || needed > 1024 * 1024 {
            return Err("无法读取打印机驱动版本。".into());
        }
        let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
        let bytes = std::slice::from_raw_parts_mut(buffer.as_mut_ptr().cast(), needed as usize);
        GetPrinterDriverW(handle, None, 6, Some(bytes), &mut needed)
            .ok()
            .map_err(err)?;
        let driver = &*(buffer.as_ptr() as *const DRIVER_INFO_6W);
        Ok(format!(
            "{}:{}:{}:{}",
            driver.pName.to_string().map_err(err)?,
            driver.dwlDriverVersion,
            driver.ftDriverDate.dwHighDateTime,
            driver.ftDriverDate.dwLowDateTime
        ))
    }
}

pub fn capture_driver_profile(printer: &str) -> AppResult<crate::profiles::DriverProfile> {
    // Vendor property sheets may require COM/OLE on a UI apartment.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(err)?;
    }
    struct UiApartment;
    impl Drop for UiApartment {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }
    let _apartment = UiApartment;
    read_driver_profile(printer, true)
}
fn read_driver_profile(printer: &str, prompt: bool) -> AppResult<crate::profiles::DriverProfile> {
    let (handle, mut data) = printer_defaults(printer)?;
    let signature = driver_signature(handle.0)?;
    unsafe {
        let mode = data.as_mut_ptr() as *mut DEVMODEW;
        let name = wide(printer);
        let result = DocumentPropertiesW(
            None,
            handle.0,
            PCWSTR(name.as_ptr()),
            Some(mode),
            Some(mode),
            (DM_IN_BUFFER
                | DM_OUT_BUFFER
                | if prompt {
                    DM_IN_PROMPT
                } else {
                    DEVMODE_FIELD_FLAGS(0)
                })
            .0,
        );
        if result == IDCANCEL.0 {
            return Err("已取消驱动预设设置。".into());
        }
        if result != IDOK.0 {
            return Err("打开打印机原厂设置失败。".into());
        }
        let size = (*mode).dmSize as usize + (*mode).dmDriverExtra as usize;
        if size < size_of::<DEVMODEW>() || size > data.len() * size_of::<usize>() {
            return Err("打印驱动返回了无效设置。".into());
        }
        Ok(crate::profiles::DriverProfile {
            id: uuid::Uuid::new_v4().to_string(),
            name: String::new(),
            printer: printer.into(),
            driver: signature,
            devmode: std::slice::from_raw_parts(data.as_ptr().cast(), size).to_vec(),
        })
    }
}

pub fn print(
    path: &str,
    options: &PrintOptions,
    profile: Option<&crate::profiles::DriverProfile>,
) -> AppResult<()> {
    print_with_profile(path, options, None, profile)
}
fn print_with_profile(
    path: &str,
    options: &PrintOptions,
    output: Option<&str>,
    profile: Option<&crate::profiles::DriverProfile>,
) -> AppResult<()> {
    unsafe {
        let name = wide(&options.printer);
        let (printer, mut buf) = printer_defaults(&options.printer)?;
        let handle = printer.0;
        let dm = buf.as_mut_ptr() as *mut DEVMODEW;
        if let Some(profile) = profile {
            if profile.printer != options.printer || profile.driver != driver_signature(handle)? {
                return Err("打印机或驱动版本与预设不匹配，请在主机重新创建预设。".into());
            }
            let size = (*dm).dmSize as usize + (*dm).dmDriverExtra as usize;
            if profile.devmode.len() != size
                || size > buf.len() * size_of::<usize>()
                || size < size_of::<DEVMODEW>()
            {
                return Err("驱动预设大小不匹配，请重新创建。".into());
            }
            // Copy the complete private driver data, including Epson media,
            // quality, bidirectional/high-speed, and color-management options.
            let saved = std::ptr::read_unaligned(profile.devmode.as_ptr().cast::<DEVMODEW>());
            if saved.dmSize != (*dm).dmSize
                || saved.dmDriverExtra != (*dm).dmDriverExtra
                || saved.dmDriverVersion != (*dm).dmDriverVersion
                || saved.dmSpecVersion != (*dm).dmSpecVersion
            {
                return Err("驱动预设格式不兼容，请重新创建。".into());
            }
            std::ptr::copy_nonoverlapping(profile.devmode.as_ptr(), dm.cast(), size);
        } else if !options.profile_id.is_empty() {
            return Err("打印任务缺少驱动预设，已停止打印。".into());
        } else {
            (*dm).dmFields |= DM_ORIENTATION | DM_PAPERSIZE | DM_DUPLEX | DM_COLOR | DM_COPIES;
            (*dm).Anonymous1.Anonymous1.dmOrientation = if options.landscape { 2 } else { 1 };
            (*dm).Anonymous1.Anonymous1.dmPaperSize = match options.paper.as_str() {
                "A5" => 11,
                "Letter" => 1,
                _ => 9,
            };
            (*dm).dmDuplex = DEVMODE_DUPLEX(match options.duplex.as_str() {
                "long" => 2,
                "short" => 3,
                _ => 1,
            });
            (*dm).dmColor = DEVMODE_COLOR(if options.color { 2 } else { 1 });
        }
        // Copies are applied by our page loop, never twice by the driver.
        (*dm).dmFields |= DM_COPIES;
        (*dm).Anonymous1.Anonymous1.dmCopies = 1;
        if DocumentPropertiesW(
            None,
            handle,
            PCWSTR(name.as_ptr()),
            Some(dm),
            Some(dm),
            (DM_IN_BUFFER | DM_OUT_BUFFER).0,
        ) < 0
        {
            return Err("打印机拒绝所选参数".into());
        }
        let color = (*dm).dmColor.0 == 2;
        let duplex = (*dm).dmFields.contains(DM_DUPLEX) && matches!((*dm).dmDuplex.0, 2 | 3);
        let dc = CreateDCW(w!("WINSPOOL"), PCWSTR(name.as_ptr()), None, Some(dm));
        if dc.is_invalid() {
            return Err("创建打印设备失败".into());
        }
        struct Dc(HDC);
        impl Drop for Dc {
            fn drop(&mut self) {
                unsafe {
                    let _ = DeleteDC(self.0);
                }
            }
        }
        let _dc = Dc(dc);
        let pages = page_range(&options.pages, document_pages(path)?)?;
        let title = wide("局域打印站文档");
        let output = output.map(wide);
        let info = DOCINFOW {
            cbSize: size_of::<DOCINFOW>() as i32,
            lpszDocName: PCWSTR(title.as_ptr()),
            lpszOutput: output
                .as_ref()
                .map_or(PCWSTR::null(), |s| PCWSTR(s.as_ptr())),
            ..Default::default()
        };
        if StartDocW(dc, &info) <= 0 {
            return Err("无法创建 Windows 打印任务".into());
        }
        let result = (|| -> AppResult<()> {
            let width = GetDeviceCaps(Some(dc), HORZRES);
            let height = GetDeviceCaps(Some(dc), VERTRES);
            let dpi_x = GetDeviceCaps(Some(dc), LOGPIXELSX) as f64;
            let dpi_y = GetDeviceCaps(Some(dc), LOGPIXELSY) as f64;
            if width <= 0 || height <= 0 || dpi_x <= 0.0 || dpi_y <= 0.0 {
                return Err("打印区域尺寸无效".into());
            }
            let render_max = ((width as f64 / dpi_x).max(height as f64 / dpi_y)
                * options.render_dpi as f64)
                .ceil()
                .clamp(1.0, 7016.0) as u32;
            for _ in 0..options.copies {
                for &page in &pages {
                    let img = white_rgb(render(path, page, render_max)?);
                    let (w, h) = img.dimensions();
                    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
                    for p in img.pixels() {
                        let c = if color {
                            p.0
                        } else {
                            let g =
                                (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000;
                            [g as u8; 3]
                        };
                        pixels.extend_from_slice(&[c[2], c[1], c[0], 0]);
                    }
                    let bitmap = BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: w as i32,
                            biHeight: -(h as i32),
                            biPlanes: 1,
                            biBitCount: 32,
                            biCompression: BI_RGB.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    let scale =
                        (width as f64 / dpi_x / w as f64).min(height as f64 / dpi_y / h as f64);
                    let dw = (w as f64 * scale * dpi_x) as i32;
                    let dh = (h as f64 * scale * dpi_y) as i32;
                    if StartPage(dc) <= 0 {
                        return Err("开始打印页失败".into());
                    }
                    SetStretchBltMode(dc, HALFTONE);
                    if StretchDIBits(
                        dc,
                        (width - dw) / 2,
                        (height - dh) / 2,
                        dw,
                        dh,
                        0,
                        0,
                        w as i32,
                        h as i32,
                        Some(pixels.as_ptr().cast()),
                        &bitmap,
                        DIB_RGB_COLORS,
                        SRCCOPY,
                    ) <= 0
                    {
                        return Err("提交打印页失败".into());
                    }
                    if EndPage(dc) <= 0 {
                        return Err("结束打印页失败".into());
                    }
                }
                // Keep copies separate when the selected range has an odd page count.
                if duplex && pages.len() % 2 == 1 && (StartPage(dc) <= 0 || EndPage(dc) <= 0) {
                    return Err("提交双面分隔页失败".into());
                }
            }
            if EndDoc(dc) <= 0 {
                return Err("提交打印任务失败".into());
            }
            Ok(())
        })();
        if result.is_err() {
            AbortDoc(dc);
        }
        result
    }
}

// WIA Automation is used directly through COM. Each worker owns its apartment.
struct Var(VARIANT);
impl Drop for Var {
    fn drop(&mut self) {
        unsafe {
            let _ = VariantClear(&mut self.0);
        }
    }
}
impl Var {
    fn int(n: i32) -> Self {
        let mut v = VARIANT::default();
        unsafe {
            (*v.Anonymous.Anonymous).vt = VT_I4;
            (*v.Anonymous.Anonymous).Anonymous.lVal = n;
        }
        Self(v)
    }
    fn string(s: &str) -> Self {
        let mut v = VARIANT::default();
        unsafe {
            (*v.Anonymous.Anonymous).vt = VT_BSTR;
            (*v.Anonymous.Anonymous).Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(s));
        }
        Self(v)
    }
    fn number(&self) -> AppResult<i32> {
        unsafe { VariantToInt32(&self.0).map_err(err) }
    }
    fn text(&self) -> AppResult<String> {
        unsafe {
            if self.0.Anonymous.Anonymous.vt != VT_BSTR {
                return Err("WIA 返回了非文本值".into());
            }
            Ok(self.0.Anonymous.Anonymous.Anonymous.bstrVal.to_string())
        }
    }
    fn object(&self) -> AppResult<IDispatch> {
        unsafe {
            if self.0.Anonymous.Anonymous.vt != VT_DISPATCH {
                return Err("WIA 返回了无效对象".into());
            }
            self.0
                .Anonymous
                .Anonymous
                .Anonymous
                .pdispVal
                .as_ref()
                .cloned()
                .ok_or("WIA 对象为空".into())
        }
    }
}
fn invoke(
    obj: &IDispatch,
    name: &str,
    flags: DISPATCH_FLAGS,
    mut args: Vec<Var>,
) -> AppResult<Var> {
    unsafe {
        let method = name;
        let name = wide(name);
        let mut id = 0;
        obj.GetIDsOfNames(&GUID::zeroed(), &PCWSTR(name.as_ptr()), 1, 0, &mut id)
            .map_err(|e| format!("COM {method}: {e}"))?;
        args.reverse();
        let mut raw: Vec<VARIANT> = args.iter().map(|v| std::ptr::read(&v.0)).collect();
        let mut named = -3;
        let params = DISPPARAMS {
            rgvarg: raw.as_mut_ptr(),
            cArgs: raw.len() as u32,
            rgdispidNamedArgs: if flags == DISPATCH_PROPERTYPUT {
                &mut named
            } else {
                std::ptr::null_mut()
            },
            cNamedArgs: if flags == DISPATCH_PROPERTYPUT { 1 } else { 0 },
        };
        let mut out = Var(VARIANT::default());
        obj.Invoke(
            id,
            &GUID::zeroed(),
            0,
            flags,
            &params,
            Some(&mut out.0),
            None,
            None,
        )
        .map_err(|e| format!("COM {method}: {e}"))?;
        Ok(out)
    }
}
fn get(obj: &IDispatch, name: &str) -> AppResult<Var> {
    invoke(obj, name, DISPATCH_PROPERTYGET, vec![])
}
fn item(obj: &IDispatch, index: Var) -> AppResult<IDispatch> {
    invoke(obj, "Item", DISPATCH_PROPERTYGET, vec![index])?.object()
}
fn manager() -> AppResult<IDispatch> {
    unsafe {
        CoCreateInstance(
            &CLSIDFromProgID(w!("WIA.DeviceManager")).map_err(err)?,
            None,
            CLSCTX_LOCAL_SERVER | CLSCTX_INPROC_SERVER,
        )
        .map_err(err)
    }
}
fn scanner_infos() -> AppResult<Vec<IDispatch>> {
    let infos = get(&manager()?, "DeviceInfos")?.object()?;
    let mut result = Vec::new();
    for i in 1..=get(&infos, "Count")?.number()? {
        let info = item(&infos, Var::int(i))?;
        if get(&info, "Type")?.number()? == 1 {
            result.push(info);
        }
    }
    Ok(result)
}
fn scanners() -> AppResult<Vec<Scanner>> {
    scanner_infos()?
        .iter()
        .map(|info| {
            let id = get(info, "DeviceID")?.text()?;
            let props = get(info, "Properties")?.object()?;
            let name = get(&item(&props, Var::string("Name"))?, "Value")?.text()?;
            Ok(Scanner { id, name })
        })
        .collect()
}
fn set_prop(obj: &IDispatch, id: i32, value: i32) -> AppResult<()> {
    let props = get(obj, "Properties")?.object()?;
    let p = item(&props, Var::int(id))?;
    invoke(&p, "Value", DISPATCH_PROPERTYPUT, vec![Var::int(value)])?;
    Ok(())
}
pub fn scan(output: &str, options: &ScanOptions) -> AppResult<()> {
    let info = scanner_infos()?
        .into_iter()
        .find(|i| {
            get(i, "DeviceID")
                .and_then(|v| v.text())
                .is_ok_and(|id| id == options.scanner)
        })
        .ok_or("扫描仪未连接")?;
    let device = invoke(&info, "Connect", DISPATCH_METHOD, vec![])?.object()?;
    // Flatbed-only devices may not expose Document Handling Select.
    let device_props = get(&device, "Properties")?.object()?;
    match item(&device_props, Var::int(3088)) {
        Ok(prop) => {
            invoke(
                &prop,
                "Value",
                DISPATCH_PROPERTYPUT,
                vec![Var::int(if options.source == "feeder" { 1 } else { 2 })],
            )
            .map_err(|e| format!("扫描仪拒绝所选原稿来源：{e}"))?;
        }
        Err(e) if options.source == "feeder" => return Err(format!("无法使用自动进纸器：{e}")),
        Err(_) => {}
    }
    if options.source == "feeder" {
        let _ = set_prop(&device, 3096, 1);
    }
    let scan_item = item(&get(&device, "Items")?.object()?, Var::int(1))?;
    set_prop(&scan_item, 6146, if options.color { 1 } else { 2 })?;
    set_prop(&scan_item, 6147, options.dpi as i32)?;
    set_prop(&scan_item, 6148, options.dpi as i32)?;
    // Resolution changes can leave a previous pixel crop in the driver. Reset
    // the origin, then use the driver's valid extent at the negotiated DPI.
    for id in [6149, 6150] {
        set_prop(&scan_item, id, 0)?;
    }
    let props = get(&scan_item, "Properties")?.object()?;
    for id in [6151, 6152] {
        let prop = item(&props, Var::int(id))?;
        if let Ok(max) = get(&prop, "SubTypeMax").and_then(|v| v.number())
            && max > 0
        {
            invoke(&prop, "Value", DISPATCH_PROPERTYPUT, vec![Var::int(max)])?;
        }
    }
    let transferred = invoke(
        &scan_item,
        "Transfer",
        DISPATCH_METHOD,
        vec![Var::string("{B96B3CAB-0728-11D3-9D7B-0000F81EF32E}")],
    )?
    .object()?;
    let temp = Path::new(output).with_extension("wia.bmp");
    let result = (|| {
        invoke(
            &transferred,
            "SaveFile",
            DISPATCH_METHOD,
            vec![Var::string(&automation_path(&temp.to_string_lossy()))],
        )?;
        save_scan(load_image(&temp.to_string_lossy())?, output, options)
    })();
    let _ = std::fs::remove_file(temp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installed_driver_preserves_full_private_settings() {
        let Ok(printer) = std::env::var("LANPRINT_TEST_DRIVER") else {
            return;
        };
        let _apartment = initialize().unwrap();
        let profile = read_driver_profile(&printer, false).unwrap();
        let saved: crate::profiles::DriverProfile =
            serde_json::from_slice(&serde_json::to_vec(&profile).unwrap()).unwrap();
        assert_eq!(saved.devmode, profile.devmode);
        let mode = unsafe { std::ptr::read_unaligned(saved.devmode.as_ptr().cast::<DEVMODEW>()) };
        assert_eq!(
            saved.devmode.len(),
            mode.dmSize as usize + mode.dmDriverExtra as usize
        );
        println!(
            "Driver preset: {} bytes, private settings: {} bytes",
            saved.devmode.len(),
            mode.dmDriverExtra
        );
        let options = PrintOptions {
            printer,
            profile_id: saved.id.clone(),
            ..Default::default()
        };
        let mut invalid = saved;
        invalid.driver.push_str("-invalid");
        assert!(
            print_with_profile("unused.pdf", &options, None, Some(&invalid))
                .unwrap_err()
                .contains("驱动版本")
        );
    }
    #[test]
    fn sent_shell_callbacks_reach_tray_queue_and_icon_is_embedded() {
        unsafe {
            let instance = GetModuleHandleW(None).unwrap();
            let icon = LoadIconW(Some(instance.into()), APP_ICON_RESOURCE).unwrap();
            assert!(!icon.is_invalid());
            let class = WNDCLASSW {
                lpfnWndProc: Some(tray_window_proc),
                hInstance: instance.into(),
                lpszClassName: w!("LanPrintTrayTest"),
                ..Default::default()
            };
            assert_ne!(RegisterClassW(&class), 0);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class.lpszClassName,
                w!(""),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .unwrap();
            for event in [WM_RBUTTONUP, WM_LBUTTONDBLCLK, WM_CONTEXTMENU] {
                SendMessageW(
                    hwnd,
                    WM_APP + 1,
                    Some(WPARAM(1)),
                    Some(LPARAM(event as isize)),
                );
                let mut msg = MSG::default();
                assert!(
                    PeekMessageW(&mut msg, Some(hwnd), WM_APP + 3, WM_APP + 3, PM_REMOVE).as_bool()
                );
                assert_eq!(msg.lParam.0, event as isize);
            }
            SendMessageW(
                hwnd,
                RegisterWindowMessageW(w!("TaskbarCreated")),
                None,
                None,
            );
            let mut msg = MSG::default();
            assert!(
                PeekMessageW(&mut msg, Some(hwnd), WM_APP + 2, WM_APP + 2, PM_REMOVE).as_bool()
            );
            DestroyWindow(hwnd).unwrap();
            UnregisterClassW(class.lpszClassName, Some(instance.into())).unwrap();
        }
    }

    #[test]
    fn desktop_shortcut_preserves_launch_arguments_and_icon() {
        let _apartment = initialize().unwrap();
        let destination =
            std::env::temp_dir().join(format!("lanprint-shortcut-{}.lnk", uuid::Uuid::new_v4()));
        let startup =
            crate::startup::Startup::new(Path::new("C:\\打印 站\\"), Some(17880), true).unwrap();
        save_shortcut(&startup, &destination).unwrap();
        unsafe {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            let file: IPersistFile = link.cast().unwrap();
            file.Load(
                PCWSTR(wide(&destination.to_string_lossy()).as_ptr()),
                STGM_READ,
            )
            .unwrap();
            let mut args = [0u16; 2048];
            link.GetArguments(&mut args).unwrap();
            let len = args.iter().position(|c| *c == 0).unwrap();
            assert_eq!(
                String::from_utf16_lossy(&args[..len]),
                startup.launch_arguments
            );
            assert!(!startup.launch_arguments.contains("--background"));
            let mut path = [0u16; 2048];
            let mut index = -1;
            link.GetIconLocation(&mut path, &mut index).unwrap();
            assert_eq!(index, 0);
            let len = path.iter().position(|c| *c == 0).unwrap();
            assert_eq!(
                String::from_utf16_lossy(&path[..len]),
                automation_path(&std::env::current_exe().unwrap().to_string_lossy())
            );
        }
        std::fs::remove_file(destination).unwrap();
    }

    #[test]
    fn generated_scan_pdf_opens_and_renders_with_windows() {
        const CHILD: &str = "LANPRINT_NATIVE_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "platform::native::tests::generated_scan_pdf_opens_and_renders_with_windows",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .status()
                .unwrap();
            assert!(
                status.success(),
                "native worker did not exit successfully: {status}"
            );
            return;
        }
        native_pdf_roundtrip();
        finish_worker(0);
    }
    fn native_pdf_roundtrip() {
        let _apartment = initialize().unwrap();
        let dir = std::env::temp_dir().join(format!("lanprint-pdf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(dir).unwrap();
        let pdf = dir.join("scan.pdf");
        let jpg = dir.join("preview.jpg");
        save_scan(
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                200,
                300,
                image::Rgb([30, 90, 60]),
            )),
            &pdf.to_string_lossy(),
            &ScanOptions::default(),
        )
        .unwrap();
        assert_eq!(document_pages(&pdf.to_string_lossy()).unwrap(), 1);
        preview(&pdf.to_string_lossy(), 0, &jpg.to_string_lossy()).unwrap();
        assert!(load_image(&jpg.to_string_lossy()).unwrap().width() > 0);
        assert!(render(&pdf.to_string_lossy(), 1, 100).is_err());
        if std::env::var_os("LANPRINT_TEST_VIRTUAL_PRINT").is_some() {
            let out = dir.join("printed.pdf");
            let options = PrintOptions {
                printer: "Microsoft Print to PDF".into(),
                copies: 2,
                landscape: true,
                ..Default::default()
            };
            print_with_profile(
                &pdf.to_string_lossy(),
                &options,
                Some(&automation_path(&out.to_string_lossy())),
                None,
            )
            .unwrap();
            let mut pages = None;
            for _ in 0..100 {
                pages = document_pages(&out.to_string_lossy()).ok();
                if pages.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            assert_eq!(pages, Some(2), "virtual printer should receive both copies");
            let mut profile = read_driver_profile("Microsoft Print to PDF", false).unwrap();
            let mut mode =
                unsafe { std::ptr::read_unaligned(profile.devmode.as_ptr().cast::<DEVMODEW>()) };
            mode.dmFields |= DM_COLOR | DM_COPIES;
            mode.dmColor = DEVMODE_COLOR(2);
            mode.Anonymous1.Anonymous1.dmCopies = 3;
            unsafe {
                std::ptr::write_unaligned(profile.devmode.as_mut_ptr().cast(), mode);
            }
            let profile_output = dir.join("profile.pdf");
            let options = PrintOptions {
                printer: "Microsoft Print to PDF".into(),
                profile_id: profile.id.clone(),
                color: false,
                copies: 2,
                render_dpi: 600,
                ..Default::default()
            };
            print_with_profile(
                &pdf.to_string_lossy(),
                &options,
                Some(&automation_path(&profile_output.to_string_lossy())),
                Some(&profile),
            )
            .unwrap();
            let mut pages = None;
            for _ in 0..100 {
                pages = document_pages(&profile_output.to_string_lossy()).ok();
                if pages.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            assert_eq!(
                pages,
                Some(2),
                "Driver copies must not multiply requested copies"
            );
            let rendered = render(&profile_output.to_string_lossy(), 0, 400)
                .unwrap()
                .to_rgb8();
            let pixel = rendered.get_pixel(rendered.width() / 2, rendered.height() / 2);
            assert!(
                pixel[1].abs_diff(pixel[0]) > 10,
                "Color preset must override the generic grayscale option"
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
