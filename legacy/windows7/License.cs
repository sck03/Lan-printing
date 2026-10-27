using System;
using System.IO;
using System.Numerics;
using System.Security.Cryptography;
using System.Text;
using System.Text.RegularExpressions;
using Microsoft.Win32;

namespace LanPrint.Legacy
{
    internal sealed class LicenseClaims
    {
        public int version;
        public string product, machine, customer;
        public long issued_at;
    }
    internal sealed class SavedLicense { public string code; }

    internal sealed class Licensing
    {
        internal readonly string Machine;
        private readonly string root;
        private readonly byte[] publicKey;
        private readonly object sync = new object();
        private string verifiedCode;
        private LicenseClaims verified;

        internal Licensing(string root, byte[] publicKey)
        {
            this.root = root;
            this.publicKey = publicKey;
            using (RegistryKey baseKey = RegistryKey.OpenBaseKey(RegistryHive.LocalMachine, RegistryView.Registry64))
            using (RegistryKey key = baseKey.OpenSubKey(@"SOFTWARE\Microsoft\Cryptography"))
            {
                Guid guid;
                if (key == null || !Guid.TryParse(key.GetValue("MachineGuid") as string, out guid))
                    throw new InvalidDataException("无法读取 Windows MachineGuid，请检查注册表权限。");
                Machine = MachineCode(guid.ToString("D"));
            }
        }
        internal static string MachineCode(string identity)
        {
            using (var hash = SHA256.Create())
            {
                byte[] digest = hash.ComputeHash(Encoding.UTF8.GetBytes("LanPrint machine v1:" + identity.Trim().ToLowerInvariant()));
                return "LP1-" + BitConverter.ToString(digest, 0, 16).Replace("-", "");
            }
        }
        internal string Code()
        {
            SavedLicense saved = Json.Read<SavedLicense>(Path.Combine(root, "license.json"), 5120);
            if (saved == null || saved.code == null) throw new InvalidDataException("授权文件损坏，请重新注册。");
            return saved.code;
        }
        internal LicenseClaims Load()
        {
            // Re-read on every protected request. Cache only the cryptographic
            // result for identical bytes; removal or modification locks the host.
            lock (sync)
            {
                string code = Code();
                if (verifiedCode != code)
                {
                    verified = Verify(code, Machine, publicKey);
                    verifiedCode = code;
                }
                return verified;
            }
        }
        internal bool Registered()
        {
            try { Load(); return true; }
            catch (InvalidDataException) { return false; }
            catch (IOException) { return false; }
            catch (UnauthorizedAccessException) { return false; }
            catch (ArgumentException) { return false; }
            catch (InvalidOperationException) { return false; }
            catch (FormatException) { return false; }
            catch (OverflowException) { return false; }
        }
        internal LicenseClaims Activate(string code)
        {
            lock (sync)
            {
                LicenseClaims claims = Verify(code, Machine, publicKey);
                string compact = Regex.Replace(code, "[\\t\\n\\v\\f\\r ]", "");
                Json.Write(Path.Combine(root, "license.json"), new SavedLicense { code = compact });
                verifiedCode = compact;
                verified = claims;
                return claims;
            }
        }
        internal static LicenseClaims Verify(string code, string machine, byte[] key)
        {
            if (code == null || Encoding.UTF8.GetByteCount(code) > 4096) throw new InvalidDataException("注册码为空或过长。");
            string[] parts = Regex.Replace(code, "[\\t\\n\\v\\f\\r ]", "").Split('.');
            if (parts.Length != 3 || parts[0] != "LP1") throw new InvalidDataException("请粘贴完整的 LP1. 注册码。");
            byte[] payload = Base64Url(parts[1]), signature = Base64Url(parts[2]);
            if (!Ed25519.Verify(payload, signature, key)) throw new InvalidDataException("注册码签名无效，可能有误或已被修改。");
            LicenseClaims claims = Json.Parse<LicenseClaims>(payload);
            if (claims == null || claims.version != 1 || claims.product != "lanprint")
                throw new InvalidDataException("注册码不适用于当前产品或版本。");
            if (!Regex.IsMatch(machine, "^LP1-[0-9A-F]{32}$") || claims.machine != machine)
                throw new InvalidDataException("注册码与本机不匹配，请使用本机机器码申请。");
            string customer = claims.customer == null ? "" : claims.customer.Trim();
            int count = 0;
            foreach (char c in customer)
            {
                if (Char.IsControl(c)) throw new InvalidDataException("授权名称包含控制字符。");
                if (!Char.IsLowSurrogate(c)) count++;
            }
            if (count < 1 || count > 80 || claims.issued_at < 0) throw new InvalidDataException("授权名称或时间无效。");
            return claims;
        }
        private static byte[] Base64Url(string text)
        {
            if (!Regex.IsMatch(text, "^[A-Za-z0-9_-]+$") || text.Length % 4 == 1)
                throw new InvalidDataException("注册码内容格式错误。");
            byte[] bytes = Convert.FromBase64String(text.Replace('-', '+').Replace('_', '/') + new string('=', (4 - text.Length % 4) % 4));
            if (Convert.ToBase64String(bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_') != text)
                throw new InvalidDataException("注册码编码无效。");
            return bytes;
        }
    }

    // RFC 8032 verification only. There is no signing key or signing operation.
    // Extended Edwards coordinates avoid inversions inside the scalar loop.
    // Reject noncanonical encodings, S >= L and small-order A/R (strict verify).
    internal static class Ed25519
    {
        private static readonly BigInteger P = (BigInteger.One << 255) - 19;
        private static readonly BigInteger L = (BigInteger.One << 252) + BigInteger.Parse("27742317777372353535851937790883648493");
        private static readonly BigInteger D = Mod(-121665 * BigInteger.ModPow(121666, P - 2, P));
        private static readonly BigInteger SqrtMinusOne = BigInteger.ModPow(2, (P - 1) / 4, P);
        private static readonly Point Identity = new Point(0, 1, 1, 0);
        private static readonly Point Base = Decode(Security.Hex("5866666666666666666666666666666666666666666666666666666666666666"));

        private sealed class Point
        {
            internal readonly BigInteger X, Y, Z, T;
            internal Point(BigInteger x, BigInteger y, BigInteger z, BigInteger t) { X = x; Y = y; Z = z; T = t; }
        }
        private static BigInteger Mod(BigInteger value) { value %= P; return value.Sign < 0 ? value + P : value; }
        private static BigInteger Number(byte[] bytes, int start, int length)
        {
            byte[] positive = new byte[length + 1];
            Buffer.BlockCopy(bytes, start, positive, 0, length);
            return new BigInteger(positive);
        }
        private static Point Decode(byte[] bytes)
        {
            if (bytes == null || bytes.Length != 32) return null;
            byte[] copy = (byte[])bytes.Clone();
            int sign = copy[31] >> 7;
            copy[31] &= 127;
            BigInteger y = Number(copy, 0, 32);
            if (y >= P) return null;
            BigInteger square = Mod((y * y - 1) * BigInteger.ModPow(Mod(D * y * y + 1), P - 2, P));
            BigInteger x = BigInteger.ModPow(square, (P + 3) / 8, P);
            if (Mod(x * x) != square) x = Mod(x * SqrtMinusOne);
            if (Mod(x * x) != square || (x.IsZero && sign != 0)) return null;
            if ((x.IsEven ? 0 : 1) != sign) x = P - x;
            return new Point(x, y, 1, Mod(x * y));
        }
        private static Point Add(Point a, Point b)
        {
            BigInteger aa = Mod((a.Y - a.X) * (b.Y - b.X)), bb = Mod((a.Y + a.X) * (b.Y + b.X));
            BigInteger cc = Mod(2 * D * a.T * b.T), dd = Mod(2 * a.Z * b.Z);
            BigInteger e = Mod(bb - aa), f = Mod(dd - cc), g = Mod(dd + cc), h = Mod(bb + aa);
            return new Point(Mod(e * f), Mod(g * h), Mod(f * g), Mod(e * h));
        }
        private static Point Multiply(Point point, BigInteger scalar)
        {
            Point result = Identity;
            while (scalar > 0)
            {
                if (!scalar.IsEven) result = Add(result, point);
                point = Add(point, point);
                scalar >>= 1;
            }
            return result;
        }
        private static bool Equal(Point a, Point b)
        {
            return Mod(a.X * b.Z - b.X * a.Z).IsZero && Mod(a.Y * b.Z - b.Y * a.Z).IsZero;
        }
        internal static bool Verify(byte[] message, byte[] signature, byte[] key)
        {
            if (signature == null || signature.Length != 64 || key == null || key.Length != 32) return false;
            byte[] encodedR = new byte[32];
            Buffer.BlockCopy(signature, 0, encodedR, 0, 32);
            Point a = Decode(key), r = Decode(encodedR);
            BigInteger s = Number(signature, 32, 32);
            if (a == null || r == null || s >= L || Equal(Multiply(a, 8), Identity) || Equal(Multiply(r, 8), Identity)) return false;
            byte[] input = new byte[64 + message.Length];
            Buffer.BlockCopy(encodedR, 0, input, 0, 32);
            Buffer.BlockCopy(key, 0, input, 32, 32);
            Buffer.BlockCopy(message, 0, input, 64, message.Length);
            using (var hash = SHA512.Create())
            {
                byte[] digest = hash.ComputeHash(input);
                BigInteger h = Number(digest, 0, digest.Length) % L;
                return Equal(Multiply(Base, s), Add(r, Multiply(a, h)));
            }
        }
    }
}
