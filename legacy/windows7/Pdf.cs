using System;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;

namespace LanPrint.Legacy
{
    internal sealed class Document : IDisposable
    {
        private readonly PdfDocument pdf;
        private readonly Image image;
        internal int Pages { get; private set; }

        internal Document(string path)
        {
            if (Store.Extension(path) == "pdf")
            {
                pdf = new PdfDocument(path);
                Pages = pdf.Pages;
            }
            else
            {
                image = Image.FromFile(path);
                try
                {
                    if (image.Width < 1 || image.Height < 1 || image.Width > 16000 || image.Height > 16000
                        || (long)image.Width * image.Height > 24 * 1000 * 1000
                        || !(image.RawFormat.Equals(ImageFormat.Png) || image.RawFormat.Equals(ImageFormat.Jpeg) || image.RawFormat.Equals(ImageFormat.Bmp)))
                        throw new InvalidDataException("图片格式不支持或尺寸过大（最多 2400 万像素、单边 16000）。");
                    Pages = 1;
                }
                catch { image.Dispose(); throw; }
            }
            PrintOptions.Ranges("", Pages);
        }

        internal Bitmap Render(int page, int maximum, bool gray, bool printing)
        {
            if (page < 0 || page >= Pages || maximum < 1 || maximum > 7016)
                throw new InvalidDataException("页面或渲染尺寸无效。");
            if (pdf != null) return pdf.Render(page, maximum, gray, printing);
            return RenderImage(maximum, gray);
        }

        internal Bitmap RenderScan(bool gray)
        {
            if (image == null) throw new InvalidDataException("扫描结果必须是图片。");
            return RenderImage(Math.Max(image.Width, image.Height), gray);
        }

        private Bitmap RenderImage(int maximum, bool gray)
        {
            double scale = Math.Min(1, (double)maximum / Math.Max(image.Width, image.Height));
            var bitmap = new Bitmap(Math.Max(1, (int)(image.Width * scale)), Math.Max(1, (int)(image.Height * scale)), PixelFormat.Format24bppRgb);
            try
            {
                using (Graphics graphics = Graphics.FromImage(bitmap))
                using (ImageAttributes attributes = Raster.Attributes(gray))
                {
                    graphics.Clear(Color.White);
                    graphics.InterpolationMode = InterpolationMode.HighQualityBicubic;
                    graphics.CompositingQuality = CompositingQuality.HighQuality;
                    graphics.DrawImage(image, new Rectangle(0, 0, bitmap.Width, bitmap.Height), 0, 0, image.Width, image.Height, GraphicsUnit.Pixel, attributes);
                }
                return bitmap;
            }
            catch { bitmap.Dispose(); throw; }
        }

        public void Dispose() { if (pdf != null) pdf.Dispose(); if (image != null) image.Dispose(); }
    }

    internal sealed class PdfDocument : IDisposable
    {
        private static bool initialized;
        private static readonly object initLock = new object();
        private FileStream input;
        private GetBlock callback;
        private IntPtr fileAccess;
        private readonly byte[] readBuffer = new byte[64 * 1024];
        private IntPtr document;
        internal int Pages { get; private set; }

        internal PdfDocument(string path)
        {
            lock (initLock)
            {
                if (!initialized)
                {
                    string library = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "pdfium.dll");
                    if (LoadLibraryEx(library, IntPtr.Zero, 8) == IntPtr.Zero)
                        throw new InvalidDataException("无法加载随包的 32 位 pdfium.dll，请完整解压旧系统版。");
                    FPDF_InitLibrary();
                    initialized = true;
                }
            }
            try
            {
                // Random-access file callbacks avoid a 100–200 MB managed copy
                // and also support Unicode paths on PDFium's older Windows build.
                input = new FileStream(path, FileMode.Open, System.IO.FileAccess.Read, FileShare.Read);
                if (input.Length < 1 || input.Length > 200 * 1024L * 1024) throw new InvalidDataException("PDF 为空或超过 200 MB。");
                callback = ReadBlock;
                var access = new NativeFileAccess { Length = (uint)input.Length, GetBlock = Marshal.GetFunctionPointerForDelegate(callback) };
                fileAccess = Marshal.AllocHGlobal(Marshal.SizeOf(access));
                Marshal.StructureToPtr(access, fileAccess, false);
                document = FPDF_LoadCustomDocument(fileAccess, IntPtr.Zero);
                if (document == IntPtr.Zero) throw new InvalidDataException("PDF 无法打开，可能已损坏或需要密码。");
                Pages = FPDF_GetPageCount(document);
                PrintOptions.Ranges("", Pages);
            }
            catch { Dispose(); throw; }
        }

