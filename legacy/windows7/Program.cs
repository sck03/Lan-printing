using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Runtime.Versioning;
using System.Security.Cryptography;
using System.Text;
using System.Windows.Forms;
using Microsoft.Win32;

[assembly: AssemblyTitle("局域打印站 · Windows 7 32 位版")]
[assembly: AssemblyVersion("0.1.0.0")]
[assembly: TargetFramework(".NETFramework,Version=v4.0")]

namespace LanPrint.Legacy
{
    internal static class Program
    {
        [STAThread]
        internal static int Main(string[] args)
        {
            bool noTray = Array.IndexOf(args, "--no-tray") >= 0;
            try
            {
                // A GUI-subsystem process has no console code page to change.
                // Write redirected logs as UTF-8 without SetConsoleOutputCP.
                Console.SetOut(new StreamWriter(Console.OpenStandardOutput(), new UTF8Encoding(false)) { AutoFlush = true });
                Console.SetError(new StreamWriter(Console.OpenStandardError(), new UTF8Encoding(false)) { AutoFlush = true });
                if (args.Length == 3 && args[0] == "--worker") return Worker.Entry(args[1], args[2]);
                bool demo = false, background = false;
                int? port = null;
                string root = null;
                for (int i = 0; i < args.Length; i++)
                {
                    switch (args[i])
                    {
                        case "--demo": demo = true; break;
                        case "--no-tray": break;
                        case "--background": background = true; break;
                        case "--port":
                            int number;
                            if (++i >= args.Length || !Int32.TryParse(args[i], out number)) throw new InvalidDataException("缺少或无效的端口。");
                            port = number; break;
                        case "--data-dir":
                            if (++i >= args.Length) throw new InvalidDataException("缺少数据目录。");
                            root = args[i]; break;
                        case "--help": Console.WriteLine("LanPrintLegacy [--port 17860] [--data-dir PATH] [--no-tray] [--background] [--demo]"); return 0;
                        default: throw new InvalidDataException("未知参数：" + args[i]);
                    }
                }
                if (Environment.OSVersion.Platform != PlatformID.Win32NT || Environment.OSVersion.Version < new Version(6, 1) || Environment.Is64BitProcess)
                    throw new InvalidDataException("请使用 Windows 7 SP1 或更新系统上的旧系统版 32 位程序。");
                root = Path.GetFullPath(root ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), demo ? "LanPrint-legacy-demo" : "LanPrint-legacy"));
                Directory.CreateDirectory(root);
                FileStream instance;
                try { instance = new FileStream(Path.Combine(root, "instance.lock"), FileMode.OpenOrCreate, FileAccess.ReadWrite, FileShare.None); }
                catch (IOException error)
                {
                    int code = Marshal.GetHRForException(error) & 0xFFFF;
                    if (code != 32 && code != 33) throw;
                    if (!noTray && !background)
                    {
                        int running = port ?? (demo ? 17861 : 17860);
                        string configPath = Path.Combine(root, "config.json");
                        if (port == null && File.Exists(configPath)) running = Json.Read<Config>(configPath, 64 * 1024).port;
                        Launch.Open("http://127.0.0.1:" + running);
                    }
                    return 0;
                }
                using (instance)
                {
                    string configPath = Path.Combine(root, "config.json");
                    Config config;
                    if (File.Exists(configPath)) config = Json.Read<Config>(configPath, 64 * 1024);
                    else
                    {
                        config = new Config(); if (demo) config.port = 17861;
                        Json.Write(configPath, config);
                    }
                    if (config == null) throw new InvalidDataException("config.json 无效，已保留原文件。");
                    if (port != null) config.port = port.Value;
                    config.Validate();
                    using (var server = new Server(root, config, demo))
                    {
                        Console.WriteLine("LanPrint Legacy x86: http://127.0.0.1:" + config.port + "\n局域网地址: " + server.ShareUrl() + "\n数据目录: " + root);
                        if (noTray) server.Wait();
                        else Tray(server, root, port, demo, background);
                    }
                }
                return 0;
            }
            catch (Exception error)
            {
                Console.Error.WriteLine(error.Message);
                if (!noTray && (args.Length == 0 || args[0] != "--worker")) MessageBox.Show(error.Message, "局域打印站 · 旧系统版", MessageBoxButtons.OK, MessageBoxIcon.Error);
                return 1;
            }
        }

        private static void Tray(Server server, string root, int? port, bool demo, bool background)
        {
            Application.EnableVisualStyles();
            string url = "http://127.0.0.1:" + server.Config.port;
            string launchArgs = "--data-dir " + Launch.Quote(root) + (port == null ? "" : " --port " + port) + (demo ? " --demo" : "");
            string exe = Process.GetCurrentProcess().MainModule.FileName;
            string startupName;
            using (var hash = SHA256.Create()) startupName = "LanPrint-Legacy-" + BitConverter.ToString(hash.ComputeHash(Encoding.UTF8.GetBytes(root.ToLowerInvariant())), 0, 8).Replace("-", "");
            using (var icon = Icon.ExtractAssociatedIcon(exe))
            using (var menu = new ContextMenuStrip())
            using (var tray = new NotifyIcon { Icon = icon, Text = "局域打印站 · 旧系统版", ContextMenuStrip = menu, Visible = true })
            {
                menu.Items.Add("打开打印站", null, delegate { Launch.Open(url); });
                menu.Items.Add("查看同事访问地址", null, delegate { MessageBox.Show(server.ShareUrl(), "同事访问地址"); });
                menu.Items.Add("打开配置和数据目录", null, delegate { Launch.Open(root); });
                menu.Items.Add("创建桌面快捷方式", null, delegate {
                    TryUi(delegate {
                        using (var com = new ComScope())
                        {
                            object shell = com.Create("WScript.Shell");
                            object link = com.Call(shell, "CreateShortcut", Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory), "局域打印站（旧系统版）.lnk"));
                            com.Put(link, "TargetPath", exe); com.Put(link, "Arguments", launchArgs);
                            com.Put(link, "WorkingDirectory", Path.GetDirectoryName(exe)); com.Put(link, "IconLocation", exe + ",0");
                            com.Call(link, "Save");
                        }
                    });
                });
                var startup = new ToolStripMenuItem("当前用户登录后自动启动");
                menu.Items.Add(startup);
                Action refreshStartup = delegate {
                    using (RegistryKey key = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Run")) startup.Checked = key != null && key.GetValue(startupName) != null;
                };
                menu.Opening += delegate { TryUi(refreshStartup); };
                startup.Click += delegate {
                    TryUi(delegate {
                        using (RegistryKey key = Registry.CurrentUser.CreateSubKey(@"Software\Microsoft\Windows\CurrentVersion\Run"))
                        {
                            string command = Launch.Quote(exe) + " --background " + launchArgs;
                            if (command.Length > 260) throw new InvalidDataException("启动路径过长，请使用较短的固定程序和数据目录。");
                            if (startup.Checked) key.DeleteValue(startupName, false);
                            else key.SetValue(startupName, command, RegistryValueKind.String);
                        }
                        refreshStartup();
                    });
                };
                var awake = new ToolStripMenuItem("本次运行阻止自动睡眠");
                menu.Items.Add(awake);
                awake.Click += delegate {
                    if (SetThreadExecutionState(0x80000000U | (awake.Checked ? 0U : 1U)) != 0) awake.Checked = !awake.Checked;
                    else MessageBox.Show("无法设置防自动睡眠。", "旧系统版");
                };
                menu.Items.Add("打开 Windows 打印机设置", null, delegate { Launch.Open("control.exe", "printers"); });
                menu.Items.Add("打开 Windows 防火墙", null, delegate { Launch.Open("control.exe", "/name Microsoft.WindowsFirewall"); });
                menu.Items.Add("退出打印站", null, delegate { Application.ExitThread(); });
                tray.DoubleClick += delegate { Launch.Open(url); };
                if (!background) Launch.Open(url);
                try { Application.Run(); }
                finally { tray.Visible = false; SetThreadExecutionState(0x80000000U); }
            }
        }
        private static void TryUi(Action action)
        {
            try { action(); } catch (Exception error) { MessageBox.Show(error.Message, "局域打印站 · 旧系统版", MessageBoxButtons.OK, MessageBoxIcon.Error); }
        }
        [DllImport("kernel32.dll")] private static extern uint SetThreadExecutionState(uint flags);
    }

    internal static class Launch
    {
        internal static void Open(string value) { Open(value, ""); }
        internal static void Open(string value, string arguments)
        {
            Process.Start(new ProcessStartInfo { FileName = value, Arguments = arguments, UseShellExecute = true });
        }
        internal static string LanIp()
        {
            try
            {
                using (var socket = new Socket(AddressFamily.InterNetwork, SocketType.Dgram, ProtocolType.Udp))
                {
                    socket.Connect("192.0.2.1", 80);
                    IPAddress address = ((IPEndPoint)socket.LocalEndPoint).Address;
                    if (Security.Private(address) && !IPAddress.IsLoopback(address)) return address.ToString();
                }
            }
            catch (SocketException) { }
            try
            {
                foreach (IPAddress address in Dns.GetHostAddresses(Environment.MachineName))
                    if (Security.Private(address) && !IPAddress.IsLoopback(address)) return address.ToString();
            }
            catch (SocketException) { }
            return "127.0.0.1";
        }
        internal static string Quote(string value)
        {
            var output = new StringBuilder("\"");
            int slashes = 0;
            foreach (char c in value)
            {
                if (c == '\\') { slashes++; continue; }
                output.Append('\\', c == '"' ? slashes * 2 + 1 : slashes); output.Append(c); slashes = 0;
            }
            output.Append('\\', slashes * 2); output.Append('"'); return output.ToString();
        }
    }

    internal static class Resources
    {
        private static readonly Dictionary<string, byte[]> cache = new Dictionary<string, byte[]>();
        internal static byte[] Bytes(string name)
        {
            lock (cache)
            {
                byte[] bytes;
                if (cache.TryGetValue(name, out bytes)) return bytes;
                using (Stream resource = Assembly.GetExecutingAssembly().GetManifestResourceStream("LanPrint." + name))
                {
                    if (resource == null) throw new FileNotFoundException("缺少内嵌资源：" + name);
                    bytes = new byte[(int)resource.Length]; Json.ReadExactly(resource, bytes, 0, bytes.Length);
                }
                cache.Add(name, bytes); return bytes;
            }
        }
        internal static string Text(string name) { return Json.Utf8.GetString(Bytes(name)); }
    }
}
