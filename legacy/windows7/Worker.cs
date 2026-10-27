using System;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;
using Microsoft.Win32.SafeHandles;

namespace LanPrint.Legacy
{
    internal sealed class WorkerRequest
    {
        public string license = "", operation = "", path = "", output = "", gate = "";
        public int page = 0;
        public PrintOptions print;
        public ScanOptions scan;
        public bool show_virtual = false, demo = false;
    }
    internal sealed class WorkerResponse
    {
        public string error;
        public int pages = 0;
        public Devices devices;
    }

    internal static class Worker
    {
        internal static int Entry(string input, string output)
        {
            var response = new WorkerResponse();
            try
            {
                WorkerRequest request = Json.Read<WorkerRequest>(input, 64 * 1024);
                if (request == null) throw new InvalidDataException("设备请求为空。");
                if (!String.IsNullOrEmpty(request.gate))
                    using (EventWaitHandle gate = EventWaitHandle.OpenExisting(request.gate))
                        if (!gate.WaitOne(10000)) throw new InvalidDataException("设备进程未获得执行许可。");
                var licensing = new Licensing(Path.GetDirectoryName(input), Security.Hex(Resources.Text("license-public-key.hex").Trim()));
                Licensing.Verify(request.license, licensing.Machine, Security.Hex(Resources.Text("license-public-key.hex").Trim()));
                switch (request.operation)
                {
                    case "devices": response.devices = Backend.Devices(request.demo, request.show_virtual); break;
                    case "inspect": using (var document = new Document(request.path)) response.pages = document.Pages; break;
                    case "preview":
                        using (var document = new Document(request.path))
                        using (var bitmap = document.Render(request.page, 1400, false, false)) Raster.Jpeg(bitmap, request.output);
                        break;
                    case "print":
                        if (request.print == null) throw new InvalidDataException("缺少打印参数。");
                        Backend.Print(request.path, request.print, request.demo);
                        break;
                    case "scan":
                        if (request.scan == null) throw new InvalidDataException("缺少扫描参数。");
                        request.scan.Validate(Backend.Devices(request.demo, true));
                        Backend.Scan(request.output, request.scan, request.demo);
                        response.pages = 1;
                        break;
                    default: throw new InvalidDataException("未知设备操作。");
                }
            }
            catch (Exception error) { response.error = error.Message; }
            try { Json.Write(output, response); }
            catch { return 3; }
            return response.error == null ? 0 : 1;
        }

        internal static WorkerResponse Call(string root, Licensing licensing, WorkerRequest request, int seconds)
        {
            request.license = licensing.Code();
            licensing.Load();
            string work = Path.Combine(root, "work", Guid.NewGuid().ToString("D"));
            Directory.CreateDirectory(work);
            string destination = request.output;
            if (!String.IsNullOrEmpty(destination)) request.output = Path.Combine(work, "output" + Path.GetExtension(destination));
            request.gate = @"Local\LanPrintLegacy-" + Guid.NewGuid().ToString("D");
            bool exited = true;
            try
            {
                string input = Path.Combine(work, "request.json"), output = Path.Combine(work, "response.json");
                Json.Write(input, request);
                using (var gate = new EventWaitHandle(false, EventResetMode.ManualReset, request.gate))
                using (var job = new ProcessJob())
                using (var process = new Process())
                {
                    process.StartInfo = new ProcessStartInfo {
                        FileName = Process.GetCurrentProcess().MainModule.FileName,
                        Arguments = "--worker " + Launch.Quote(input) + " " + Launch.Quote(output),
                        WorkingDirectory = work, UseShellExecute = false, CreateNoWindow = true
                    };
                    if (!process.Start()) throw new InvalidDataException("无法启动设备进程。");
                    exited = false;
                    try
                    {
                        // Windows 7 cannot nest jobs. Fail before releasing the
                        // worker's gate if a launcher prevents process isolation.
                        job.Assign(process.Handle);
                        gate.Set();
                        if (!process.WaitForExit(seconds * 1000))
                            throw new InvalidDataException("设备响应超时。打印可能已送达，请先检查主机队列，避免重复提交。");
                        exited = true;
                        if (process.ExitCode != 0 && process.ExitCode != 1)
                            throw new InvalidDataException("设备进程异常退出，可能内存不足或驱动故障。打印请先检查主机队列。");
                        WorkerResponse response = Json.Read<WorkerResponse>(output, 1024 * 1024);
                        if (response == null || response.error != null) throw new InvalidDataException(response == null ? "设备未返回结果。" : response.error);
                        if (process.ExitCode != 0) throw new InvalidDataException("设备操作未成功。");
                        if (!String.IsNullOrEmpty(destination)) File.Move(request.output, destination);
                        return response;
                    }
                    finally
                    {
                        // Closing the kill-on-close job also stops a worker when
                        // the host crashes/exits. Reap it before removing files.
                        job.Dispose();
                        if (!exited)
                        {
                            try { if (!process.HasExited) process.Kill(); } catch (InvalidOperationException) { } catch (Win32Exception) { }
                            exited = process.WaitForExit(5000);
                        }
                    }
                }
            }
            finally { if (exited) Json.DeleteScratch(work); }
        }
    }

