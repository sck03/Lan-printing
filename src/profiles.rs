use crate::{model::*, store::atomic_json};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct DriverProfile {
    pub id: String,
    pub name: String,
    pub printer: String,
    pub driver: String,
    pub devmode: Vec<u8>,
}

fn path(root: &Path, id: &str) -> AppResult<PathBuf> {
    uuid::Uuid::parse_str(id).map_err(|_| "无效的驱动预设编号。")?;
    Ok(root.join("profiles").join(format!("{id}.json")))
}
pub fn load(root: &Path, id: &str, printer: &str) -> AppResult<DriverProfile> {
    let profile: DriverProfile = serde_json::from_slice(
        &std::fs::read(path(root, id)?).map_err(|_| "驱动预设不存在，请重新选择。")?,
    )
    .map_err(|_| "驱动预设已损坏。")?;
    if profile.id != id || profile.printer != printer {
        return Err("驱动预设与所选打印机不匹配。".into());
    }
    Ok(profile)
}
pub fn list(root: &Path) -> AppResult<Vec<serde_json::Value>> {
    let directory = root.join("profiles");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        if let Ok(profile) = serde_json::from_slice::<DriverProfile>(
            &std::fs::read(entry.path()).map_err(|e| e.to_string())?,
        ) {
            result.push(
                serde_json::json!({"id":profile.id,"name":profile.name,"printer":profile.printer}),
            );
        }
    }
    result.sort_by_key(|v| v["name"].as_str().unwrap_or_default().to_owned());
    Ok(result)
}
pub fn save(root: &Path, profile: &DriverProfile) -> AppResult<()> {
    std::fs::create_dir_all(root.join("profiles")).map_err(|e| e.to_string())?;
    atomic_json(&path(root, &profile.id)?, profile)
}
pub fn remove(root: &Path, id: &str) -> AppResult<()> {
    std::fs::remove_file(path(root, id)?).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presets_are_bound_to_printer_and_cannot_escape_directory() {
        let root = std::env::temp_dir().join(format!("lanprint-profiles-{}", uuid::Uuid::new_v4()));
        let profile = DriverProfile {
            id: uuid::Uuid::new_v4().to_string(),
            name: "照片纸 · 高质量 · 关闭高速".into(),
            printer: "EPSON L300".into(),
            driver: "test".into(),
            devmode: vec![1, 2, 3],
        };
        save(&root, &profile).unwrap();
        assert_eq!(
            load(&root, &profile.id, "EPSON L300").unwrap().devmode,
            [1, 2, 3]
        );
        assert!(load(&root, &profile.id, "Another printer").is_err());
        assert!(remove(&root, "../config").is_err());
        assert_eq!(list(&root).unwrap().len(), 1);
        remove(&root, &profile.id).unwrap();
        std::fs::remove_dir(root.join("profiles")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
