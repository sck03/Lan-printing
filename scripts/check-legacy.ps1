# Compatible with Windows 7's built-in Windows PowerShell 2.0.
param([string]$Executable = '', [string]$LicenseFile = '')
$ErrorActionPreference = 'Stop'
if (-not $Executable -or -not (Test-Path -LiteralPath $Executable)) {
    $Executable = Join-Path (Split-Path $MyInvocation.MyCommand.Path -Parent) 'LanPrintLegacy.exe'
}
$exePath = (Resolve-Path -LiteralPath $Executable).Path
$reader = New-Object IO.BinaryReader([IO.File]::OpenRead($exePath))
try {
    $reader.BaseStream.Position = 60
    $peOffset = $reader.ReadUInt32()
    $reader.BaseStream.Position = $peOffset + 4
    if ($reader.ReadUInt16() -ne 332) { throw 'The executable is not x86.' }
} finally { $reader.Close() }
$hashStream = [IO.File]::OpenRead($exePath)
$sha = New-Object Security.Cryptography.SHA256Managed
try { $hash = [BitConverter]::ToString($sha.ComputeHash($hashStream)).Replace('-', '') }
finally { $sha.Dispose(); $hashStream.Close() }
Write-Output ('Windows: ' + [Environment]::OSVersion.VersionString)
Write-Output ('Executable SHA256: ' + $hash)
Write-Output ('CPU architecture: ' + $env:PROCESSOR_ARCHITECTURE)
if (-not $LicenseFile) { $LicenseFile = Join-Path $env:LOCALAPPDATA 'LanPrint-legacy/license.json' }
if (-not (Test-Path -LiteralPath $LicenseFile)) { throw 'Register the host first, or pass -LicenseFile with its saved license.json.' }
$licenseText = [IO.File]::ReadAllText((Resolve-Path -LiteralPath $LicenseFile).Path)
$match = [regex]::Match($licenseText, '"code"\s*:\s*"(LP1\.[A-Za-z0-9_.-]+)"')
if (-not $match.Success) { throw 'Invalid license.json; register the host again.' }
$code = $match.Groups[1].Value
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ('lanprint-legacy-check-' + [Guid]::NewGuid().ToString('D'))
[IO.Directory]::CreateDirectory($temporaryRoot) | Out-Null
function Escape-Json([string]$Value) { return $Value.Replace('\', '\\').Replace('"', '\"') }
function Invoke-Worker([string]$Operation, [string]$Path, [string]$Output) {
    $requestPath = Join-Path $temporaryRoot 'request.json'
    $responsePath = Join-Path $temporaryRoot 'response.json'
    $request = '{"license":"' + $code + '","operation":"' + $Operation + '","path":"' + (Escape-Json $Path) + '","output":"' + (Escape-Json $Output) + '","show_virtual":true}'
    [IO.File]::WriteAllText($requestPath, $request, (New-Object Text.UTF8Encoding($false)))
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $exePath
    $start.Arguments = '--worker "' + $requestPath + '" "' + $responsePath + '"'
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $process = [Diagnostics.Process]::Start($start)
    try {
        if (-not $process.WaitForExit(30000)) { $process.Kill(); $process.WaitForExit(); throw 'Worker timed out.' }
        $response = [IO.File]::ReadAllText($responsePath)
        if ($process.ExitCode -ne 0) { throw ('Worker failed: ' + $response) }
        return $response
    } finally { $process.Dispose() }
}
try {
    $pdf = Join-Path $temporaryRoot 'sample.pdf'
    $preview = Join-Path $temporaryRoot 'preview.jpg'
    $text = "%PDF-1.4`n"
    $offsets = @(0)
    $objects = @('<< /Type /Catalog /Pages 2 0 R >>', '<< /Type /Pages /Kids [3 0 R] /Count 1 >>', '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] >>')
    for ($i = 0; $i -lt $objects.Length; $i++) {
        $offsets += $text.Length
        $text += ($i + 1).ToString() + " 0 obj`n" + $objects[$i] + "`nendobj`n"
    }
    $xref = $text.Length
    $text += "xref`n0 4`n0000000000 65535 f `n"
    for ($i = 1; $i -lt $offsets.Length; $i++) { $text += $offsets[$i].ToString('D10') + " 00000 n `n" }
    $text += "trailer`n<< /Size 4 /Root 1 0 R >>`nstartxref`n" + $xref + "`n%%EOF`n"
    [IO.File]::WriteAllText($pdf, $text, [Text.Encoding]::ASCII)
    Write-Output (Invoke-Worker 'inspect' $pdf '')
    Invoke-Worker 'preview' $pdf $preview | Out-Null
    if (([IO.File]::ReadAllBytes($preview))[0] -ne 255) { throw 'Invalid JPEG preview.' }
    Write-Output (Invoke-Worker 'devices' '' '')
    Write-Output 'PASS: x86 worker, offline license, native PDF page count/preview and device enumeration. No paper used.'
} finally {
    # Verify the final absolute target before recursive cleanup, in one shell.
    $resolved = [IO.Path]::GetFullPath($temporaryRoot)
    $tempPrefix = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $resolved.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase) -or -not ([IO.Path]::GetFileName($resolved)).StartsWith('lanprint-legacy-check-')) { throw 'Unsafe temporary directory.' }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
