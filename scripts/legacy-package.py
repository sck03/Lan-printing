"""Pin and verify the small Windows 7 x86 package without extra Python libraries."""
import hashlib
import pathlib
import struct
import sys
import tarfile

PDFIUM_ARCHIVE_SHA256 = "ab580310cf0239f5aaa14fda0137de8980bc1602846e14c0d1819018cd4d11fb"
PACKAGE_FILES = {
    "LanPrintLegacy.exe", "LanPrintLegacy.exe.config", "pdfium.dll",
    "PDFIUM-LICENSE.txt", "README.md", "check-legacy.ps1", "SHA256SUMS.txt",
}


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def extract(archive, output):
    assert sha256(archive) == PDFIUM_ARCHIVE_SHA256, "PDFium archive checksum mismatch"
    output.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive) as source:
        # Fixed known members only; never extract arbitrary archive paths/links.
        for member, name in (("bin/pdfium.dll", "pdfium.dll"), ("LICENSE", "PDFIUM-LICENSE.txt")):
            info = source.getmember(member)
            assert info.isfile() and info.size < 16 * 1024 * 1024
            with source.extractfile(info) as stream:
                (output / name).write_bytes(stream.read())


def pe(path):
    data = path.read_bytes()
    offset = struct.unpack_from("<I", data, 60)[0]
    assert data[:2] == b"MZ" and data[offset:offset + 4] == b"PE\0\0"
    machine, count = struct.unpack_from("<HH", data, offset + 4)
    optional = offset + 24
    assert machine == 0x14C and struct.unpack_from("<H", data, optional)[0] == 0x10B, f"Not PE32/x86: {path}"
    assert struct.unpack_from("<HH", data, optional + 48) <= (6, 1), "Subsystem requires newer than Windows 7"
    section_table = optional + struct.unpack_from("<H", data, offset + 20)[0]
    sections = [struct.unpack_from("<IIII", data, section_table + i * 40 + 8) for i in range(count)]

    def rva(value):
        return next(raw + value - address for size, address, raw_size, raw in sections if address <= value < address + max(size, raw_size))

    def text(value):
        begin = rva(value)
        return data[begin:data.index(0, begin)].decode("ascii")

    imports = {}
    directory = struct.unpack_from("<I", data, optional + 104)[0]
    if directory:
        descriptor = rva(directory)
        while any(data[descriptor:descriptor + 20]):
            lookup, _, _, name, thunk = struct.unpack_from("<IIIII", data, descriptor)
            position = rva(lookup or thunk)
            functions = []
            while (value := struct.unpack_from("<I", data, position)[0]):
                assert not value & 0x80000000, "Unexpected ordinal import"
                functions.append(text(value + 2))
                position += 4
            imports[text(name).lower()] = functions
            descriptor += 20
    return data, optional, rva, imports


def verify(output):
    assert {path.name for path in output.iterdir()} == PACKAGE_FILES, "Unexpected/missing package contents"
    data, optional, rva, imports = pe(output / "LanPrintLegacy.exe")
    assert set(imports) == {"mscoree.dll"} and imports["mscoree.dll"] == ["_CorExeMain"]
    clr = rva(struct.unpack_from("<I", data, optional + 96 + 14 * 8)[0])
    flags = struct.unpack_from("<I", data, clr + 16)[0]
    assert flags & 3 == 3, "CLR assembly must require 32-bit execution"
    metadata = rva(struct.unpack_from("<I", data, clr + 8)[0])
    assert data[metadata:metadata + 4] == b"BSJB" and data[metadata + 16:].startswith(b"v4.0.30319")
    assert b".NETFramework,Version=v4.0" in data, "Assembly must target .NET Framework 4.0"
    _, _, _, imports = pe(output / "pdfium.dll")
    assert set(imports) == {"kernel32.dll", "advapi32.dll", "gdi32.dll", "user32.dll"}, "Unexpected native runtime dependency"
    newer = {"WaitOnAddress", "WakeByAddressSingle", "WakeByAddressAll", "CreateFile2", "GetSystemTimePreciseAsFileTime", "SetThreadDescription", "GetDpiForWindow", "IsWow64Process2", "GetTempPath2W"}
    assert not newer.intersection(name for functions in imports.values() for name in functions), "Post-Windows-7 static import"
    checked = set()
    for line in (output / "SHA256SUMS.txt").read_text(encoding="ascii").splitlines():
        expected, name = line.split("  ", 1)
        assert name not in checked and name in PACKAGE_FILES - {"SHA256SUMS.txt"} and sha256(output / name) == expected.lower(), f"Checksum mismatch: {name}"
        checked.add(name)
    assert checked == PACKAGE_FILES - {"SHA256SUMS.txt"}, "Incomplete checksum manifest"
    print("PASS: exact package contents, SHA256, PE32/x86, CLR 4.0, 32-bit-required flag and Windows 7 native imports")


if __name__ == "__main__":
    if sys.argv[1] == "extract-pdfium":
        extract(pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3]))
    elif sys.argv[1] == "verify":
        verify(pathlib.Path(sys.argv[2]))
    else:
        raise SystemExit("Use extract-pdfium ARCHIVE DIRECTORY or verify PACKAGE_DIRECTORY")