        private int ReadBlock(IntPtr parameter, uint position, IntPtr destination, uint count)
        {
            // Exceptions must never cross the unmanaged callback boundary.
            try
            {
                lock (input)
                {
                    if ((long)position + count > input.Length) return 0;
                    input.Position = position;
                    while (count > 0)
                    {
                        int n = (int)Math.Min(readBuffer.Length, count);
                        Json.ReadExactly(input, readBuffer, 0, n);
                        Marshal.Copy(readBuffer, 0, destination, n);
                        destination = new IntPtr(destination.ToInt64() + n);
                        count -= (uint)n;
                    }
                    return 1;
                }
            }
            catch { return 0; }
        }

        internal Bitmap Render(int index, int maximum, bool gray, bool printing)
        {
            IntPtr page = FPDF_LoadPage(document, index);
            if (page == IntPtr.Zero) throw new InvalidDataException("PDF 页面无法打开。");
            try
            {
                double width = FPDF_GetPageWidth(page), height = FPDF_GetPageHeight(page);
                if (Double.IsNaN(width) || Double.IsInfinity(width) || Double.IsNaN(height) || Double.IsInfinity(height) || width <= 0 || height <= 0)
                    throw new InvalidDataException("PDF 页面尺寸无效。");
                double scale = maximum / Math.Max(width, height);
                int w = Math.Max(1, (int)(width * scale)), h = Math.Max(1, (int)(height * scale));
                var bitmap = new Bitmap(w, h, PixelFormat.Format32bppArgb);
                try
                {
                    BitmapData bits = bitmap.LockBits(new Rectangle(0, 0, w, h), ImageLockMode.WriteOnly, PixelFormat.Format32bppArgb);
                    try
                    {
                        IntPtr nativeBitmap = FPDFBitmap_CreateEx(w, h, 4, bits.Scan0, bits.Stride);
                        if (nativeBitmap == IntPtr.Zero) throw new InvalidDataException("PDF 页面内存不足。");
                        try
                        {
                            FPDFBitmap_FillRect(nativeBitmap, 0, 0, w, h, 0xFFFFFFFF);
                            FPDF_RenderPageBitmap(nativeBitmap, page, 0, 0, w, h, 0, 1 | (gray ? 8 : 0) | (printing ? 0x800 : 0));
                        }
                        finally { FPDFBitmap_Destroy(nativeBitmap); }
                    }
                    finally { bitmap.UnlockBits(bits); }
                    return bitmap;
                }
                catch { bitmap.Dispose(); throw; }
            }
            finally { FPDF_ClosePage(page); }
        }

