using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Printing;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Threading;

namespace LanPrint.Legacy
{
    internal sealed class Printer
    {
        public string id, name;
        public bool is_default, duplex, color;
        public List<string> papers = new List<string>();
    }
    internal sealed class Scanner { public string id, name; }
    internal sealed class Devices
    {
        public List<Printer> printers = new List<Printer>();
        public List<Scanner> scanners = new List<Scanner>();
        public List<string> warnings = new List<string>();
    }

    internal static class Backend
    {
        internal static Devices Devices(bool demo, bool virtualPrinters)
        {
            var devices = new Devices();
            if (demo)
            {
                devices.printers.Add(new Printer { id = "demo-printer", name = "演示 · 办公室打印机", is_default = true, duplex = true, color = true, papers = new List<string> { "A4", "A5", "Letter" } });
                devices.scanners.Add(new Scanner { id = "demo-scanner", name = "演示 · 平板扫描仪" });
                devices.warnings.Add("旧系统演示模式：不会使用真实打印机或扫描仪。");
                return devices;
            }
            foreach (string name in PrinterSettings.InstalledPrinters)
            {
                if (!virtualPrinters && new[] { "pdf", "xps", "onenote", "fax" }.Any(s => name.IndexOf(s, StringComparison.OrdinalIgnoreCase) >= 0)) continue;
                try
                {
                    devices.printers.Add(PrinterInfo(new PrinterSettings { PrinterName = name }));
                }
                catch (Exception error) { devices.warnings.Add("无法读取打印机 " + name + "：" + error.Message); }
            }
            var com = new ComScope();
            try
            {
                object manager = com.Create("WIA.DeviceManager"), infos = com.Get(manager, "DeviceInfos");
                for (int i = 1; i <= com.Number(infos, "Count"); i++)
                {
                    object info = com.Item(infos, i);
                    if (com.Number(info, "Type") != 1) continue;
                    object props = com.Get(info, "Properties"), name = com.Item(props, "Name");
                    devices.scanners.Add(new Scanner { id = (string)com.Get(info, "DeviceID"), name = (string)com.Get(name, "Value") });
                }
            }
            catch (Exception error) { devices.warnings.Add("扫描设备读取失败，请安装 32 位 WIA 驱动：" + error.Message); }
            finally { com.Dispose(); }
            return devices;
        }

        private static string PaperLabel(PaperKind kind)
        {
            if (kind == PaperKind.A4) return "A4";
            if (kind == PaperKind.A5) return "A5";
            return kind == PaperKind.Letter ? "Letter" : null;
        }

        private static Printer PrinterInfo(PrinterSettings settings)
        {
            var printer = new Printer { id = settings.PrinterName, name = settings.PrinterName, is_default = settings.IsDefaultPrinter, duplex = settings.CanDuplex, color = settings.SupportsColor };
            foreach (PaperSize paper in settings.PaperSizes)
            {
                string label = PaperLabel(paper.Kind);
                if (label != null && !printer.papers.Contains(label)) printer.papers.Add(label);
            }
            return printer;
        }

        internal static void Print(string path, PrintOptions options, bool demo, string output = null)
        {
            using (var document = new Document(path))
            using (var print = new PrintDocument())
            {
                if (demo) { options.Validate(Devices(true, true), document.Pages); Thread.Sleep(2000); return; }
                print.PrinterSettings.PrinterName = options.printer;
                if (!print.PrinterSettings.IsValid) throw new InvalidDataException("打印机不可用。");
                options.Validate(new Devices { printers = new List<Printer> { PrinterInfo(print.PrinterSettings) } }, document.Pages);
                // Copies are handled in our own collated page sequence, once only.
                print.PrinterSettings.Copies = 1;
                print.PrinterSettings.Duplex = options.duplex == "long" ? Duplex.Vertical : options.duplex == "short" ? Duplex.Horizontal : Duplex.Simplex;
                PaperSize paper = print.PrinterSettings.PaperSizes.Cast<PaperSize>().FirstOrDefault(p => PaperLabel(p.Kind) == options.paper);
                if (paper == null) throw new InvalidDataException("驱动未提供所选纸张。");
                print.DefaultPageSettings.PaperSize = paper;
                print.DefaultPageSettings.Landscape = options.landscape;
                print.DefaultPageSettings.Color = options.color;
                print.DefaultPageSettings.Margins = new Margins(0, 0, 0, 0);
                print.PrintController = new StandardPrintController();
                if (output != null) { print.PrinterSettings.PrintToFile = true; print.PrinterSettings.PrintFileName = output; }
                print.DocumentName = "局域打印站文档";
                int[] pages = PrintOptions.Ranges(options.pages, document.Pages);
                bool separator = options.duplex != "simplex" && pages.Length % 2 == 1;
                int perCopy = pages.Length + (separator ? 1 : 0), position = 0;
                print.PrintPage += delegate(object sender, PrintPageEventArgs e)
                {
                    int index = position % perCopy;
                    if (index < pages.Length)
                    {
                        // Graphics coordinates are physical hundredths of an inch;
                        // the default origin is already the printable area's corner.
                        RectangleF area = e.PageSettings.PrintableArea;
                        if (area.Width <= 0 || area.Height <= 0) throw new InvalidDataException("打印区域无效。");
                        int maximum = (int)Math.Min(4096, Math.Ceiling(Math.Max(area.Width, area.Height) / 100 * options.render_dpi));
                        using (Bitmap bitmap = document.Render(pages[index], maximum, !options.color, true))
                        {
                            float scale = Math.Min(area.Width / bitmap.Width, area.Height / bitmap.Height);
                            float w = bitmap.Width * scale, h = bitmap.Height * scale;
                            e.Graphics.DrawImage(bitmap, (area.Width - w) / 2, (area.Height - h) / 2, w, h);
                        }
                    }
                    // An odd duplex range gets a blank back before the next copy.
                    e.HasMorePages = ++position < perCopy * options.copies;
                };
                print.Print();
            }
        }

