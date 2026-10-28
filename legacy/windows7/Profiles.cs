using System;
using System.ComponentModel;
using System.Drawing.Printing;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Windows.Forms;

namespace LanPrint.Legacy
{
    internal sealed class DriverProfile
    {
        public string id, name, printer, driver, devmode;
        internal object Public() { return new { id, name, printer }; }
    }
    internal sealed class ProfileInput { public string printer, name; }

    internal static class Profiles
    {
        private static string FilePath(string root, string id)
        {
            Store.RequireId(id);
            return Path.Combine(root, "profiles", id + ".json");
        }
        internal static DriverProfile Load(string root, string id, string printer)
        {
            DriverProfile profile = Json.Read<DriverProfile>(FilePath(root, id), 128 * 1024);
            if (profile == null || profile.id != id || profile.printer != printer)
                throw new InvalidDataException("打印机与预设不匹配，请重新选择预设。");
            Decode(profile);
            return profile;
        }
        internal static object[] List(string root)
        {
            string directory = Path.Combine(root, "profiles");
            if (!Directory.Exists(directory)) return new object[0];
            string[] files = Directory.GetFiles(directory, "*.json");
            if (files.Length > 50) throw new InvalidDataException("驱动预设最多 50 个。");
            return files.OrderBy(p => p, StringComparer.Ordinal).Select(path => {
                DriverProfile profile = Json.Read<DriverProfile>(path, 128 * 1024);
                if (profile == null || profile.id != Path.GetFileNameWithoutExtension(path)) throw new InvalidDataException("驱动预设文件无效。");
                Store.RequireId(profile.id); Decode(profile); return profile.Public();
            }).ToArray();
        }
        internal static void Save(string root, DriverProfile profile)
        {
            Decode(profile);
            Directory.CreateDirectory(Path.Combine(root, "profiles"));
            Json.Write(FilePath(root, profile.id), profile);
        }
        internal static void Remove(string root, string id) { File.Delete(FilePath(root, id)); }
        private static byte[] Decode(DriverProfile profile)
        {
            if (String.IsNullOrEmpty(profile.driver) || profile.devmode == null || profile.devmode.Length > 90 * 1024)
                throw new InvalidDataException("驱动预设数据无效。");
            byte[] bytes;
            try { bytes = Convert.FromBase64String(profile.devmode); }
            catch (FormatException) { throw new InvalidDataException("驱动预设数据无效。"); }
            if (bytes.Length < 220 || Word(bytes, 68) < 220 || Word(bytes, 68) + Word(bytes, 70) != bytes.Length)
                throw new InvalidDataException("驱动预设格式不兼容，请重新创建。");
            return bytes;
        }
        private static int Word(byte[] bytes, int offset) { return bytes[offset] | bytes[offset + 1] << 8; }

        internal static DriverProfile Capture(string printer, bool prompt)
        {
            Application.OleRequired();
            using (var mode = new NativeMode(printer))
            {
                string driver = mode.Signature();
                if (prompt)
                {
                    int result = DocumentProperties(IntPtr.Zero, mode.Handle, printer, mode.Data, mode.Data, 4 | 8 | 2);
                    if (result == 2) throw new InvalidDataException("已取消驱动预设设置。");
                    if (result != 1) throw new InvalidDataException("打开打印机原厂设置失败。");
                }
                return new DriverProfile { id = Guid.NewGuid().ToString("D"), name = "", printer = printer, driver = driver,
                    devmode = Convert.ToBase64String(mode.Bytes()) };
            }
        }
        internal static void Apply(PrintDocument document, DriverProfile profile)
        {
            byte[] saved = Decode(profile);
            using (var mode = new NativeMode(document.PrinterSettings.PrinterName))
            {
                if (profile.printer != document.PrinterSettings.PrinterName || profile.driver != mode.Signature())
                    throw new InvalidDataException("打印机或驱动版本与预设不匹配，请在主机重新创建预设。");
                byte[] current = mode.Bytes();
                if (saved.Length != current.Length || !saved.Skip(64).Take(8).SequenceEqual(current.Skip(64).Take(8)))
                    throw new InvalidDataException("驱动预设格式不兼容，请重新创建。");
                Marshal.Copy(saved, 0, mode.Data, saved.Length);
                // Only the application handles copies. Keep all vendor-private
                // paper/quality/high-speed/color data and normalize with the driver.
                Marshal.WriteInt32(mode.Data, 72, Marshal.ReadInt32(mode.Data, 72) | 0x100);
                Marshal.WriteInt16(mode.Data, 86, 1);
                if (DocumentProperties(IntPtr.Zero, mode.Handle, profile.printer, mode.Data, mode.Data, 8 | 2) != 1)
                    throw new InvalidDataException("打印机拒绝预设参数。");
                byte[] normalized = mode.Bytes();
                IntPtr global = GlobalAlloc(0x42, new UIntPtr((uint)normalized.Length));
                if (global == IntPtr.Zero) throw new Win32Exception();
                try
                {
                    IntPtr pointer = GlobalLock(global);
                    if (pointer == IntPtr.Zero) throw new Win32Exception();
                    try { Marshal.Copy(normalized, 0, pointer, normalized.Length); }
                    finally { GlobalUnlock(global); }
                    document.PrinterSettings.SetHdevmode(global);
                    document.DefaultPageSettings.SetHdevmode(global);
                }
                finally { GlobalFree(global); }
            }
        }

