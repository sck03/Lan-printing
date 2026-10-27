using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Text;
using System.Web.Script.Serialization;

namespace LanPrint.Legacy
{
    internal static class Json
    {
        internal static readonly UTF8Encoding Utf8 = new UTF8Encoding(false, true);

        private static JavaScriptSerializer Serializer()
        {
            return new JavaScriptSerializer { MaxJsonLength = 8 * 1024 * 1024, RecursionLimit = 32 };
        }

        internal static byte[] Bytes(object value)
        {
            return Utf8.GetBytes(Serializer().Serialize(value));
        }

        internal static T Parse<T>(byte[] bytes)
        {
            string text = Utf8.GetString(bytes);
            object value = Serializer().DeserializeObject(text);
            Check(value, typeof(T));
            return Serializer().Deserialize<T>(text);
        }

        // JavaScriptSerializer otherwise silently ignores unknown fields and
        // converts strings/fractions into numbers. Keep the API and config strict.
        private static void Check(object value, Type type)
        {
            Type nullable = Nullable.GetUnderlyingType(type);
            if (nullable != null) { if (value != null) Check(value, nullable); return; }
            if (value == null)
            {
                if (type.IsValueType) throw new InvalidDataException("JSON 字段不能为空。");
                return;
            }
            if (type == typeof(string))
            {
                if (!(value is string)) throw new InvalidDataException("JSON 字段应为文本。");
                return;
            }
            if (type == typeof(bool))
            {
                if (!(value is bool)) throw new InvalidDataException("JSON 字段应为布尔值。");
                return;
            }
            if (type == typeof(int) || type == typeof(long))
            {
                if (!(value is int) && !(value is long) && !(value is decimal) && !(value is double))
                    throw new InvalidDataException("JSON 字段应为整数。");
                decimal number = Convert.ToDecimal(value, CultureInfo.InvariantCulture);
                if (number != Decimal.Truncate(number) || number < Int64.MinValue || number > Int64.MaxValue
                    || (type == typeof(int) && (number < Int32.MinValue || number > Int32.MaxValue)))
                    throw new InvalidDataException("JSON 整数超出范围。");
                return;
            }
            if (type.IsGenericType && type.GetGenericTypeDefinition() == typeof(List<>))
            {
                var items = value as object[];
                if (items == null) throw new InvalidDataException("JSON 字段应为列表。");
                foreach (object item in items) Check(item, type.GetGenericArguments()[0]);
                return;
            }
            var dictionary = value as IDictionary<string, object>;
            if (dictionary == null) throw new InvalidDataException("JSON 格式错误。");
            foreach (var entry in dictionary)
            {
                FieldInfo field = type.GetField(entry.Key, BindingFlags.Public | BindingFlags.Instance);
                if (field == null) throw new InvalidDataException("未知 JSON 字段：" + entry.Key);
                Check(entry.Value, field.FieldType);
            }
        }

        internal static T Read<T>(string path, int limit)
        {
            using (var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read))
            {
                if (stream.Length > limit) throw new InvalidDataException("配置或状态文件过大。");
                byte[] bytes = new byte[(int)stream.Length];
                ReadExactly(stream, bytes, 0, bytes.Length);
                return Parse<T>(bytes);
            }
        }

        internal static void Write(string path, object value)
        {
            string temp = path + ".tmp";
            try
            {
                byte[] bytes = Bytes(value);
                using (var stream = new FileStream(temp, FileMode.Create, FileAccess.Write, FileShare.None))
                {
                    stream.Write(bytes, 0, bytes.Length);
                    stream.Flush(true);
                }
                if (!MoveFileEx(temp, path, 1 | 8)) throw new System.ComponentModel.Win32Exception();
            }
            finally { TryDelete(temp); }
        }

        internal static void ReadExactly(Stream stream, byte[] buffer, int offset, int count)
        {
            while (count > 0)
            {
                int n = stream.Read(buffer, offset, count);
                if (n == 0) throw new EndOfStreamException("请求或文件不完整。");
                offset += n;
                count -= n;
            }
        }

        internal static void Copy(Stream input, Stream output, long count)
        {
            byte[] buffer = new byte[64 * 1024];
            while (count > 0)
            {
                int n = input.Read(buffer, 0, (int)Math.Min(buffer.Length, count));
                if (n == 0) throw new EndOfStreamException("上传未完成。");
                output.Write(buffer, 0, n);
                count -= n;
            }
        }

        internal static void TryDelete(string path)
        {
            try { File.Delete(path); } catch (IOException) { } catch (UnauthorizedAccessException) { }
        }

        internal static void DeleteScratch(string path)
        {
            // Only callers with a generated, direct child of work/ use this.
            // Never follow a junction planted in a scratch directory.
            try
            {
                if (!Directory.Exists(path)) return;
                foreach (string file in Directory.GetFiles(path)) File.Delete(file);
                foreach (string directory in Directory.GetDirectories(path))
                {
                    if ((File.GetAttributes(directory) & FileAttributes.ReparsePoint) == 0)
                        DeleteScratch(directory);
                }
                Directory.Delete(path);
            }
            catch (IOException) { }
            catch (UnauthorizedAccessException) { }
        }

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true, EntryPoint = "MoveFileExW")]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool MoveFileEx(string from, string to, int flags);
    }
}