        internal static void Scan(string output, ScanOptions options, bool demo)
        {
            if (demo)
            {
                Thread.Sleep(2000);
                using (var sample = new Bitmap(1240, 1754))
                {
                    using (Graphics g = Graphics.FromImage(sample))
                    {
                        g.Clear(Color.White);
                        using (var brush = new SolidBrush(options.color ? Color.FromArgb(30, 91, 80) : Color.FromArgb(73, 73, 73))) g.FillRectangle(brush, 100, 160, 1040, 100);
                        for (int y = 400; y < 1450; y += 100) g.FillRectangle(Brushes.Gray, 100, y, 950, 12);
                    }
                    Raster.SaveScan(sample, output, options);
                }
                return;
            }
            var com = new ComScope();
            string temp = output + ".wia.bmp";
            try
            {
                object manager = com.Create("WIA.DeviceManager"), infos = com.Get(manager, "DeviceInfos"), selected = null;
                for (int i = 1; i <= com.Number(infos, "Count"); i++)
                {
                    object info = com.Item(infos, i);
                    if (com.Number(info, "Type") == 1 && (string)com.Get(info, "DeviceID") == options.scanner) { selected = info; break; }
                }
                if (selected == null) throw new InvalidDataException("扫描仪未连接。");
                object device = com.Call(selected, "Connect"), properties = com.Get(device, "Properties");
                object source = com.OptionalItem(properties, 3088);
                if (source != null) com.Put(source, "Value", options.source == "feeder" ? 1 : 2);
                else if (options.source == "feeder") throw new InvalidDataException("此设备不支持 WIA 自动进纸器。");
                if (options.source == "feeder")
                {
                    object count = com.OptionalItem(properties, 3096);
                    if (count != null) com.Put(count, "Value", 1);
                }
                object item = com.Item(com.Get(device, "Items"), 1), props = com.Get(item, "Properties");
                foreach (var value in new[] { new[] { 6146, options.color ? 1 : 2 }, new[] { 6147, options.dpi }, new[] { 6148, options.dpi }, new[] { 6149, 0 }, new[] { 6150, 0 } })
                    com.Put(com.Item(props, value[0]), "Value", value[1]);
                foreach (int id in new[] { 6151, 6152 })
                {
                    object prop = com.Item(props, id);
                    try { int max = com.Number(prop, "SubTypeMax"); if (max > 0) com.Put(prop, "Value", max); }
                    catch (COMException) { }
                }
                object image = com.Call(item, "Transfer", "{B96B3CAB-0728-11D3-9D7B-0000F81EF32E}");
                com.Call(image, "SaveFile", temp);
                using (var document = new Document(temp))
                using (Bitmap bitmap = document.RenderScan(!options.color)) Raster.SaveScan(bitmap, output, options);
            }
            finally { com.Dispose(); Json.TryDelete(temp); }
        }
    }

    // Track every RCW once, and release in reverse order in the worker's STA.
    internal sealed class ComScope : IDisposable
    {
        private readonly List<object> objects = new List<object>();
        private object Track(object value)
        {
            if (value != null && Marshal.IsComObject(value) && !objects.Any(o => Object.ReferenceEquals(o, value))) objects.Add(value);
            return value;
        }
        internal object Create(string id)
        {
            Type type = Type.GetTypeFromProgID(id, true);
            return Track(Activator.CreateInstance(type));
        }
        private object Invoke(object obj, string member, BindingFlags flags, params object[] args)
        {
            try { return Track(obj.GetType().InvokeMember(member, flags, null, obj, args)); }
            catch (TargetInvocationException error) { throw error.InnerException ?? error; }
        }
        internal object Get(object obj, string name) { return Invoke(obj, name, BindingFlags.GetProperty); }
        internal object Call(object obj, string name, params object[] args) { return Invoke(obj, name, BindingFlags.InvokeMethod, args); }
        internal void Put(object obj, string name, object value) { Invoke(obj, name, BindingFlags.SetProperty, value); }
        internal object Item(object obj, object id) { return Invoke(obj, "Item", BindingFlags.GetProperty, id); }
        internal object OptionalItem(object obj, object id) { try { return Item(obj, id); } catch (COMException) { return null; } }
        internal int Number(object obj, string name) { return Convert.ToInt32(Get(obj, name)); }
        public void Dispose()
        {
            for (int i = objects.Count - 1; i >= 0; i--) Marshal.FinalReleaseComObject(objects[i]);
            objects.Clear();
        }
    }
}
