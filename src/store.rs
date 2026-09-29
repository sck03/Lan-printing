use crate::model::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Default, Serialize, Deserialize)]
pub struct Database {
    pub files: HashMap<String, StoredFile>,
    pub jobs: Vec<Job>,
}

pub struct Store {
    pub root: PathBuf,
    pub db: Database,
}

// Also runs when a client disconnects and the request future is cancelled.
pub struct PendingFile(pub PathBuf);
impl PendingFile {
    pub fn persist(mut self) {
        self.0.clear();
    }
}
impl Drop for PendingFile {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}

pub struct PendingDirectory(pub PathBuf);
impl Drop for PendingDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn atomic_json(path: &Path, data: &impl Serialize) -> AppResult<()> {
    use std::io::Write;
    let temp = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(data).map_err(|e| e.to_string())?;
    let mut f = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
    f.write_all(&bytes)
        .and_then(|_| f.sync_all())
        .map_err(|e| e.to_string())?;
    drop(f);
    // MoveFileExW replaces atomically on Windows; std::fs::rename does not replace there.
    crate::platform::replace_file(&temp, path)
}

impl Store {
    pub fn open(root: PathBuf) -> AppResult<Self> {
        std::fs::create_dir_all(root.join("files")).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(root.join("work")).map_err(|e| e.to_string())?;
        let db_path = root.join("state.json");
        let db: Database = if db_path.exists() {
            serde_json::from_slice(&std::fs::read(&db_path).map_err(|e| e.to_string())?)
                .map_err(|e| format!("state.json 无法读取，保留原文件以便恢复：{e}"))?
        } else {
            Database::default()
        };
        let mut s = Self { root, db };
        for job in &mut s.db.jobs {
            if job.active() {
                job.status = "interrupted".into();
                job.message =
                    "服务重启，任务未自动重试。若已送入打印机，请先检查出纸情况，避免重复打印。"
                        .into();
                job.finished_at = Some(now());
            }
        }
        s.cleanup()?;
        // Only our UUID-named scratch directories; no external or arbitrary paths.
        for e in std::fs::read_dir(s.root.join("work"))
            .map_err(|e| e.to_string())?
            .flatten()
        {
            if e.file_type().is_ok_and(|t| t.is_file())
                && e.path()
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
            {
                let _ = std::fs::remove_file(e.path());
            }
            if uuid::Uuid::parse_str(&e.file_name().to_string_lossy()).is_ok()
                && e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink())
            {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
        // Uncommitted uploads left by a crash do not accumulate on disk.
        for e in std::fs::read_dir(s.root.join("files"))
            .map_err(|e| e.to_string())?
            .flatten()
        {
            let path = e.path();
            let id = path.file_stem().and_then(|x| x.to_str()).unwrap_or("");
            if uuid::Uuid::parse_str(id).is_ok() && !s.db.files.contains_key(id) {
                let _ = std::fs::remove_file(path);
            }
        }
        Ok(s)
    }
    pub fn save(&self) -> AppResult<()> {
        atomic_json(&self.root.join("state.json"), &self.db)
    }
    pub fn path(&self, f: &StoredFile) -> PathBuf {
        self.root
            .join("files")
            .join(format!("{}.{}", f.id, f.extension))
    }
    pub fn owned(&self, id: &str, owner: &str) -> AppResult<StoredFile> {
        self.db
            .files
            .get(id)
            .filter(|f| f.owner == owner && (f.expires_at > now() || self.pinned(id)))
            .cloned()
            .ok_or("文件不存在或已过期。".into())
    }
    pub fn pinned(&self, id: &str) -> bool {
        self.db
            .jobs
            .iter()
            .any(|j| j.active() && j.file_id.as_deref() == Some(id))
    }
    pub fn existing_request(
        &self,
        owner: &str,
        request_id: &str,
        kind: &str,
    ) -> AppResult<Option<Job>> {
        let existing = self
            .db
            .jobs
            .iter()
            .find(|j| j.owner == owner && j.request_id == request_id);
        if existing.is_some_and(|j| j.kind != kind) {
            return Err("请求标识已用于其他类型的任务，请重新提交。".into());
        }
        Ok(existing.cloned())
    }
    pub fn check_capacity(&self, bytes: u64, max_storage_mb: u64) -> AppResult<()> {
        if self.db.files.len() >= 1000 {
            return Err("暂存文件数量已达上限。".into());
        }
        let used = self
            .db
            .files
            .values()
            .fold(0u64, |sum, f| sum.saturating_add(f.bytes));
        if used.saturating_add(bytes) > max_storage_mb * 1024 * 1024 {
            return Err("暂存空间不足，请删除不需要的文件或等待自动清理。".into());
        }
        Ok(())
    }
    pub fn insert_file(&mut self, file: StoredFile, max_storage_mb: u64) -> AppResult<()> {
        self.check_capacity(file.bytes, max_storage_mb)?;
        let id = file.id.clone();
        self.db.files.insert(id.clone(), file);
        if let Err(e) = self.save() {
            self.db.files.remove(&id);
            return Err(e);
        }
        Ok(())
    }
    pub fn enqueue(&mut self, job: Job, max: usize) -> AppResult<Job> {
        if let Some(old) = self.existing_request(&job.owner, &job.request_id, &job.kind)? {
            return Ok(old);
        }
        if self.db.jobs.iter().filter(|j| j.active()).count() >= max {
            return Err("队列已满，请稍后重试。".into());
        }
        self.db.jobs.push(job.clone());
        if let Err(e) = self.save() {
            self.db.jobs.pop();
            return Err(e);
        }
        Ok(job)
    }
    pub fn cancel(&mut self, id: &str, owner: &str) -> AppResult<()> {
        let j = self
            .db
            .jobs
            .iter_mut()
            .find(|j| j.id == id && j.owner == owner)
            .ok_or("任务不存在。")?;
        if j.status != "queued" {
            return Err("只能取消尚未执行的任务。已提交的打印请在主机打印队列中取消。".into());
        }
        let old = j.clone();
        j.status = "cancelled".into();
        j.message = "已取消".into();
        j.finished_at = Some(now());
        if let Err(e) = self.save() {
            if let Some(j) = self.db.jobs.iter_mut().find(|j| j.id == id) {
                *j = old;
            }
            return Err(e);
        }
        Ok(())
    }
    pub fn cleanup(&mut self) -> AppResult<()> {
        let expired: Vec<String> = self
            .db
            .files
            .values()
            .filter(|f| f.expires_at <= now() && !self.pinned(&f.id))
            .map(|f| f.id.clone())
            .collect();
        for id in expired {
            if let Some(f) = self.db.files.get(&id) {
                match std::fs::remove_file(self.path(f)) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => continue,
                }
            }
            self.db.files.remove(&id);
        }
        self.db
            .jobs
            .retain(|j| j.active() || j.finished_at.unwrap_or(j.created_at) + 86400 > now());
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn job(owner: &str, request: &str) -> Job {
        Job {
            id: uuid::Uuid::new_v4().to_string(),
            owner: owner.into(),
            request_id: request.into(),
            kind: "print".into(),
            name: "test".into(),
            file_id: None,
            print: Some(PrintOptions::default()),
            scan: None,
            status: "queued".into(),
            message: String::new(),
            created_at: now(),
            finished_at: None,
        }
    }
    #[test]
    fn queue_is_idempotent_owned_and_never_replayed_after_restart() {
        let root = std::env::temp_dir().join(format!("lanprint-test-{}", uuid::Uuid::new_v4()));
        let mut st = Store::open(root.clone()).unwrap();
        let first = st.enqueue(job("alice", "request"), 2).unwrap();
        assert_eq!(first.id, st.enqueue(job("alice", "request"), 2).unwrap().id);
        let mut conflicting = job("alice", "request");
        conflicting.kind = "scan".into();
        assert!(st.enqueue(conflicting, 2).is_err());
        assert!(st.cancel(&first.id, "bob").is_err());
        let other = st.enqueue(job("bob", "request"), 2).unwrap();
        assert_ne!(first.id, other.id);
        assert!(st.enqueue(job("alice", "overflow"), 2).is_err());
        st.cancel(&other.id, "bob").unwrap();
        drop(st);
        let st = Store::open(root.clone()).unwrap();
        assert_eq!(st.db.jobs[0].status, "interrupted");
        assert_eq!(st.db.jobs[1].status, "cancelled");
        assert!(st.db.jobs.iter().all(|j| !j.active()));
        drop(st);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn files_are_owner_scoped_and_pinned_until_job_finishes() {
        let root = std::env::temp_dir().join(format!("lanprint-test-{}", uuid::Uuid::new_v4()));
        let mut st = Store::open(root.clone()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let file = StoredFile {
            id: id.clone(),
            owner: "alice".into(),
            name: "test.png".into(),
            extension: "png".into(),
            bytes: 1,
            pages: 1,
            created_at: 0,
            expires_at: 0,
            scanned: false,
        };
        std::fs::write(st.path(&file), [0]).unwrap();
        st.db.files.insert(id.clone(), file.clone());
        let mut j = job("alice", "print");
        j.file_id = Some(id.clone());
        let j = st.enqueue(j, 2).unwrap();
        assert!(st.owned(&id, "bob").is_err());
        assert!(st.owned(&id, "alice").is_ok());
        st.cleanup().unwrap();
        assert!(st.path(&file).exists());
        st.cancel(&j.id, "alice").unwrap();
        st.cleanup().unwrap();
        assert!(!st.path(&file).exists());
        assert!(st.db.files.is_empty());
        drop(st);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn file_limits_and_failed_persistence_apply_to_scans_and_uploads() {
        let root = std::env::temp_dir().join(format!("lanprint-store-{}", uuid::Uuid::new_v4()));
        let mut st = Store::open(root.clone()).unwrap();
        let _cleanup = PendingDirectory(root.clone());
        let mut file = StoredFile {
            id: uuid::Uuid::new_v4().to_string(),
            owner: "alice".into(),
            name: "scan.pdf".into(),
            extension: "pdf".into(),
            bytes: 1024 * 1024,
            pages: 1,
            created_at: now(),
            expires_at: now() + 60,
            scanned: true,
        };
        st.insert_file(file.clone(), 1).unwrap();
        assert!(st.check_capacity(1, 1).is_err());
        st.db.files.clear();
        file.bytes = 1;
        for _ in 0..1000 {
            file.id = uuid::Uuid::new_v4().to_string();
            st.db.files.insert(file.id.clone(), file.clone());
        }
        for scanned in [true, false] {
            file.scanned = scanned;
            file.id = uuid::Uuid::new_v4().to_string();
            assert!(st.insert_file(file.clone(), 1).is_err());
            assert!(!st.db.files.contains_key(&file.id));
        }
        st.db.files.clear();
        std::fs::remove_file(root.join("state.json")).unwrap();
        std::fs::create_dir(root.join("state.json")).unwrap();
        assert!(st.insert_file(file.clone(), 1).is_err());
        assert!(
            !st.db.files.contains_key(&file.id),
            "Failed writes must roll back memory"
        );
    }

    #[test]
    fn pending_files_are_removed_unless_committed() {
        let root = std::env::temp_dir().join(format!("lanprint-pending-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let _cleanup = PendingDirectory(root.clone());
        let path = root.join("file.pdf");
        std::fs::write(&path, b"test").unwrap();
        drop(PendingFile(path.clone()));
        assert!(!path.exists());
        std::fs::write(&path, b"test").unwrap();
        PendingFile(path.clone()).persist();
        assert!(path.exists());
    }
}
