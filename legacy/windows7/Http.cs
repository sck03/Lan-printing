using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Security.Cryptography;
using System.Text;
using System.Text.RegularExpressions;

namespace LanPrint.Legacy
{
    internal sealed class ApiError : Exception
    {
        internal readonly int Status;
        internal ApiError(int status, string message) : base(message) { Status = status; }
    }

    // One request per connection keeps framing unambiguous and works without an
    // HTTP.sys URL reservation/admin rights. Bodies spool to disk, not RAM.
    internal sealed class Http : IDisposable
    {
        private readonly TcpClient client;
        private readonly Stream output;
        internal readonly Stream Stream;
        internal readonly Dictionary<string, string> Headers = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        internal string Method, Target, Owner, Cookie;
        internal bool Local, Responded;
        private int headerBytes;

        internal Http(TcpClient client)
        {
            this.client = client;
            client.ReceiveTimeout = 30000;
            client.SendTimeout = 30000;
            output = client.GetStream();
            // Network streams cannot seek. A duplex BufferedStream cannot
            // switch to writing when a rejected request still has buffered
            // body bytes, so responses use the raw output stream separately.
            Stream = new BufferedStream(output, 64 * 1024);
        }
        internal void ReadHeaders()
        {
            string[] request = Line(8192).Split(' ');
            if (request.Length != 3 || request[2] != "HTTP/1.1" && request[2] != "HTTP/1.0"
                || !request[1].StartsWith("/", StringComparison.Ordinal) || request[1].StartsWith("//", StringComparison.Ordinal))
                throw new ApiError(400, "无效的 HTTP 请求。");
            Method = request[0]; Target = request[1];
            for (int count = 0; ; count++)
            {
                string line = Line(8192);
                if (line.Length == 0) break;
                int colon = line.IndexOf(':');
                if (count >= 64 || colon < 1 || !Regex.IsMatch(line.Substring(0, colon), "^[!#$%&'*+.^_`|~0-9A-Za-z-]+$"))
                    throw new ApiError(400, "HTTP 请求头无效。");
                string key = line.Substring(0, colon), value = line.Substring(colon + 1).Trim();
                if (Headers.ContainsKey(key) || value.IndexOf('\0') >= 0) throw new ApiError(400, "HTTP 请求头重复或无效。");
                Headers.Add(key, value);
            }
            if (Headers.ContainsKey("Content-Length") && Headers.ContainsKey("Transfer-Encoding"))
                throw new ApiError(400, "HTTP 请求体长度冲突。");
            IPAddress remote = ((IPEndPoint)client.Client.RemoteEndPoint).Address;
            Local = IPAddress.IsLoopback(remote);
            if (!Security.Private(remote)) throw new ApiError(403, "此服务仅允许内网访问。");
            if (Method != "GET" && Method != "HEAD" && Method != "POST" && Method != "DELETE")
                throw new ApiError(405, "不支持此请求方法。");
        }
        internal string Header(string name) { string value; return Headers.TryGetValue(name, out value) ? value : ""; }
        private string Line(int limit)
        {
            using (var bytes = new MemoryStream())
            {
                while (true)
                {
                    int b = Stream.ReadByte();
                    if (b < 0) throw new EndOfStreamException("HTTP 请求不完整。");
                    if (++headerBytes > 32 * 1024 || bytes.Length >= limit) throw new ApiError(400, "HTTP 请求头过长。");
                    if (b == '\r')
                    {
                        if (Stream.ReadByte() != '\n') throw new ApiError(400, "HTTP 换行无效。");
                        return Encoding.ASCII.GetString(bytes.ToArray());
                    }
                    if (b < 32 && b != '\t' || b > 126) throw new ApiError(400, "HTTP 请求头编码无效。");
                    bytes.WriteByte((byte)b);
                }
            }
        }
        internal void Body(string path, long limit)
        {
            long length = 0;
            string transfer = Header("Transfer-Encoding"), size = Header("Content-Length");
            if (transfer != "" && !transfer.Equals("chunked", StringComparison.OrdinalIgnoreCase))
                throw new ApiError(400, "不支持此请求体编码。");
            if (size != "" && (!Int64.TryParse(size, NumberStyles.None, CultureInfo.InvariantCulture, out length) || length > limit))
                throw new ApiError(413, "文件超过大小限制。");
            if (Header("Expect") != "")
            {
                if (!Header("Expect").Equals("100-continue", StringComparison.OrdinalIgnoreCase)) throw new ApiError(417, "请求预期无效。");
                byte[] interim = Encoding.ASCII.GetBytes("HTTP/1.1 100 Continue\r\n\r\n");
                output.Write(interim, 0, interim.Length); output.Flush();
            }
            using (var output = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None))
            {
                if (transfer == "") Json.Copy(Stream, output, length);
                else
                {
                    long total = 0;
                    while (true)
                    {
                        string chunk = Line(128).Split(';')[0];
                        if (!Int64.TryParse(chunk, NumberStyles.AllowHexSpecifier, CultureInfo.InvariantCulture, out length) || length < 0)
                            throw new ApiError(400, "HTTP 分块长度无效。");
                        if (length == 0)
                        {
                            if (Line(8192) != "") throw new ApiError(400, "不支持 HTTP 尾部字段。");
                            break;
                        }
                        if (length > limit - total) throw new ApiError(413, "文件超过大小限制。");
                        Json.Copy(Stream, output, length); total += length;
                        if (Stream.ReadByte() != '\r' || Stream.ReadByte() != '\n') throw new ApiError(400, "HTTP 分块不完整。");
                    }
                }
            }
        }
        internal T JsonBody<T>(string path)
        {
            if (!Header("Content-Type").Split(';')[0].Trim().Equals("application/json", StringComparison.OrdinalIgnoreCase))
                throw new ApiError(415, "请求应使用 JSON 格式。");
            Body(path, 64 * 1024);
            T body = Json.Read<T>(path, 64 * 1024);
            if (body == null) throw new ApiError(400, "请求内容为空。");
            return body;
        }
        internal void JsonResponse(object value, int status) { Bytes(Json.Bytes(value), "application/json; charset=utf-8", status); }
        internal void Bytes(byte[] bytes, string mime, int status)
        {
            Start(mime, status, bytes.Length, null);
            if (Method != "HEAD") output.Write(bytes, 0, bytes.Length);
            output.Flush();
        }
        internal void FileResponse(string path, string mime, string disposition)
        {
            using (var input = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read))
            {
                Start(mime, 200, input.Length, disposition);
                if (Method != "HEAD") Json.Copy(input, output, input.Length);
                output.Flush();
            }
        }
        private void Start(string mime, int status, long length, string disposition)
        {
            string reason = status == 200 ? "OK" : status == 403 ? "Forbidden" : status == 404 ? "Not Found" : "Error";
            var headers = new StringBuilder("HTTP/1.1 " + status + " " + reason + "\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n");
            headers.Append("Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' blob:; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'\r\n");
            headers.Append("Content-Type: " + mime + "\r\nContent-Length: " + length.ToString(CultureInfo.InvariantCulture) + "\r\n");
            if (Cookie != null) headers.Append("Set-Cookie: " + Cookie + "\r\n");
            if (disposition != null) headers.Append("Content-Disposition: " + disposition + "\r\n");
            headers.Append("\r\n");
            byte[] bytes = Encoding.ASCII.GetBytes(headers.ToString());
            Responded = true;
            output.Write(bytes, 0, bytes.Length);
        }
        public void Dispose()
        {
            try { Stream.Dispose(); } catch (IOException) { } catch (SocketException) { }
            finally { client.Close(); }
        }
    }

    internal static class Security
    {
        internal static byte[] Hex(string value)
        {
            if (value.Length % 2 != 0) throw new FormatException("十六进制格式错误。");
            var bytes = new byte[value.Length / 2];
            for (int i = 0; i < bytes.Length; i++) bytes[i] = Byte.Parse(value.Substring(i * 2, 2), NumberStyles.AllowHexSpecifier, CultureInfo.InvariantCulture);
            return bytes;
        }
        internal static string RandomHex(int length)
        {
            byte[] bytes = new byte[length];
            using (var random = new RNGCryptoServiceProvider()) random.GetBytes(bytes);
            return BitConverter.ToString(bytes).Replace("-", "").ToLowerInvariant();
        }
        internal static bool Private(IPAddress ip)
        {
            if (IPAddress.IsLoopback(ip)) return true;
            byte[] bytes = ip.GetAddressBytes();
            if (bytes.Length != 4) return false; // Listener is IPv4-only.
            return bytes[0] == 10 || bytes[0] == 172 && bytes[1] >= 16 && bytes[1] <= 31
                || bytes[0] == 192 && bytes[1] == 168 || bytes[0] == 169 && bytes[1] == 254;
        }
        internal static string Sign(Config config, string text)
        {
            using (var mac = new HMACSHA256(Encoding.UTF8.GetBytes(config.secret)))
                return BitConverter.ToString(mac.ComputeHash(Encoding.UTF8.GetBytes(text))).Replace("-", "").ToLowerInvariant();
        }
        private static bool Same(string left, string right)
        {
            if (right == null || left.Length != right.Length) return false;
            int difference = 0;
            for (int i = 0; i < left.Length; i++) difference |= left[i] ^ right[i];
            return difference == 0;
        }
        internal static void Check(Http http, Config config)
        {
            string host = http.Header("Host");
            Uri authority;
            IPAddress ip;
            if (!Uri.TryCreate("http://" + host + "/", UriKind.Absolute, out authority) || authority.UserInfo != "" || authority.AbsolutePath != "/"
                || host.IndexOfAny(new[] { '/', '\\', '?', '#' }) >= 0 || authority.HostNameType == UriHostNameType.Unknown
                || !(authority.Host.Equals("localhost", StringComparison.OrdinalIgnoreCase)
                    || authority.Host.Equals(Environment.MachineName, StringComparison.OrdinalIgnoreCase)
                    || IPAddress.TryParse(authority.Host, out ip) && Private(ip))) throw new ApiError(403, "请使用主机的内网地址访问。");
            string owner = null;
            foreach (string cookie in http.Header("Cookie").Split(';'))
            {
                string value = cookie.Trim();
                if (!value.StartsWith("lp_session=", StringComparison.Ordinal)) continue;
                string[] parts = value.Substring(11).Split('.');
                if (parts.Length == 2 && Store.ValidId(parts[0]) && Same(Sign(config, parts[0]), parts[1])) owner = parts[0];
            }
            bool fresh = owner == null;
            if (fresh) owner = Guid.NewGuid().ToString("D");
            http.Owner = owner;
            if (fresh) http.Cookie = "lp_session=" + owner + "." + Sign(config, owner) + "; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000";
            if (http.Method != "GET" && http.Method != "HEAD")
            {
                if (fresh || !Same(Sign(config, "csrf:" + owner), http.Header("x-csrf-token"))) throw new ApiError(403, "会话已失效，请刷新页面。");
                if (http.Header("Origin") != "" && http.Header("Origin") != "http://" + host) throw new ApiError(403, "不允许跨站提交。");
            }
        }
    }
}
