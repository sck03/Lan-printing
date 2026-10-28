using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;

namespace LanPrint.Legacy
{
    internal sealed class Config
    {
        public int port = 17860;
        public int retention_minutes = 30;
        public int max_upload_mb = 100;
        public int max_storage_mb = 512;
        public int max_pending = 20;
        public bool show_virtual_printers = false;
        public string secret = Security.RandomHex(32);

        internal void Validate()
        {
            if (port < 1024 || port > 65535 || retention_minutes < 5 || retention_minutes > 1440
                || max_upload_mb < 1 || max_upload_mb > 200 || max_storage_mb < 200 || max_storage_mb > 10240
                || max_pending < 1 || max_pending > 50 || secret == null || secret.Length < 32 || secret.Length > 256)
                throw new InvalidDataException("配置超出范围：端口 1024–65535，保留 5–1440 分钟，单文件 1–200 MB，总空间 200–10240 MB，队列 1–50。");
        }
    }

    internal sealed class PrintOptions
    {
        public string printer = "";
        public int copies = 1;
        public string duplex = "simplex";
        public string paper = "A4";
        public bool landscape = false;
        public bool color = false;
        public string pages = "";
        public string profile_id = "";
        public int render_dpi = 300;

        internal void Validate(Devices devices, int total)
        {
            Printer device = devices.printers.Find(p => p.id == printer);
            if (device == null) throw new InvalidDataException("打印机不存在，请刷新设备列表。");
            if (copies < 1 || copies > 99 || (render_dpi != 300 && render_dpi != 600))
                throw new InvalidDataException("份数须为 1–99；图像处理清晰度须为 300 或 600 DPI。");
            if (!String.IsNullOrEmpty(profile_id)) { Store.RequireId(profile_id); Ranges(pages, total); return; }
            if (duplex != "simplex" && duplex != "long" && duplex != "short")
                throw new InvalidDataException("无效的双面设置。");
            if (duplex != "simplex" && !device.duplex) throw new InvalidDataException("此打印机不支持自动双面。");
            if (color && !device.color) throw new InvalidDataException("此打印机不支持彩色。");
            if (!device.papers.Contains(paper)) throw new InvalidDataException("打印机不支持所选纸张。");
            Ranges(pages, total);
        }

        internal static int[] Ranges(string value, int total)
        {
            if (total < 1 || total > 1000) throw new InvalidDataException("文档须包含 1–1000 页。");
            if (String.IsNullOrWhiteSpace(value)) return Enumerable.Range(0, total).ToArray();
            if (value.Length > 200) throw new InvalidDataException("页码范围过长。");
            var result = new SortedSet<int>();
            foreach (string part in value.Split(','))
            {
                string[] range = part.Trim().Split('-');
                int first, last;
                if (range.Length > 2 || !Int32.TryParse(range[0].Trim(), out first))
                    throw new InvalidDataException("页码格式错误，例如 1-3,5。");
                last = first;
                if (range.Length == 2 && !Int32.TryParse(range[1].Trim(), out last))
                    throw new InvalidDataException("页码格式错误，例如 1-3,5。");
                if (first < 1 || last < first || last > total) throw new InvalidDataException("页码超出文档范围。");
                for (int n = first - 1; n < last; n++) result.Add(n);
            }
            return result.ToArray();
        }
    }

    internal sealed class ScanOptions
    {
        public string scanner = "";
        public int dpi = 200;
        public bool color = true;
        public string format = "pdf";
        public string source = "flatbed";

        internal void Validate(Devices devices)
        {
            if (!devices.scanners.Any(s => s.id == scanner)) throw new InvalidDataException("扫描仪不存在，请刷新设备列表。");
            if ((dpi != 100 && dpi != 200 && dpi != 300) || (format != "pdf" && format != "jpg" && format != "png")
                || (source != "flatbed" && source != "feeder")) throw new InvalidDataException("无效的扫描设置。");
        }
    }

    internal sealed class FileRecord
    {
        public string id, owner, name, extension;
        public long bytes, created_at, expires_at;
        public int pages;
        public bool scanned;

        internal object Public()
        {
            return new { id, name, extension, bytes, pages, createdAt = created_at, expiresAt = expires_at, scanned };
        }
    }

