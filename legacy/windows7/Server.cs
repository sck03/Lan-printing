using System;
using System.IO;
using System.Linq;
using System.Net;
using System.Net.Sockets;
using System.Text.RegularExpressions;
using System.Threading;

namespace LanPrint.Legacy
{
    internal sealed class ActivationInput { public string code; }
    internal sealed class PrintInput { public string file_id, request_id; public PrintOptions options; }
    internal sealed class ScanInput { public string request_id; public ScanOptions options; }

    internal sealed class Server : IDisposable
    {
        internal readonly Config Config;
        internal readonly Store Store;
        internal readonly Licensing Licensing;
        internal readonly bool Demo;
        private readonly TcpListener listener;
        private readonly Semaphore connections = new Semaphore(24, 24), heavy = new Semaphore(1, 1);
        private readonly AutoResetEvent notify = new AutoResetEvent(false);
        private readonly object deviceLock = new object();
        private Devices cachedDevices;
        private long deviceTime;
        private volatile bool stopped;
        private readonly Thread queue, cleanup, accept;

        internal Server(string root, Config config, bool demo)
        {
            Config = config; Demo = demo;
            Licensing = new Licensing(root, Security.Hex(Resources.Text("license-public-key.hex").Trim()));
            Store = new Store(root);
            listener = new TcpListener(IPAddress.Any, config.port);
            listener.Start(24);
            queue = Start(Queue);
            cleanup = Start(Cleanup);
            accept = Start(Accept);
        }
        private static Thread Start(ThreadStart action)
        {
            var thread = new Thread(action) { IsBackground = true };
            thread.Start(); return thread;
        }
        internal void Wait() { accept.Join(); }
        private void Accept()
        {
            try
            {
                while (!stopped)
                {
                    TcpClient client = listener.AcceptTcpClient();
                    if (!connections.WaitOne(0)) { client.Close(); continue; }
                    ThreadPool.QueueUserWorkItem(delegate {
                        try { Handle(client); }
                        finally { connections.Release(); }
                    });
                }
            }
            catch (SocketException) { if (!stopped) throw; }
            catch (ObjectDisposedException) { if (!stopped) throw; }
        }
        private void Handle(TcpClient client)
        {
            using (var http = new Http(client))
            {
                string scratch = null;
                try
                {
                    http.ReadHeaders(); Security.Check(http, Config);
                    string path = Uri.UnescapeDataString(http.Target.Split('?')[0]);
                    bool registration = new[] { "/", "/register", "/api/session", "/api/license", "/app.css", "/activation.js" }.Contains(path);
                    if (!registration && !Licensing.Registered())
                    {
                        http.JsonResponse(new { error = "打印站尚未注册或授权无效，请在主机完成注册。", code = "license_required" }, 403); return;
                    }
                    string method = http.Method == "HEAD" ? "GET" : http.Method;
                    if (method == "GET" && path == "/api/session")
                        http.JsonResponse(new { csrf = Security.Sign(Config, "csrf:" + http.Owner), demo = Demo, maxUploadMb = Config.max_upload_mb,
                            retentionMinutes = Config.retention_minutes, officeFormats = new string[0], localAdmin = http.Local,
                            shareUrl = ShareUrl(), platform = "windows", legacy = true, renderDpiMax = 300,
                            driverProfiles = false, firewallRepair = false, shareQr = false }, 200);
                    else if (method == "GET" && path == "/api/license")
                    {
                        bool registered = Licensing.Registered();
                        http.JsonResponse(new { registered, localAdmin = http.Local, machineCode = http.Local ? Licensing.Machine : null,
                            customer = http.Local && registered ? Licensing.Load().customer : null,
                            message = !http.Local ? "请在打印主机打开本机地址完成注册。" : registered ? "已注册 · 本机永久授权" : "尚未注册或授权无效，请输入本机注册码。" }, 200);
                    }
                    else if (method == "GET" && path == "/api/devices") http.JsonResponse(GetDevices(), 200);
                    else if (method == "GET" && path == "/api/network")
                    {
                        RequireHost(http);
                        http.JsonResponse(new { port = Config.port, address = Launch.LanIp(), listening = true, ruleConfigured = false, canRepair = false,
                            state = "manual", profiles = new object[0], message = "共享端口正在监听；旧系统版请在 Windows 防火墙中手动允许本程序，并从另一台电脑验证访问。" }, 200);
                    }
                    else if (method == "GET" && path == "/api/files")
                    {
                        object[] files;
                        lock (Store.Sync) files = Store.Db.files.Where(f => f.owner == http.Owner && (f.expires_at > Legacy.Store.Now() || Store.Pinned(f.id)))
                            .OrderByDescending(f => f.created_at).ThenBy(f => f.id).Select(f => f.Public()).ToArray();
                        http.JsonResponse(files, 200);
                    }
                    else if (method == "GET" && path == "/api/jobs")
                    {
                        object[] jobs;
                        lock (Store.Sync) jobs = Store.Db.jobs.Where(j => j.owner == http.Owner).Reverse().Take(100).Select(j => j.Public()).ToArray();
                        http.JsonResponse(jobs, 200);
                    }
                    else if (method == "GET" && (path == "/" || path == "/register"))
                        http.Bytes(Resources.Bytes(path == "/register" || !Licensing.Registered() ? "activation.html" : "index.html"), "text/html; charset=utf-8", 200);
                    else if (method == "GET" && new[] { "/app.css", "/app.js", "/activation.js", "/profiles.js", "/network.js" }.Contains(path))
                        http.Bytes(Resources.Bytes(path.Substring(1)), path.EndsWith(".css") ? "text/css; charset=utf-8" : "text/javascript; charset=utf-8", 200);
                    else
                    {
                        scratch = Path.Combine(Store.Root, "work", Guid.NewGuid().ToString("D"));
                        Directory.CreateDirectory(scratch);
                        string body = Path.Combine(scratch, "body");
                        if (method == "POST" && path == "/api/license")
                        {
                            RequireHost(http);
                            LicenseClaims license = Licensing.Activate(http.JsonBody<ActivationInput>(body).code);
                            http.JsonResponse(new { registered = true, customer = license.customer }, 200);
                        }
                        else if (method == "POST" && path == "/api/files") Upload(http, body);
                        else if (method == "POST" && path == "/api/print") SubmitPrint(http, http.JsonBody<PrintInput>(body));
                        else if (method == "POST" && path == "/api/scan") SubmitScan(http, http.JsonBody<ScanInput>(body));
                        else
                        {
                            Match file = Regex.Match(path, "^/api/files/([0-9a-f-]+)(/(preview|download))?$");
                            Match job = Regex.Match(path, "^/api/jobs/([0-9a-f-]+)/cancel$");
                            if (file.Success)
                            {
                                string id = file.Groups[1].Value; Legacy.Store.RequireId(id);
                                if (method == "DELETE" && !file.Groups[3].Success)
                                {
                                    lock (Store.Sync) Store.Remove(id, http.Owner);
                                    http.JsonResponse(new { ok = true }, 200);
                                }
                                else if (method == "GET" && file.Groups[3].Success) Download(http, id, file.Groups[3].Value == "preview", scratch);
                                else throw new ApiError(405, "请求方法无效。");
                            }
                            else if (job.Success && method == "POST")
                            {
                                Legacy.Store.RequireId(job.Groups[1].Value);
                                lock (Store.Sync) Store.Cancel(job.Groups[1].Value, http.Owner);
                                http.JsonResponse(new { ok = true }, 200);
                            }
                            else throw new ApiError(404, "接口不存在。");
                        }
                    }
                }
                catch (Exception error)
                {
                    if (!http.Responded)
                        try { http.JsonResponse(new { error = error.Message }, error is ApiError ? ((ApiError)error).Status : 400); }
                        catch (IOException) { } catch (SocketException) { }
                }
                finally { if (scratch != null) Json.DeleteScratch(scratch); }
            }
        }
        private Devices GetDevices()
        {
            lock (deviceLock)
            {
                if (cachedDevices == null || Legacy.Store.Now() - deviceTime >= 30)
                {
                    cachedDevices = Worker.Call(Store.Root, Licensing, new WorkerRequest { operation = "devices", demo = Demo, show_virtual = Config.show_virtual_printers }, 25).devices;
                    if (cachedDevices == null) throw new InvalidDataException("无法读取设备列表。");
                    deviceTime = Legacy.Store.Now();
                }
                return cachedDevices;
            }
        }
        private void Upload(Http http, string body)
        {
            if (!heavy.WaitOne(0)) throw new ApiError(429, "正在处理其他文件，请稍后重试。");
            string path = null; bool committed = false;
            try
            {
                http.Body(body, Config.max_upload_mb * 1024L * 1024 + 32 * 1024);
                string id = Guid.NewGuid().ToString("D");
                string name = Multipart.Extract(body, http.Header("Content-Type"), Path.Combine(Store.Root, "files"), id, Config.max_upload_mb * 1024L * 1024);
                string ext = Legacy.Store.Extension(name);
                path = Path.Combine(Store.Root, "files", id + "." + ext);
                int pages = Worker.Call(Store.Root, Licensing, new WorkerRequest { operation = "inspect", path = path }, 30).pages;
                var file = new FileRecord { id = id, owner = http.Owner, name = name, extension = ext, bytes = new FileInfo(path).Length,
                    pages = pages, created_at = Legacy.Store.Now(), expires_at = Legacy.Store.Now() + Config.retention_minutes * 60L };
                lock (Store.Sync) Store.Insert(file, Config.max_storage_mb);
                committed = true;
                http.JsonResponse(file.Public(), 200);
            }
            finally { if (!committed && path != null) Json.TryDelete(path); heavy.Release(); }
        }
        private void Download(Http http, string id, bool preview, string scratch)
        {
            if (preview && !heavy.WaitOne(0)) throw new ApiError(429, "正在生成其他预览，请稍后重试。");
            try
            {
                IDisposable lease; FileRecord file; string path;
                lock (Store.Sync)
                {
                    lease = Store.Lease(id, http.Owner, Config.retention_minutes);
                    file = Store.Owned(id, http.Owner); path = Store.FilePath(file);
                }
                using (lease)
                {
                    if (preview)
                    {
                        MatchCollection values = Regex.Matches(http.Target, "[?&]page=([^&]*)");
                        int page = 0;
                        if (values.Count > 1 || values.Count == 1 && !Int32.TryParse(values[0].Groups[1].Value, out page)) throw new ApiError(400, "页码无效。");
                        if (page < 0 || page >= file.pages) throw new ApiError(400, "页码超出范围。");
                        string output = Path.Combine(scratch, "preview.jpg");
                        Worker.Call(Store.Root, Licensing, new WorkerRequest { operation = "preview", path = path, output = output, page = page }, 30);
                        http.FileResponse(output, "image/jpeg", null);
                    }
                    else
                    {
                        string mime = file.extension == "pdf" ? "application/pdf" : file.extension == "png" ? "image/png" : file.extension == "bmp" ? "image/bmp" : "image/jpeg";
                        string disposition = "attachment; filename=\"document." + file.extension + "\"; filename*=UTF-8''" + Uri.EscapeDataString(file.name);
                        http.FileResponse(path, mime, disposition);
                    }
                }
            }
            finally { if (preview) heavy.Release(); }
        }
        private void SubmitPrint(Http http, PrintInput input)
        {
            Legacy.Store.RequireId(input.request_id);
            Job job;
            lock (Store.Sync) job = Store.Existing(http.Owner, input.request_id, "print");
            if (job == null)
            {
                Devices devices = GetDevices();
                lock (Store.Sync)
                {
                    job = Store.Existing(http.Owner, input.request_id, "print");
                    if (job == null)
                    {
                        FileRecord file = Store.Owned(input.file_id, http.Owner);
                        if (input.options == null) throw new ApiError(400, "缺少打印参数。");
                        input.options.Validate(devices, file.pages);
                        job = Store.Enqueue(new Job { id = Guid.NewGuid().ToString("D"), owner = http.Owner, request_id = input.request_id, kind = "print",
                            name = file.name, file_id = file.id, print = input.options, status = "queued", message = "等待处理", created_at = Legacy.Store.Now() }, Config.max_pending);
                    }
                }
                notify.Set();
            }
            http.JsonResponse(job.Public(), 200);
        }
        private void SubmitScan(Http http, ScanInput input)
        {
            Legacy.Store.RequireId(input.request_id);
            Job job;
            lock (Store.Sync) job = Store.Existing(http.Owner, input.request_id, "scan");
            if (job == null)
            {
                Devices devices = GetDevices();
                lock (Store.Sync)
                {
                    job = Store.Existing(http.Owner, input.request_id, "scan");
                    if (job == null)
                    {
                        if (input.options == null) throw new ApiError(400, "缺少扫描参数。");
                        input.options.Validate(devices); Store.Capacity(50 * 1024L * 1024, Config.max_storage_mb);
                        job = Store.Enqueue(new Job { id = Guid.NewGuid().ToString("D"), owner = http.Owner, request_id = input.request_id, kind = "scan",
                            name = "扫描文档", scan = input.options, status = "queued", message = "等待扫描，请放置原稿", created_at = Legacy.Store.Now() }, Config.max_pending);
                    }
                }
                notify.Set();
            }
            http.JsonResponse(job.Public(), 200);
        }
        private void Queue()
        {
            while (!stopped)
            {
                Job job;
                lock (Store.Sync)
                {
                    job = Store.Db.jobs.Find(j => j.status == "queued");
                    if (job != null)
                    {
                        job.status = "running"; job.message = "正在处理，请勿重复提交";
                        try { Store.Save(); }
                        catch (Exception error) { job.status = "failed"; job.message = "保存状态失败，未调用设备：" + error.Message; job.finished_at = Legacy.Store.Now(); job = null; notify.Set(); }
                    }
                }
                if (job == null) { notify.WaitOne(1000); continue; }
                string status, message; FileRecord result = null;
                try
                {
                    Licensing.Load();
                    if (job.kind == "print")
                    {
                        string path;
                        lock (Store.Sync) path = Store.FilePath(Store.Owned(job.file_id, job.owner));
                        Worker.Call(Store.Root, Licensing, new WorkerRequest { operation = "print", path = path, print = job.print, demo = Demo }, 180);
                        status = Demo ? "simulated" : "submitted";
                        message = Demo ? "演示任务完成，未使用真实设备。" : "已提交系统打印队列；实际出纸以打印机为准。";
                    }
                    else
                    {
                        lock (Store.Sync) Store.Capacity(50 * 1024L * 1024, Config.max_storage_mb);
                        string id = Guid.NewGuid().ToString("D"), path = Path.Combine(Store.Root, "files", id + "." + job.scan.format);
                        bool committed = false;
                        try
                        {
                            Worker.Call(Store.Root, Licensing, new WorkerRequest { operation = "scan", output = path, scan = job.scan, demo = Demo }, 120);
                            result = new FileRecord { id = id, owner = job.owner, name = "扫描-" + Legacy.Store.Now() + "." + job.scan.format, extension = job.scan.format,
                                bytes = new FileInfo(path).Length, pages = 1, scanned = true, created_at = Legacy.Store.Now(), expires_at = Legacy.Store.Now() + Config.retention_minutes * 60L };
                            lock (Store.Sync) Store.Insert(result, Config.max_storage_mb);
                            committed = true;
                        }
                        finally { if (!committed) Json.TryDelete(path); }
                        status = "completed"; message = "扫描完成，请及时下载。";
                    }
                }
                catch (Exception error) { status = "failed"; message = error.Message; }
                lock (Store.Sync)
                {
                    job.status = status; job.message = message; job.finished_at = Legacy.Store.Now();
                    if (result != null && status == "completed") { job.file_id = result.id; job.name = result.name; }
                    try { Store.Save(); } catch (Exception error) { Console.Error.WriteLine("保存任务失败：" + error.Message); }
                }
            }
        }
        private void Cleanup()
        {
            while (!stopped)
            {
                for (int i = 0; i < 60 && !stopped; i++) Thread.Sleep(1000);
                if (stopped) break;
                try { lock (Store.Sync) Store.Cleanup(); }
                catch (Exception error) { Console.Error.WriteLine("文件清理失败：" + error.Message); }
            }
        }
        private static void RequireHost(Http http) { if (!http.Local) throw new ApiError(403, "请在打印主机打开 http://127.0.0.1 地址进行管理。"); }
        internal string ShareUrl() { return "http://" + Launch.LanIp() + ":" + Config.port; }
        public void Dispose()
        {
            stopped = true; listener.Stop(); notify.Set();
            accept.Join(2000); cleanup.Join(2000);
            // Device jobs are killed by the OS when this host exits. Active jobs
            // remain persisted and become interrupted on the next launch.
            queue.Join(2000);
        }
    }
}
