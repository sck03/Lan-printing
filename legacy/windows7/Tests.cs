using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Drawing.Printing;
using System.IO;
using System.Linq;
using System.Text;

namespace LanPrint.Legacy
{
    internal static class Tests
    {
        private static void Assert(bool value, string message) { if (!value) throw new Exception(message); }
        private static void Reject(Action action)
        {
            try { action(); } catch (InvalidDataException) { return; } catch (IOException) { return; } catch (ArgumentException) { return; }
            catch (System.ComponentModel.Win32Exception) { return; }
            throw new Exception("Expected operation to be rejected");
        }
        [STAThread]
        internal static int Main(string[] args)
        {
            if (args.Length > 0 && args[0] == "--worker") return Program.Main(args);
            string root = Path.Combine(Path.GetTempPath(), "lanprint-legacy-tests-" + Guid.NewGuid().ToString("D"));
            Directory.CreateDirectory(root);
            try
            {
                Assert(!Environment.Is64BitProcess, "Tests must actually execute in an x86 process");
                Signature(); Models(); Persistence(root); DriverProfiles(root); Images(root); LargePdf(root); Framing(root);
                if (args.Length == 1)
                {
                    TestLicense fixture = Json.Read<TestLicense>(args[0], 64 * 1024);
                    LicenseClaims claims = Licensing.Verify(fixture.code, fixture.machine, Security.Hex(fixture.public_key));
                    Assert(claims.customer == "测试办公室", "Signed claims must remain compatible");
                    Reject(() => Licensing.Verify(fixture.code, Licensing.MachineCode("other"), Security.Hex(fixture.public_key)));
                }
                Console.WriteLine("PASS: x86 execution, strict verification/JSON, ownership/quotas, persistence/restart, driver presets, 300/600 DPI, images, native PDF and 100 MB streaming");
                return 0;
            }
            catch (Exception error) { Console.Error.WriteLine(error); return 1; }
            finally { Json.DeleteScratch(root); }
        }
        internal sealed class TestLicense { public string code, machine, public_key; }
        private static void Signature()
        {
            // Independent published RFC 8032 vectors, not a self-generated round trip.
            byte[] key = Security.Hex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
            byte[] signature = Security.Hex("e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b");
            Assert(Ed25519.Verify(new byte[0], signature, key), "RFC 8032 empty-message vector");
            Assert(!Ed25519.Verify(new byte[] { 1 }, signature, key), "Changed message must be rejected");
            signature[63] |= 0x80;
            Assert(!Ed25519.Verify(new byte[0], signature, key), "Noncanonical scalar must be rejected");
            key = Security.Hex("3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c");
            signature = Security.Hex("92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00");
            Assert(Ed25519.Verify(new byte[] { 0x72 }, signature, key), "RFC 8032 one-byte vector");
            byte[] identity = new byte[32]; identity[0] = 1;
            Assert(!Ed25519.Verify(new byte[0], new byte[64], identity), "Small-order public key must be rejected");
            Assert(Licensing.MachineCode(" ABC ") == Licensing.MachineCode("abc"), "Machine normalization");
            Assert(Launch.Quote(@"C:\打印 站\") == "\"C:\\打印 站\\\\\"", "Windows trailing-backslash quoting");
        }
        private static void Models()
        {
            Assert(new Config().max_upload_mb == 100, "Default upload limit must be 100 MB");
            new Config { max_upload_mb = 200 }.Validate();
            Reject(() => new Config { max_upload_mb = 201 }.Validate());
            Assert(PrintOptions.Ranges("1-3,2,5", 5).SequenceEqual(new[] { 0, 1, 2, 4 }), "Page range ordering/deduplication");
            foreach (string range in new[] { "0", "3-1", "1,", "1-6", "foo", "-2", "1-2-3" }) Reject(() => PrintOptions.Ranges(range, 5));
            foreach (string json in new[] { "{\"unknown\":1}", "{\"copies\":1.5}", "{\"copies\":\"2\"}", "{\"color\":1}" })
                Reject(() => Json.Parse<PrintOptions>(Encoding.UTF8.GetBytes(json)));
            Assert(Json.Parse<PrintOptions>(Encoding.UTF8.GetBytes("{\"printer\":\"demo\"}")).render_dpi == 300, "Old options use defaults");
            Devices devices = Backend.Devices(true, true);
            new PrintOptions { printer = "demo-printer", render_dpi = 600 }.Validate(devices, 1);
            Reject(() => new PrintOptions { printer = "demo-printer", render_dpi = 1200 }.Validate(devices, 1));
            Reject(() => new PrintOptions { printer = "demo-printer", profile_id = "../invalid" }.Validate(devices, 1));
        }
        private static void DriverProfiles(string root)
        {
            byte[] mode = new byte[224]; mode[68] = 220; mode[70] = 4;
            mode[220] = 1; mode[221] = 2; mode[222] = 3; mode[223] = 4;
            var fixture = new DriverProfile { id = Guid.NewGuid().ToString("D"), name = "照片纸 · 高质量 · 关闭高速",
                printer = "demo-printer", driver = "test", devmode = Convert.ToBase64String(mode) };
            Profiles.Save(root, fixture);
            Assert(Profiles.Load(root, fixture.id, fixture.printer).devmode == fixture.devmode, "Preserve private driver bytes on disk");
            Assert(Profiles.List(root).Length == 1, "List stored presets");
            Reject(() => Profiles.Load(root, fixture.id, "other-printer"));
            Reject(() => Profiles.Remove(root, "../config"));
            fixture.devmode = "invalid-base64"; Reject(() => Profiles.Save(root, fixture));
            mode[70] = 5; fixture.devmode = Convert.ToBase64String(mode); Reject(() => Profiles.Save(root, fixture));
            Profiles.Remove(root, fixture.id); Assert(Profiles.List(root).Length == 0, "Remove idle preset");
            string printer = Environment.GetEnvironmentVariable("LANPRINT_TEST_DRIVER");
            if (!String.IsNullOrEmpty(printer))
            {
                DriverProfile profile = Profiles.Capture(printer, false);
                profile.name = "Installed driver test"; Profiles.Save(root, profile);
                DriverProfile loaded = Profiles.Load(root, profile.id, printer);
                Assert(loaded.devmode == profile.devmode, "Installed vendor data survives persistence");
                using (var document = new PrintDocument())
                {
                    document.PrinterSettings.PrinterName = printer;
                    Profiles.Apply(document, loaded);
                    loaded.driver += "-changed"; Reject(() => Profiles.Apply(document, loaded));
                }
                Console.WriteLine("PASS: installed driver capture/restore, private bytes=" + (Convert.FromBase64String(profile.devmode).Length - 220));
                Profiles.Remove(root, profile.id);
            }
        }
        private static Job NewJob(string owner, string request, string file)
        {
            return new Job { id = Guid.NewGuid().ToString("D"), owner = owner, request_id = request, file_id = file,
                kind = "print", name = "test", status = "queued", message = "waiting", created_at = Store.Now() };
        }
        private static void Persistence(string root)
        {
            string directory = Path.Combine(root, "store");
            var store = new Store(directory);
            string owner = Guid.NewGuid().ToString("D"), other = Guid.NewGuid().ToString("D"), request = Guid.NewGuid().ToString("D");
            var file = new FileRecord { id = Guid.NewGuid().ToString("D"), owner = owner, name = "test.png", extension = "png", bytes = 1, pages = 1, created_at = Store.Now(), expires_at = Store.Now() + 60 };
            File.WriteAllBytes(store.FilePath(file), new byte[] { 1 }); store.Insert(file, 1);
            Job first = store.Enqueue(NewJob(owner, request, file.id), 2);
            Assert(store.Enqueue(NewJob(owner, request, file.id), 2).id == first.id, "Retries must return the accepted job");
            Reject(() => store.Existing(owner, request, "scan"));
            Reject(() => store.Owned(file.id, other));
            Reject(() => store.Cancel(first.id, other));
            file.expires_at = 0; store.Save(); store.Cleanup();
            Assert(File.Exists(store.FilePath(file)), "Queued input must survive expiry");
            using (store.Lease(file.id, owner, 30)) { Reject(() => store.Remove(file.id, owner)); }
            var reopened = new Store(directory);
            Assert(reopened.Db.jobs.Single().status == "interrupted", "Restart must never replay a print");
            Assert(reopened.Existing(owner, request, "print").id == first.id, "Restart keeps idempotency");
            Assert(reopened.Db.files.Count == 1, "Committed files survive restart");
            file = reopened.Db.files.Single();
            long expiry = file.expires_at;
            using (var locked = new FileStream(Path.Combine(directory, "state.json"), FileMode.Open, FileAccess.ReadWrite, FileShare.None))
            {
                Reject(() => reopened.Lease(file.id, owner, 30));
                Assert(file.expires_at == expiry, "Failed persistence must roll back expiry");
                Reject(() => reopened.Remove(file.id, owner));
                Assert(File.Exists(reopened.FilePath(file)), "Failed deletion persistence must preserve bytes");
                Assert(reopened.Db.files.Count == 1, "Failed deletion persistence must preserve metadata");
            }
            Assert(!File.Exists(Path.Combine(directory, "state.json.tmp")), "Failed writes must clean scratch JSON");
            file.bytes = 1024 * 1024; Reject(() => reopened.Capacity(1, 1));
            reopened.Remove(file.id, owner); Assert(reopened.Db.files.Count == 0, "Deletion commits");
            File.WriteAllText(Path.Combine(directory, "state.json"), "{bad-json");
            Reject(() => new Store(directory));
            Assert(File.ReadAllText(Path.Combine(directory, "state.json")) == "{bad-json", "Corrupt state must not be overwritten");
        }
        private static void Images(string root)
        {
            string path = Path.Combine(root, "透明图片.png");
            using (var image = new Bitmap(80, 100, PixelFormat.Format32bppArgb))
            {
                using (Graphics graphics = Graphics.FromImage(image)) { graphics.Clear(Color.Transparent); graphics.FillRectangle(Brushes.Red, 10, 10, 20, 20); }
                image.Save(path, ImageFormat.Png);
            }
            using (var document = new Document(path))
            using (Bitmap image = document.Render(0, 1400, false, false))
            {
                Assert(image.GetPixel(0, 0).R == 255, "Transparent image prints on white");
                Assert(image.GetPixel(20, 20).R > 200 && image.GetPixel(20, 20).G < 50, "Color content survives rendering");
                string pdf = Path.Combine(root, "扫描.pdf");
                Raster.SaveScan(image, pdf, new ScanOptions { dpi = 200, format = "pdf" });
                using (var scanned = new Document(pdf))
                {
                    using (Bitmap preview = scanned.Render(0, 1400, false, false)) Assert(scanned.Pages == 1 && preview.Height == 1400, "Scanned PDF opens and renders natively");
                    using (Bitmap high = scanned.Render(0, 7016, false, true)) Assert(high.Height == 7016, "600 DPI PDF rendering is available in x86");
                    Reject(() => { using (Bitmap invalid = scanned.Render(0, 7017, false, true)) { } });
                }
                if (Environment.GetEnvironmentVariable("LANPRINT_TEST_VIRTUAL_PRINT") == "1")
                {
                    DriverProfile profile = Profiles.Capture("Microsoft Print to PDF", false);
                    byte[] mode = Convert.FromBase64String(profile.devmode);
                    BitConverter.GetBytes(BitConverter.ToInt32(mode, 72) | 0x1903).CopyTo(mode, 72);
                    BitConverter.GetBytes((short)2).CopyTo(mode, 76); // Landscape
                    BitConverter.GetBytes((short)11).CopyTo(mode, 78); // A5
                    BitConverter.GetBytes((short)7).CopyTo(mode, 86); // Must normalize to one driver copy
                    BitConverter.GetBytes((short)2).CopyTo(mode, 92); // Color
                    BitConverter.GetBytes((short)1).CopyTo(mode, 94); // Simplex
                    profile.devmode = Convert.ToBase64String(mode);
                    string printed = Path.Combine(root, "virtual-print.pdf");
                    Backend.Print(pdf, new PrintOptions { printer = profile.printer, profile_id = profile.id, copies = 2,
                        landscape = false, color = false, paper = "A4", render_dpi = 600 }, false, printed, profile);
                    // Print() submits to the spooler; the virtual driver writes
                    // and releases its output asynchronously.
                    bool ready = false;
                    for (int i = 0; i < 200; i++)
                    {
                        try { using (var probe = File.Open(printed, FileMode.Open, FileAccess.Read, FileShare.None)) ready = probe.Length > 0; }
                        catch (IOException) { }
                        if (ready) break;
                        System.Threading.Thread.Sleep(100);
                    }
                    Assert(ready, "Virtual spooler must finish writing its output");
                    using (var result = new Document(printed))
                    using (Bitmap page = result.Render(0, 1400, false, false))
                    {
                        Assert(result.Pages == 2 && page.Width > page.Height, "Preset overrides orientation and does not multiply copies");
                        bool red = false;
                        for (int y = 0; y < page.Height && !red; y += 5)
                            for (int x = 0; x < page.Width && !red; x += 5)
                            { Color pixel = page.GetPixel(x, y); red = pixel.R > 180 && pixel.G < 80 && pixel.B < 80; }
                        Assert(red, "Preset color overrides generic grayscale");
                    }
                    Console.WriteLine("PASS: Microsoft Print to PDF preset color/orientation override, two copies and 600 DPI");
                }
            }
            string bad = Path.Combine(root, "bad.pdf"); File.WriteAllText(bad, "not a PDF");
            Reject(() => { using (var doc = new Document(bad)) { } });
        }
        private static void LargePdf(string root)
        {
            string path = Path.Combine(root, "100MB-中文文档.pdf");
            using (var stream = new FileStream(path, FileMode.CreateNew, FileAccess.Write))
            {
                Action<string> write = text => { byte[] bytes = Encoding.ASCII.GetBytes(text); stream.Write(bytes, 0, bytes.Length); };
                write("%PDF-1.4\n");
                byte[] padding = Enumerable.Repeat((byte)' ', 64 * 1024).ToArray(); padding[0] = (byte)'%'; padding[padding.Length - 1] = (byte)'\n';
                for (int i = 0; i < 1600; i++) stream.Write(padding, 0, padding.Length);
                var offsets = new long[5];
                string[] objects = { "<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>", "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Contents 4 0 R >>", "<< /Length 0 >>\nstream\n\nendstream" };
                for (int i = 0; i < objects.Length; i++) { offsets[i + 1] = stream.Position; write((i + 1) + " 0 obj\n" + objects[i] + "\nendobj\n"); }
                long xref = stream.Position;
                write("xref\n0 5\n0000000000 65535 f \n");
                for (int i = 1; i < offsets.Length; i++) write(offsets[i].ToString("D10") + " 00000 n \n");
                write("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n" + xref + "\n%%EOF\n");
            }
            using (var document = new Document(path))
            using (Bitmap preview = document.Render(0, 1400, false, false)) Assert(document.Pages == 1 && preview.Width > 0, "100 MB PDF must load through random-access callbacks");
        }
        private static void Framing(string root)
        {
            string body = Path.Combine(root, "multipart");
            string marker = "LanPrintBoundary";
            string header = "--" + marker + "\r\nContent-Disposition: form-data; name=\"file\"; filename=\"中文.pdf\"\r\n\r\n";
            File.WriteAllBytes(body, Encoding.UTF8.GetBytes(header + "data\r\n--" + marker + "--\r\n"));
            string id = Guid.NewGuid().ToString("D");
            Assert(Multipart.Extract(body, "multipart/form-data; boundary=" + marker, root, id, 100) == "中文.pdf", "Unicode multipart filename");
            Assert(File.ReadAllText(Path.Combine(root, id + ".pdf")) == "data", "Multipart copies exactly the file bytes");
            File.WriteAllBytes(body, Encoding.UTF8.GetBytes(header + "data\r\n--" + marker + "\r\nother\r\n--" + marker + "--\r\n"));
            try { Multipart.Extract(body, "multipart/form-data; boundary=" + marker, root, Guid.NewGuid().ToString("D"), 100); throw new Exception("Multiple parts accepted"); }
            catch (ApiError) { }
        }
    }
}