        public void Dispose()
        {
            if (document != IntPtr.Zero) { FPDF_CloseDocument(document); document = IntPtr.Zero; }
            if (fileAccess != IntPtr.Zero) { Marshal.FreeHGlobal(fileAccess); fileAccess = IntPtr.Zero; }
            if (input != null) { input.Dispose(); input = null; }
            GC.KeepAlive(callback);
        }

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true, EntryPoint = "LoadLibraryExW")]
        private static extern IntPtr LoadLibraryEx(string path, IntPtr file, int flags);
        // PDFium's Windows x86 FPDF_CALLCONV is __stdcall, not cdecl.
        [DllImport("pdfium.dll", CallingConvention = CallingConvention.StdCall)] private static extern void FPDF_InitLibrary();
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        private delegate int GetBlock(IntPtr parameter, uint position, IntPtr destination, uint count);
        [StructLayout(LayoutKind.Sequential)]
        private struct NativeFileAccess { internal uint Length; internal IntPtr GetBlock, Parameter; }
        [DllImport("pdfium.dll", CallingConvention = CallingConvention.StdCall)] private static extern IntPtr FPDF_LoadCustomDocument(IntPtr access, IntPtr password);
        [DllImport("pdfium.dll", CallingConvention = CallingConvention.StdCall)] private static extern int FPDF_GetPageCount(IntPtr document);
        [DllImport("pdfium.dll", CallingConvention = CallingConvention.StdCall)] private static extern IntPtr FPDF_LoadPage(IntPtr document, int page);
        [DllImport("pdfium.dll", CallingConvention = CallingConvention.StdCall)] private static extern double FPDF_GetPageWidth(IntPtr page);
        [DllImport("pdfium.dll", CallingConvention = CallingConvention.StdCall)] private static extern double FPDF_GetPageHeight(IntPtr page);
        [DllImport("pdfium.dll", CallingConvention = CallingConvention.StdCall)] private static extern IntPtr FPDFBitmap_CreateEx(int width, int height, int format, IntPtr buffer, int stride);
        [DllImport("pdfium.dll", CallingConvention = CallingConvention.StdCall)] private static extern void FPDFBitmap_FillRect(IntPtr bitmap, int x, int y, int width, int height, uint color);
        [DllImport("pdfium.dll", CallingConvention = CallingConvention.StdCall)] private static extern void FPDF_RenderPageBitmap(IntPtr bitmap, IntPtr page, int x, int y, int width, int height, int rotate, int flags);
        [DllImport("pdfium.dll", CallingConvention = CallingConvention.StdCall)] private static extern void FPDFBitmap_Destroy(IntPtr bitmap);
        [DllImport("pdfium.dll", CallingConvention = CallingConvention.StdCall)] private static extern void FPDF_ClosePage(IntPtr page);
        [DllImport("pdfium.dll", CallingConvention = CallingConvention.StdCall)] private static extern void FPDF_CloseDocument(IntPtr document);
    }

    internal static class Raster
    {
        internal static ImageAttributes Attributes(bool gray)
        {
            var attributes = new ImageAttributes();
            if (gray) attributes.SetColorMatrix(new ColorMatrix(new[] {
                new[] { .299f, .299f, .299f, 0, 0 }, new[] { .587f, .587f, .587f, 0, 0 },
                new[] { .114f, .114f, .114f, 0, 0 }, new[] { 0f, 0, 0, 1, 0 }, new[] { 0f, 0, 0, 0, 1 }
            }));
            return attributes;
        }
        internal static void Jpeg(Image image, string output)
        {
            using (var parameters = new EncoderParameters(1))
            {
                parameters.Param[0] = new EncoderParameter(System.Drawing.Imaging.Encoder.Quality, 90L);
                image.Save(output, ImageCodecInfo.GetImageEncoders().First(c => c.MimeType == "image/jpeg"), parameters);
            }
        }
        internal static void SaveScan(Bitmap bitmap, string output, ScanOptions options)
        {
            bitmap.SetResolution(options.dpi, options.dpi);
            if (options.format == "png") bitmap.Save(output, ImageFormat.Png);
            else if (options.format == "jpg") Jpeg(bitmap, output);
            else
            {
                string jpeg = output + ".jpg";
                try { Jpeg(bitmap, jpeg); ScanPdf(File.ReadAllBytes(jpeg), bitmap.Width, bitmap.Height, options.dpi, output); }
                finally { Json.TryDelete(jpeg); }
            }
        }
        internal static void ScanPdf(byte[] jpeg, int width, int height, int dpi, string output)
        {
            string w = (width * 72.0 / dpi).ToString("F3", CultureInfo.InvariantCulture);
            string h = (height * 72.0 / dpi).ToString("F3", CultureInfo.InvariantCulture);
            string content = "q " + w + " 0 0 " + h + " 0 0 cm /Im0 Do Q";
            using (var stream = new FileStream(output, FileMode.CreateNew, FileAccess.Write, FileShare.None))
            {
                Action<string> write = text => { byte[] bytes = Encoding.ASCII.GetBytes(text); stream.Write(bytes, 0, bytes.Length); };
                write("%PDF-1.4\n");
                var offsets = new long[6];
                string[] objects = {
                    "<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 " + w + " " + h + "] /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>"
                };
                for (int i = 0; i < objects.Length; i++) { offsets[i + 1] = stream.Position; write((i + 1) + " 0 obj\n" + objects[i] + "\nendobj\n"); }
                offsets[4] = stream.Position;
                write("4 0 obj\n<< /Type /XObject /Subtype /Image /Width " + width + " /Height " + height + " /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length " + jpeg.Length + " >>\nstream\n");
                stream.Write(jpeg, 0, jpeg.Length);
                write("\nendstream\nendobj\n");
                offsets[5] = stream.Position;
                write("5 0 obj\n<< /Length " + content.Length + " >>\nstream\n" + content + "\nendstream\nendobj\n");
                long xref = stream.Position;
                write("xref\n0 6\n0000000000 65535 f \n");
                for (int i = 1; i < offsets.Length; i++) write(offsets[i].ToString("D10", CultureInfo.InvariantCulture) + " 00000 n \n");
                write("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n" + xref + "\n%%EOF\n");
                stream.Flush(true);
            }
        }
    }
}
