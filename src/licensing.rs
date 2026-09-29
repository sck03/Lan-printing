use crate::{model::AppResult, store::atomic_json};
use lan_print::license_format::{License, MAX_CODE_BYTES, machine_code, verify_code};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path};
#[cfg(windows)]
use windows::{Win32::System::Registry::*, core::w};

#[cfg(windows)]
pub fn current_machine() -> AppResult<String> {
    let mut data = [0u16; 256];
    let mut bytes = (data.len() * 2) as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Cryptography"),
            w!("MachineGuid"),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    result
        .ok()
        .map_err(|e| format!("无法读取本机标识，请检查 Windows 注册表读取权限：{e}"))?;
    let length = data.iter().position(|v| *v == 0).unwrap_or(data.len());
    let value = String::from_utf16(&data[..length]).map_err(|_| "本机标识编码错误。")?;
    let guid = uuid::Uuid::parse_str(value.trim()).map_err(|_| "Windows 本机标识格式错误。")?;
    Ok(machine_code(&guid.to_string()))
}

#[cfg(target_os = "linux")]
pub fn current_machine() -> AppResult<String> {
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(value) = std::fs::read_to_string(path) {
            let value = value.trim();
            if value.len() == 32
                && value.bytes().all(|b| b.is_ascii_hexdigit())
                && value.bytes().any(|b| b != b'0')
            {
                return Ok(machine_code(&format!("linux:{value}")));
            }
        }
    }
    Err("无法读取 Linux machine-id，请让管理员初始化 /etc/machine-id 后重试。".into())
}

#[cfg(target_os = "macos")]
pub fn current_machine() -> AppResult<String> {
    let output = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .map_err(|e| format!("无法读取 macOS 本机标识：{e}"))?;
    if !output.status.success() {
        return Err("ioreg 无法读取本机标识。".into());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let value = text
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "\"IOPlatformUUID\"").then(|| value.trim().trim_matches('"'))
        })
        .ok_or("未找到 macOS IOPlatformUUID。")?;
    let guid = uuid::Uuid::parse_str(value).map_err(|_| "macOS 本机标识格式错误。")?;
    Ok(machine_code(&format!("macos:{guid}")))
}

pub fn verify(code: &str, machine: &str) -> AppResult<License> {
    let public: [u8; 32] = hex::decode(include_str!("../assets/license-public-key.hex").trim())
        .map_err(|_| "授权公钥格式错误，请重新安装程序。")?
        .try_into()
        .map_err(|_| "授权公钥长度错误，请重新安装程序。")?;
    verify_code(code, machine, &public)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedLicense {
    code: String,
}

pub fn saved_code(root: &Path) -> AppResult<String> {
    let file = std::fs::File::open(root.join("license.json")).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "尚未注册，请在打印主机输入注册码。".to_string()
        } else {
            format!("无法读取授权文件：{e}")
        }
    })?;
    let mut bytes = Vec::new();
    file.take((MAX_CODE_BYTES + 1025) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_CODE_BYTES + 1024 {
        return Err("授权文件过大，请重新注册。".into());
    }
    let saved: SavedLicense =
        serde_json::from_slice(&bytes).map_err(|_| "授权文件损坏，请重新输入注册码。")?;
    Ok(saved.code)
}

pub fn load(root: &Path, machine: &str) -> AppResult<License> {
    verify(&saved_code(root)?, machine)
}

pub fn activate(root: &Path, machine: &str, code: &str) -> AppResult<License> {
    // Verify before replacing: an invalid submission must preserve an existing license.
    let license = verify(code, machine)?;
    let code = code.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    atomic_json(&root.join("license.json"), &SavedLicense { code })?;
    Ok(license)
}