    internal sealed class Job
    {
        public string id, owner, request_id, kind, name, file_id, status, message;
        public PrintOptions print;
        public ScanOptions scan;
        public long created_at;
        public long? finished_at;

        internal bool Active { get { return status == "queued" || status == "running"; } }
        internal object Public()
        {
            return new { id, kind, name, fileId = file_id, status, message, createdAt = created_at, finishedAt = finished_at };
        }
    }

    internal sealed class Database
    {
        public int schema = 1;
        public List<FileRecord> files = new List<FileRecord>();
        public List<Job> jobs = new List<Job>();
    }

    // Every access, including the queue and cleanup thread, holds Sync.
    internal sealed class Store
    {
        internal readonly object Sync = new object();
        internal readonly string Root;
        internal readonly Database Db;
        private readonly Dictionary<string, int> leases = new Dictionary<string, int>();

        internal Store(string root)
        {
            Root = root;
            Directory.CreateDirectory(Path.Combine(root, "files"));
            Directory.CreateDirectory(Path.Combine(root, "work"));
            RejectJunction(Path.Combine(root, "files"));
            RejectJunction(Path.Combine(root, "work"));
            string state = Path.Combine(root, "state.json");
            Db = File.Exists(state) ? Json.Read<Database>(state, 8 * 1024 * 1024) : new Database();
            if (Db == null || Db.schema != 1 || Db.files == null || Db.jobs == null || Db.files.Count > 1000)
                throw new InvalidDataException("state.json 格式或版本不兼容；已保留原文件，请使用旧系统版独立数据目录。");
            var ids = new HashSet<string>();
            foreach (FileRecord f in Db.files)
            {
                if (f == null || !ValidId(f.id) || !ValidId(f.owner) || !ids.Add(f.id) || f.bytes < 1
                    || f.pages < 1 || f.pages > 1000 || String.IsNullOrEmpty(f.name) || Extension(f.name) != f.extension)
                    throw new InvalidDataException("文件状态损坏；已保留 state.json。");
            }
            ids.Clear();
            foreach (Job job in Db.jobs)
            {
                if (job == null || !ValidId(job.id) || !ValidId(job.owner) || !ValidId(job.request_id) || !ids.Add(job.id)
                    || (job.kind != "print" && job.kind != "scan") || (job.file_id != null && !ValidId(job.file_id))
                    || !new[] { "queued", "running", "submitted", "completed", "simulated", "failed", "cancelled", "interrupted" }.Contains(job.status))
                    throw new InvalidDataException("任务状态损坏；已保留 state.json。");
                if (job.Active)
                {
                    job.status = "interrupted";
                    job.message = "服务重启，任务未自动重试。请先检查打印机和系统队列，避免重复打印。";
                    job.finished_at = Now();
                }
            }
            // Persist interruption before deleting any crash residue.
            Save();
            Cleanup();
            foreach (string directory in Directory.GetDirectories(Path.Combine(root, "work")))
                if (ValidId(Path.GetFileName(directory)) && (File.GetAttributes(directory) & FileAttributes.ReparsePoint) == 0)
                    Json.DeleteScratch(directory);
            foreach (string path in Directory.GetFiles(Path.Combine(root, "files")))
                if (ValidId(Path.GetFileNameWithoutExtension(path)) && !Db.files.Any(f => FilePath(f) == path))
                    Json.TryDelete(path);
        }

