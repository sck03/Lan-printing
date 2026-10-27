using System;
using System.IO;
using System.Text;
using System.Text.RegularExpressions;

namespace LanPrint.Legacy
{
    internal static class Multipart
    {
        internal static string Extract(string body, string contentType, string directory, string id, long maximum)
        {
            if (!contentType.StartsWith("multipart/form-data;", StringComparison.OrdinalIgnoreCase))
                throw new ApiError(415, "请选择文件上传。");
            MatchCollection boundaries = Regex.Matches(contentType, "(?:^|;)\\s*boundary=(?:\"([^\"]+)\"|([^;\\s]+))", RegexOptions.IgnoreCase);
            if (boundaries.Count != 1) throw new ApiError(400, "上传边界无效。");
            string boundary = boundaries[0].Groups[1].Success ? boundaries[0].Groups[1].Value : boundaries[0].Groups[2].Value;
            if (boundary.Length < 1 || boundary.Length > 70 || !Regex.IsMatch(boundary, "^[0-9A-Za-z'()+_,./:=? -]+$") || boundary.EndsWith(" "))
                throw new ApiError(400, "上传边界无效。");
            using (var stream = new FileStream(body, FileMode.Open, FileAccess.Read, FileShare.Read))
            {
                int headerSize = 0;
                if (Line(stream, ref headerSize) != "--" + boundary) throw new ApiError(400, "上传内容不完整。");
                string disposition = null;
                while (true)
                {
                    string line = Line(stream, ref headerSize);
                    if (line == "") break;
                    if (line.StartsWith("Content-Disposition:", StringComparison.OrdinalIgnoreCase))
                    {
                        if (disposition != null) throw new ApiError(400, "上传字段重复。");
                        disposition = line.Substring(20).Trim();
                    }
                    else if (!line.StartsWith("Content-Type:", StringComparison.OrdinalIgnoreCase)) throw new ApiError(400, "上传字段无效。");
                }
                if (disposition == null || !disposition.StartsWith("form-data;", StringComparison.OrdinalIgnoreCase)
                    || !Regex.IsMatch(disposition, "(?:^|;)\\s*name=\"file\"(?:;|$)")) throw new ApiError(400, "每次请求只能上传一个文件。");
                Match filename = Regex.Match(disposition, "(?:^|;)\\s*filename=\"([^\"]*)\"");
                if (!filename.Success) throw new ApiError(400, "缺少上传文件名。");
                string name = filename.Groups[1].Value.Replace("\\", "/");
                name = name.Substring(name.LastIndexOf('/') + 1);
                name = Regex.Replace(name, "[\\x00-\\x1f\\x7f]", "");
                string extension = Store.Extension(name);
                if (name.Length > 180) name = name.Substring(0, 170) + "." + extension;
                long start = stream.Position;
                byte[] end = Encoding.ASCII.GetBytes("\r\n--" + boundary + "--\r\n");
                if (stream.Length - start <= end.Length) throw new ApiError(400, "文件为空或上传未完成。");
                stream.Position = stream.Length - end.Length;
                byte[] tail = new byte[end.Length];
                Json.ReadExactly(stream, tail, 0, tail.Length);
                if (Encoding.ASCII.GetString(tail) != Encoding.ASCII.GetString(end)) throw new ApiError(400, "上传结束边界无效。");
                long length = stream.Length - start - end.Length;
                if (length > maximum) throw new ApiError(413, "文件超过大小限制。");
                stream.Position = start;
                string output = Path.Combine(directory, id + "." + extension);
                try
                {
                    using (var file = new FileStream(output, FileMode.CreateNew, FileAccess.Write, FileShare.None))
                        CopyPart(stream, file, length, Encoding.ASCII.GetBytes("\r\n--" + boundary));
                }
                catch { Json.TryDelete(output); throw; }
                return name;
            }
        }
        private static string Line(Stream stream, ref int total)
        {
            using (var bytes = new MemoryStream())
            {
                while (true)
                {
                    int value = stream.ReadByte();
                    if (value < 0 || ++total > 16 * 1024) throw new ApiError(400, "上传字段不完整或过长。");
                    if (value == '\r')
                    {
                        if (stream.ReadByte() != '\n') throw new ApiError(400, "上传换行无效。");
                        return Json.Utf8.GetString(bytes.ToArray());
                    }
                    if (value == '\n' || value == 0) throw new ApiError(400, "上传字段无效。");
                    bytes.WriteByte((byte)value);
                }
            }
        }
        private static void CopyPart(Stream input, Stream output, long length, byte[] marker)
        {
            var prefix = new int[marker.Length];
            for (int i = 1, j = 0; i < marker.Length; i++)
            {
                while (j > 0 && marker[i] != marker[j]) j = prefix[j - 1];
                if (marker[i] == marker[j]) j++;
                prefix[i] = j;
            }
            byte[] buffer = new byte[64 * 1024];
            int matched = 0;
            while (length > 0)
            {
                int n = input.Read(buffer, 0, (int)Math.Min(buffer.Length, length));
                if (n == 0) throw new ApiError(400, "上传未完成。");
                for (int i = 0; i < n; i++)
                {
                    while (matched > 0 && buffer[i] != marker[matched]) matched = prefix[matched - 1];
                    if (buffer[i] == marker[matched]) matched++;
                    if (matched == marker.Length) throw new ApiError(400, "每次请求只能上传一个文件。");
                }
                output.Write(buffer, 0, n);
                length -= n;
            }
        }
    }
}
