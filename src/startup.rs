//! Per-user logon registration. No elevation, service, or GPU-specific settings.
use crate::model::AppResult;
use sha2::{Digest, Sha256};
use std::path::Path;
use windows::{
    Win32::{Foundation::ERROR_FILE_NOT_FOUND, System::Registry::*},
    core::{PCWSTR, w},
};

const RUN: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

// Windows argv quoting, including a trailing backslash on a drive root.
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in s.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        out.extend(std::iter::repeat_n(
            '\\',
            if c == '"' { slashes * 2 + 1 } else { slashes },
        ));
        out.push(c);
        slashes = 0;
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

pub struct Startup {
    name: String,
    command: String,
    pub launch_arguments: String,
}
impl Startup {
    pub fn new(root: &Path, port_override: Option<u16>, demo: bool) -> AppResult<Self> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let path = root.to_string_lossy();
        let identity = hex::encode(Sha256::digest(path.to_lowercase().as_bytes()));
        let mut launch_arguments = format!("--data-dir {}", quote(&path));
        if let Some(port) = port_override {
            launch_arguments.push_str(&format!(" --port {port}"));
        }
        if demo {
            launch_arguments.push_str(" --demo");
        }
        let command = format!(
            "{} --background {launch_arguments}",
            quote(&exe.to_string_lossy())
        );
        Ok(Self {
            name: format!("LanPrint-{}", &identity[..16]),
            command,
            launch_arguments,
        })
    }
    pub fn enabled(&self) -> AppResult<bool> {
        self.read(RUN).map(|v| v.is_some())
    }
    pub fn set_enabled(&self, enabled: bool) -> AppResult<()> {
        self.write(RUN, enabled)
    }
    fn read(&self, key: PCWSTR) -> AppResult<Option<String>> {
        let name = wide(&self.name);
        let mut data = vec![0u16; 32768];
        let mut bytes = (data.len() * 2) as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key,
                PCWSTR(name.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(data.as_mut_ptr().cast()),
                Some(&mut bytes),
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        status.ok().map_err(|e| format!("读取启动设置失败：{e}"))?;
        let len = (bytes as usize / 2).saturating_sub(1);
        Ok(Some(String::from_utf16_lossy(&data[..len])))
    }
    fn write(&self, key: PCWSTR, enabled: bool) -> AppResult<()> {
        if enabled && self.command.encode_utf16().count() > 260 {
            return Err("启动路径过长，请将程序和数据目录放到较短的固定路径。".into());
        }
        let mut handle = HKEY::default();
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key,
                None,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut handle,
                None,
            )
            .ok()
            .map_err(|e| format!("打开启动设置失败：{e}"))?;
            let name = wide(&self.name);
            let status = if enabled {
                let bytes: Vec<u8> = wide(&self.command)
                    .iter()
                    .flat_map(|c| c.to_le_bytes())
                    .collect();
                RegSetValueExW(handle, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&bytes))
            } else {
                RegDeleteValueW(handle, PCWSTR(name.as_ptr()))
            };
            let _ = RegCloseKey(handle);
            if !enabled && status == ERROR_FILE_NOT_FOUND {
                return Ok(());
            }
            status.ok().map_err(|e| format!("保存启动设置失败：{e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_registration_roundtrips_without_registering_logon() {
        let path = wide(&format!("Software\\LanPrint-Test-{}", uuid::Uuid::new_v4()));
        let key = PCWSTR(path.as_ptr());
        let startup = Startup::new(Path::new("C:\\打印 站\\"), Some(17880), true).unwrap();
        assert!(startup.command.contains("--background"));
        assert!(startup.command.ends_with("--port 17880 --demo"));
        assert_eq!(quote("C:\\打印 站\\"), "\"C:\\打印 站\\\\\"");
        assert!(startup.read(key).unwrap().is_none());
        startup.write(key, true).unwrap();
        assert_eq!(startup.read(key).unwrap(), Some(startup.command.clone()));
        startup.write(key, false).unwrap();
        assert!(startup.read(key).unwrap().is_none());
        startup.write(key, false).unwrap();
        unsafe {
            RegDeleteKeyW(HKEY_CURRENT_USER, key).ok().unwrap();
        }
    }
}