        private static void RejectJunction(string path)
        {
            if ((File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0)
                throw new InvalidDataException("数据子目录不能是链接或目录联接。");
        }

        internal static long Now() { return (long)(DateTime.UtcNow - new DateTime(1970, 1, 1)).TotalSeconds; }
        internal static bool ValidId(string id)
        {
            Guid guid;
            return id != null && Guid.TryParse(id, out guid) && guid.ToString("D") == id;
        }
        internal static void RequireId(string id)
        {
            if (!ValidId(id)) throw new InvalidDataException("无效的请求或文件编号。");
        }
        internal static string Extension(string name)
        {
            string ext = Path.GetExtension(name).TrimStart('.').ToLowerInvariant();
            if (!new[] { "pdf", "png", "jpg", "jpeg", "bmp" }.Contains(ext))
                throw new InvalidDataException("旧系统版支持 PDF、PNG、JPEG 和 BMP；办公文档请先导出 PDF。");
            return ext;
        }
        internal void Save() { Json.Write(Path.Combine(Root, "state.json"), Db); }
        internal string FilePath(FileRecord file) { return Path.Combine(Root, "files", file.id + "." + file.extension); }
        internal bool Pinned(string id) { return leases.ContainsKey(id) || Db.jobs.Any(j => j.Active && j.file_id == id); }
        internal FileRecord Owned(string id, string owner)
        {
            FileRecord file = Db.files.Find(f => f.id == id && f.owner == owner && (f.expires_at > Now() || Pinned(id)));
            if (file == null) throw new InvalidDataException("文件不存在或已过期。");
            return file;
        }
        internal IDisposable Lease(string id, string owner, int retention)
        {
            FileRecord file = Owned(id, owner);
            long old = file.expires_at;
            file.expires_at = Now() + retention * 60L;
            try { Save(); } catch { file.expires_at = old; throw; }
            leases[id] = leases.ContainsKey(id) ? leases[id] + 1 : 1;
            return new FileLease(this, id);
        }
        private sealed class FileLease : IDisposable
        {
            private readonly Store store;
            private string id;
            internal FileLease(Store store, string id) { this.store = store; this.id = id; }
            public void Dispose()
            {
                lock (store.Sync)
                {
                    if (id == null) return;
                    if (--store.leases[id] == 0) store.leases.Remove(id);
                    id = null;
                }
            }
        }
        internal void Capacity(long bytes, int maximum)
        {
            if (Db.files.Count >= 1000 || bytes > maximum * 1024L * 1024 - Db.files.Sum(f => f.bytes))
                throw new InvalidDataException("暂存空间或文件数量已达上限，请删除无用文件或等待清理。");
        }
        internal void Insert(FileRecord file, int maximum)
        {
            Capacity(file.bytes, maximum);
            Db.files.Add(file);
            try { Save(); } catch { Db.files.Remove(file); throw; }
        }
        internal Job Existing(string owner, string request, string kind)
        {
            Job job = Db.jobs.Find(j => j.owner == owner && j.request_id == request);
            if (job != null && job.kind != kind) throw new InvalidDataException("请求标识已用于其他类型的任务。");
            return job;
        }
        internal Job Enqueue(Job job, int maximum)
        {
            Job existing = Existing(job.owner, job.request_id, job.kind);
            if (existing != null) return existing;
            if (Db.jobs.Count(j => j.Active) >= maximum) throw new InvalidDataException("队列已满，请稍后重试。");
            Db.jobs.Add(job);
            try { Save(); } catch { Db.jobs.Remove(job); throw; }
            return job;
        }
        internal void Cancel(string id, string owner)
        {
            Job job = Db.jobs.Find(j => j.id == id && j.owner == owner);
            if (job == null || job.status != "queued") throw new InvalidDataException("只能取消自己的排队任务；已提交打印请在主机系统队列取消。");
            job.status = "cancelled";
            job.message = "已取消";
            job.finished_at = Now();
            try { Save(); }
            catch { job.status = "queued"; job.message = "等待处理"; job.finished_at = null; throw; }
        }
        internal void Remove(string id, string owner)
        {
            FileRecord file = Owned(id, owner);
            if (Pinned(id)) throw new InvalidDataException("文件仍在使用，暂时不能删除。");
            // Save first: failed persistence must never destroy the user's file.
            Db.files.Remove(file);
            try { Save(); } catch { Db.files.Add(file); throw; }
            Json.TryDelete(FilePath(file));
        }
        internal void Cleanup()
        {
            long now = Now();
            FileRecord[] files = Db.files.Where(f => f.expires_at <= now && !Pinned(f.id)).ToArray();
            Job[] jobs = Db.jobs.Where(j => !j.Active && now - (j.finished_at ?? j.created_at) >= 86400).ToArray();
            foreach (FileRecord file in files) Db.files.Remove(file);
            foreach (Job job in jobs) Db.jobs.Remove(job);
            try { Save(); }
            catch { Db.files.AddRange(files); Db.jobs.AddRange(jobs); throw; }
            foreach (FileRecord file in files) Json.TryDelete(FilePath(file));
        }
    }
}