        private sealed class NativeMode : IDisposable
        {
            internal IntPtr Handle, Data;
            private readonly int capacity;
            internal NativeMode(string printer)
            {
                try
                {
                    if (!OpenPrinter(printer, out Handle, IntPtr.Zero)) throw new Win32Exception();
                    capacity = DocumentProperties(IntPtr.Zero, Handle, printer, IntPtr.Zero, IntPtr.Zero, 0);
                    if (capacity < 220 || capacity > 1024 * 1024) throw new InvalidDataException("无法读取打印机设置。");
                    Data = Marshal.AllocHGlobal(capacity);
                    Marshal.Copy(new byte[capacity], 0, Data, capacity);
                    if (DocumentProperties(IntPtr.Zero, Handle, printer, Data, IntPtr.Zero, 2) != 1)
                        throw new InvalidDataException("读取打印机设置失败。");
                }
                catch { Dispose(); throw; }
            }
            internal byte[] Bytes()
            {
                int size = (ushort)Marshal.ReadInt16(Data, 68), extra = (ushort)Marshal.ReadInt16(Data, 70);
                if (size < 220 || size + extra > capacity) throw new InvalidDataException("打印驱动返回了无效设置。");
                byte[] bytes = new byte[size + extra]; Marshal.Copy(Data, bytes, 0, bytes.Length); return bytes;
            }
            internal string Signature()
            {
                uint needed;
                GetPrinterDriver(Handle, null, 6, IntPtr.Zero, 0, out needed);
                if (needed < Marshal.SizeOf(typeof(DriverInfo)) || needed > 1024 * 1024) throw new InvalidDataException("无法读取打印机驱动版本。");
                IntPtr buffer = Marshal.AllocHGlobal((int)needed);
                try
                {
                    if (!GetPrinterDriver(Handle, null, 6, buffer, needed, out needed)) throw new Win32Exception();
                    var info = (DriverInfo)Marshal.PtrToStructure(buffer, typeof(DriverInfo));
                    return Marshal.PtrToStringUni(info.Name) + ":" + info.Version + ":" + info.DateHigh + ":" + info.DateLow;
                }
                finally { Marshal.FreeHGlobal(buffer); }
            }
            public void Dispose()
            {
                if (Data != IntPtr.Zero) { Marshal.FreeHGlobal(Data); Data = IntPtr.Zero; }
                if (Handle != IntPtr.Zero) { ClosePrinter(Handle); Handle = IntPtr.Zero; }
            }
        }
        [StructLayout(LayoutKind.Sequential)]
        private struct DriverInfo
        {
            internal uint DriverVersion;
            internal IntPtr Name, Environment, DriverPath, DataFile, ConfigFile, HelpFile, DependentFiles, MonitorName, DefaultDataType, PreviousNames;
            internal uint DateLow, DateHigh;
            internal ulong Version;
            internal IntPtr Manufacturer, Url, HardwareId, Provider;
        }
        [DllImport("winspool.drv", CharSet = CharSet.Unicode, SetLastError = true, EntryPoint = "OpenPrinterW")]
        [return: MarshalAs(UnmanagedType.Bool)] private static extern bool OpenPrinter(string name, out IntPtr handle, IntPtr defaults);
        [DllImport("winspool.drv")] [return: MarshalAs(UnmanagedType.Bool)] private static extern bool ClosePrinter(IntPtr handle);
        [DllImport("winspool.drv", CharSet = CharSet.Unicode, EntryPoint = "DocumentPropertiesW")]
        private static extern int DocumentProperties(IntPtr window, IntPtr printer, string name, IntPtr output, IntPtr input, int flags);
        [DllImport("winspool.drv", CharSet = CharSet.Unicode, SetLastError = true, EntryPoint = "GetPrinterDriverW")]
        [return: MarshalAs(UnmanagedType.Bool)] private static extern bool GetPrinterDriver(IntPtr printer, string environment, uint level, IntPtr buffer, uint size, out uint needed);
        [DllImport("kernel32.dll", SetLastError = true)] private static extern IntPtr GlobalAlloc(uint flags, UIntPtr size);
        [DllImport("kernel32.dll", SetLastError = true)] private static extern IntPtr GlobalLock(IntPtr handle);
        [DllImport("kernel32.dll")] [return: MarshalAs(UnmanagedType.Bool)] private static extern bool GlobalUnlock(IntPtr handle);
        [DllImport("kernel32.dll")] private static extern IntPtr GlobalFree(IntPtr handle);
    }
}
