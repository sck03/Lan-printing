using System;
using System.IO;
using System.Text;

namespace LanPrint.Legacy
{
    // The shared IPv4 URL fits QR version 2-L (32 byte capacity). Keeping this
    // encoder limited to that format avoids a runtime library or network call.
    internal static class ShareQr
    {
        internal static string Svg(string url)
        {
            if (url == null || url.Length > 32) throw new InvalidDataException("共享地址过长。");
            foreach (char c in url) if (c > 127) throw new InvalidDataException("共享地址须为 ASCII。");
            int[] data = new int[44];
            int position = 0;
            Action<int, int> append = (value, count) => {
                for (int i = count - 1; i >= 0; i--, position++)
                    data[position / 8] |= ((value >> i) & 1) << (7 - position % 8);
            };
            append(4, 4); append(url.Length, 8);
            foreach (char c in url) append(c, 8);
            append(0, 4);
            while (position % 8 != 0) append(0, 1);
            for (int pad = 0; position < 272; pad++) append(pad % 2 == 0 ? 0xEC : 0x11, 8);

            // Reed-Solomon: one block of 34 data and 10 correction codewords.
            int[] divisor = new int[10]; divisor[9] = 1;
            int root = 1;
            for (int i = 0; i < 10; i++, root = Multiply(root, 2))
                for (int j = 0; j < 10; j++)
                    divisor[j] = Multiply(divisor[j], root) ^ (j < 9 ? divisor[j + 1] : 0);
            int[] remainder = new int[10];
            for (int i = 0; i < 34; i++)
            {
                int factor = data[i] ^ remainder[0];
                Array.Copy(remainder, 1, remainder, 0, 9); remainder[9] = 0;
                for (int j = 0; j < 10; j++) remainder[j] ^= Multiply(divisor[j], factor);
            }
            Array.Copy(remainder, 0, data, 34, 10);

            bool[,] modules = new bool[25, 25], reserved = new bool[25, 25];
            Action<int, int, bool> set = (x, y, dark) => { modules[x, y] = dark; reserved[x, y] = true; };
            for (int i = 0; i < 25; i++) { set(6, i, i % 2 == 0); set(i, 6, i % 2 == 0); }
            foreach (int[] center in new[] { new[] { 3, 3 }, new[] { 21, 3 }, new[] { 3, 21 } })
                for (int dy = -4; dy <= 4; dy++)
                    for (int dx = -4; dx <= 4; dx++)
                    {
                        int x = center[0] + dx, y = center[1] + dy;
                        int distance = Math.Max(Math.Abs(dx), Math.Abs(dy));
                        if (x >= 0 && y >= 0 && x < 25 && y < 25) set(x, y, distance != 2 && distance != 4);
                    }
            for (int dy = -2; dy <= 2; dy++)
                for (int dx = -2; dx <= 2; dx++) set(18 + dx, 18 + dy, Math.Max(Math.Abs(dx), Math.Abs(dy)) != 1);

            // L correction, mask 0; BCH format information and its second copy.
            int format = 8, check = format;
            for (int i = 0; i < 10; i++) check = (check << 1) ^ ((check >> 9) * 0x537);
            format = ((format << 10) | check) ^ 0x5412;
            Func<int, bool> bit = i => ((format >> i) & 1) != 0;
            for (int i = 0; i <= 5; i++) set(8, i, bit(i));
            set(8, 7, bit(6)); set(8, 8, bit(7)); set(7, 8, bit(8));
            for (int i = 9; i < 15; i++) set(14 - i, 8, bit(i));
            for (int i = 0; i < 8; i++) set(24 - i, 8, bit(i));
            for (int i = 8; i < 15; i++) set(8, 10 + i, bit(i));
            set(8, 17, true);

            position = 0;
            for (int right = 24; right >= 1; right -= 2)
            {
                if (right == 6) right = 5;
                for (int vertical = 0; vertical < 25; vertical++)
                    for (int j = 0; j < 2; j++)
                    {
                        int x = right - j, y = ((right + 1) & 2) == 0 ? 24 - vertical : vertical;
                        if (reserved[x, y]) continue;
                        bool dark = position < 352 && ((data[position / 8] >> (7 - position % 8)) & 1) != 0;
                        modules[x, y] = dark ^ ((x + y) % 2 == 0); position++;
                    }
            }
            var svg = new StringBuilder("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 33 33\" width=\"198\" height=\"198\" shape-rendering=\"crispEdges\"><rect width=\"33\" height=\"33\" fill=\"white\"/><path fill=\"black\" d=\"");
            for (int y = 0; y < 25; y++)
                for (int x = 0; x < 25; x++)
                    if (modules[x, y]) svg.Append("M").Append(x + 4).Append(",").Append(y + 4).Append("h1v1h-1z");
            return svg.Append("\"/></svg>").ToString();
        }
        private static int Multiply(int x, int y)
        {
            int result = 0;
            for (int i = 7; i >= 0; i--) result = (result << 1) ^ ((result >> 7) * 0x11D) ^ (((y >> i) & 1) * x);
            return result;
        }
    }
}
