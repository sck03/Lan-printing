use serde::{Deserialize, Serialize};

pub type AppResult<T> = Result<T, String>;

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub port: u16,
    pub retention_minutes: u64,
    pub max_upload_mb: u64,
    pub max_storage_mb: u64,
    pub max_pending: usize,
    pub show_virtual_printers: bool,
    pub secret: String,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            port: 17860,
            retention_minutes: 30,
            max_upload_mb: 50,
            max_storage_mb: 1024,
            max_pending: 50,
            show_virtual_printers: false,
            secret: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
        }
    }
}
impl Config {
    pub fn validate(&self) -> AppResult<()> {
        if self.port < 1024
            || !(5..=1440).contains(&self.retention_minutes)
            || !(1..=200).contains(&self.max_upload_mb)
            || !(200..=10240).contains(&self.max_storage_mb)
            || !(1..=200).contains(&self.max_pending)
            || self.secret.len() < 32
        {
            return Err("配置超出范围：端口 ≥1024，保留 5–1440 分钟，单文件 1–200 MB，总空间 200–10240 MB，队列 1–200。".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Printer {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub duplex: bool,
    pub color: bool,
    pub papers: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scanner {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Devices {
    pub printers: Vec<Printer>,
    pub scanners: Vec<Scanner>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrintOptions {
    pub printer: String,
    pub copies: u16,
    pub duplex: String,
    pub paper: String,
    pub landscape: bool,
    pub color: bool,
    pub pages: String,
    pub profile_id: String,
    pub render_dpi: u32,
}
impl Default for PrintOptions {
    fn default() -> Self {
        Self {
            printer: String::new(),
            copies: 1,
            duplex: "simplex".into(),
            paper: "A4".into(),
            landscape: false,
            color: false,
            pages: String::new(),
            profile_id: String::new(),
            render_dpi: 300,
        }
    }
}
impl PrintOptions {
    pub fn validate(&self, devices: &Devices, pages: u32) -> AppResult<()> {
        let p = devices
            .printers
            .iter()
            .find(|p| p.id == self.printer)
            .ok_or("打印机不存在，请刷新设备列表。")?;
        if !(1..=99).contains(&self.copies) {
            return Err("份数应为 1–99。".into());
        }
        if ![300, 600].contains(&self.render_dpi) {
            return Err("打印清晰度应为 300 或 600 DPI。".into());
        }
        if !self.profile_id.is_empty() {
            uuid::Uuid::parse_str(&self.profile_id).map_err(|_| "无效的驱动预设。")?;
            page_range(&self.pages, pages)?;
            return Ok(());
        }
        if !["simplex", "long", "short"].contains(&self.duplex.as_str()) {
            return Err("无效的双面设置。".into());
        }
        if self.duplex != "simplex" && !p.duplex {
            return Err("此打印机不支持自动双面。".into());
        }
        if self.color && !p.color {
            return Err("此打印机不支持彩色。".into());
        }
        if !p.papers.contains(&self.paper) {
            return Err("打印机不支持所选纸张。".into());
        }
        page_range(&self.pages, pages)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScanOptions {
    pub scanner: String,
    pub dpi: u32,
    pub color: bool,
    pub format: String,
    pub source: String,
}
impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            scanner: String::new(),
            dpi: 200,
            color: true,
            format: "pdf".into(),
            source: "flatbed".into(),
        }
    }
}
impl ScanOptions {
    pub fn validate(&self, devices: &Devices) -> AppResult<()> {
        if !devices.scanners.iter().any(|s| s.id == self.scanner) {
            return Err("扫描仪不存在，请刷新设备列表。".into());
        }
        if ![100, 200, 300].contains(&self.dpi)
            || !["png", "jpg", "pdf"].contains(&self.format.as_str())
            || !["flatbed", "feeder"].contains(&self.source.as_str())
        {
            return Err("无效的扫描设置。".into());
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredFile {
    pub id: String,
    #[serde(default)]
    pub owner: String,
    pub name: String,
    pub extension: String,
    pub bytes: u64,
    pub pages: u32,
    pub created_at: u64,
    pub expires_at: u64,
    pub scanned: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub owner: String,
    pub request_id: String,
    pub kind: String,
    pub name: String,
    pub file_id: Option<String>,
    pub print: Option<PrintOptions>,
    pub scan: Option<ScanOptions>,
    pub status: String,
    pub message: String,
    pub created_at: u64,
    pub finished_at: Option<u64>,
}
impl Job {
    pub fn active(&self) -> bool {
        self.status == "queued" || self.status == "running"
    }
}

pub fn page_range(value: &str, total: u32) -> AppResult<Vec<u32>> {
    if total == 0 || total > 1000 {
        return Err("文档须包含 1–1000 页。".into());
    }
    if value.trim().is_empty() {
        return Ok((0..total).collect());
    }
    if value.len() > 200 {
        return Err("页码范围过长。".into());
    }
    let mut result = std::collections::BTreeSet::new();
    for part in value.split(',') {
        let nums: Vec<&str> = part.trim().split('-').collect();
        let a: u32 = nums[0]
            .trim()
            .parse()
            .map_err(|_| "页码格式错误，例如 1-3,5。")?;
        let b = match nums.len() {
            1 => a,
            2 => nums[1]
                .trim()
                .parse()
                .map_err(|_| "页码格式错误，例如 1-3,5。")?,
            _ => return Err("页码格式错误，例如 1-3,5。".into()),
        };
        if a == 0 || a > b || b > total {
            return Err(format!("页码超出范围：文档共 {total} 页。"));
        }
        result.extend(a - 1..b);
    }
    Ok(result.into_iter().collect())
}

pub fn extension(name: &str) -> AppResult<String> {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    if !["pdf", "png", "jpg", "jpeg", "bmp"].contains(&ext.as_str()) && !office_extension(&ext) {
        return Err(
            "支持 PDF、图片、DOC/DOCX、XLS/XLSX、PPT/PPTX、WPS/ET/DPS；不支持宏专用格式。".into(),
        );
    }
    Ok(ext)
}

pub fn office_extension(ext: &str) -> bool {
    [
        "doc", "docx", "xls", "xlsx", "ppt", "pptx", "wps", "et", "dps",
    ]
    .contains(&ext)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_print_options_use_default_profile_and_resolution() {
        let options: PrintOptions =
            serde_json::from_str(r#"{"printer":"EPSON L300","copies":2,"color":true}"#).unwrap();
        assert!(options.profile_id.is_empty());
        assert_eq!(options.render_dpi, 300);
        assert_eq!(options.copies, 2);
        assert!(options.color);
    }
    #[test]
    fn page_ranges_are_strict_and_deduplicated() {
        assert_eq!(page_range("1-3,2,5", 5).unwrap(), vec![0, 1, 2, 4]);
        assert_eq!(page_range("", 2).unwrap(), vec![0, 1]);
        for range in ["0", "3-1", "1,", "1-6", "foo", "-2", "1-2-3"] {
            assert!(page_range(range, 5).is_err(), "{range}");
        }
    }
    #[test]
    fn only_supported_extensions_are_accepted() {
        assert_eq!(extension("报告.PDF").unwrap(), "pdf");
        assert!(extension("file.pdf.exe").is_err());
        for ext in [
            "doc", "docx", "xls", "xlsx", "ppt", "pptx", "wps", "et", "dps",
        ] {
            assert_eq!(extension(&format!("报告.{ext}")).unwrap(), ext);
        }
        for ext in ["docm", "xlsm", "pptm", "exe", "zip"] {
            assert!(extension(&format!("file.{ext}")).is_err());
        }
    }
}
