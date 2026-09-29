#[cfg(windows)]
#[path = "windows.rs"]
mod native;
#[cfg(windows)]
pub use native::*;

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[path = "unix.rs"]
mod native;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub use native::*;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
compile_error!("LanPrint supports Windows, macOS and Linux hosts.");

pub fn lan_ip() -> String {
    (|| {
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.connect("192.0.2.1:80").ok()?;
        Some(socket.local_addr().ok()?.ip().to_string())
    })()
    .unwrap_or_else(|| "127.0.0.1".into())
}

pub fn data_directory(demo: bool) -> crate::model::AppResult<std::path::PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .ok_or("无法读取 LOCALAPPDATA，请使用 --data-dir 指定数据目录。")?;
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or("无法读取 HOME，请使用 --data-dir 指定数据目录。")?
        .join("Library/Application Support");
    #[cfg(target_os = "linux")]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|p| std::path::PathBuf::from(p).join(".local/share"))
        })
        .ok_or("无法读取用户数据目录，请使用 --data-dir 指定数据目录。")?;
    Ok(base.join(if demo { "LanPrint-demo" } else { "LanPrint" }))
}