    internal sealed class ProcessJob : IDisposable
    {
        private readonly SafeJobHandle handle;
        internal ProcessJob()
        {
            handle = CreateJobObject(IntPtr.Zero, null);
            if (handle.IsInvalid) throw new Win32Exception();
            var limits = new ExtendedLimits();
            limits.Basic.LimitFlags = 0x2000 | 0x100; // KILL_ON_JOB_CLOSE | PROCESS_MEMORY
            limits.ProcessMemoryLimit = new UIntPtr(384 * 1024 * 1024);
            if (!SetInformationJobObject(handle, 9, ref limits, (uint)Marshal.SizeOf(limits)))
            {
                handle.Dispose();
                throw new Win32Exception();
            }
        }
        internal void Assign(IntPtr process)
        {
            if (!AssignProcessToJobObject(handle, process))
                throw new InvalidDataException("无法隔离设备进程，请直接从资源管理器启动旧系统版。" , new Win32Exception());
        }
        public void Dispose() { handle.Dispose(); }

        [StructLayout(LayoutKind.Sequential)]
        private struct BasicLimits
        {
            internal long ProcessTime, JobTime;
            internal uint LimitFlags;
            internal UIntPtr MinimumWorkingSet, MaximumWorkingSet;
            internal uint ActiveProcessLimit;
            internal UIntPtr Affinity;
            internal uint PriorityClass, SchedulingClass;
        }
        [StructLayout(LayoutKind.Sequential)]
        private struct IoCounters { internal ulong ReadCount, WriteCount, OtherCount, ReadBytes, WriteBytes, OtherBytes; }
        [StructLayout(LayoutKind.Sequential)]
        private struct ExtendedLimits
        {
            internal BasicLimits Basic;
            internal IoCounters Io;
            internal UIntPtr ProcessMemoryLimit, JobMemoryLimit, PeakProcessMemory, PeakJobMemory;
        }
        private sealed class SafeJobHandle : SafeHandleZeroOrMinusOneIsInvalid
        {
            private SafeJobHandle() : base(true) { }
            protected override bool ReleaseHandle() { return CloseHandle(handle); }
        }
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true, EntryPoint = "CreateJobObjectW")]
        private static extern SafeJobHandle CreateJobObject(IntPtr security, string name);
        [DllImport("kernel32.dll", SetLastError = true)] [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool SetInformationJobObject(SafeJobHandle job, int kind, ref ExtendedLimits limits, uint length);
        [DllImport("kernel32.dll", SetLastError = true)] [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool AssignProcessToJobObject(SafeJobHandle job, IntPtr process);
        [DllImport("kernel32.dll")] [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool CloseHandle(IntPtr handle);
    }
}
