# Windows PowerShell 5.1+. Does not print, scan physical paper, or change settings.
param(
    [string]$Exe,
    [string]$LicenseFile = (Join-Path $env:LOCALAPPDATA 'LanPrint/license.json'),
    [string]$OutputDirectory = (Join-Path $env:TEMP ('LanPrint-check-' + [guid]::NewGuid().ToString()))
)
$ErrorActionPreference = 'Stop'
if (-not $Exe) {
    $Exe = Join-Path $PSScriptRoot '../LanPrint.exe'
    if (-not (Test-Path -LiteralPath $Exe)) {
        $Exe = Join-Path $PSScriptRoot '../dist/LanPrint.exe'
    }
}
$exePath = (Resolve-Path -LiteralPath $Exe).Path
if (-not (Test-Path -LiteralPath $LicenseFile)) { throw 'Register LanPrint on this host first, or pass -LicenseFile PATH.' }
$licenseCode = ([IO.File]::ReadAllText((Resolve-Path -LiteralPath $LicenseFile).Path) | ConvertFrom-Json).code
if (-not $licenseCode) { throw 'License file has no registration code.' }
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$checkRoot = (Resolve-Path -LiteralPath $OutputDirectory).Path
$utf8 = New-Object System.Text.UTF8Encoding($false)
$report = [ordered]@{
    timestamp = (Get-Date).ToString('o')
    executableSha256 = (Get-FileHash -LiteralPath $exePath -Algorithm SHA256).Hash
    os = (Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, OSArchitecture)
    graphics = @(Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion)
    checks = @()
    passed = $false
}
function Invoke-CheckWorker([string]$Name, [hashtable]$Request) {
    $inputPath = Join-Path $checkRoot ($Name + '-request.json')
    $resultPath = Join-Path $checkRoot ($Name + '-response.json')
    $defaults = @{ license=$licenseCode; operation=''; path=''; output=''; page=0; print=$null; scan=$null; show_virtual=$false; demo=$false }
    foreach ($key in $Request.Keys) { $defaults[$key] = $Request[$key] }
    [IO.File]::WriteAllText($inputPath, ($defaults | ConvertTo-Json -Depth 8), $utf8)
    $process = Start-Process -FilePath $exePath -ArgumentList @('--worker', ('"' + $inputPath + '"'), ('"' + $resultPath + '"')) -WindowStyle Hidden -PassThru
    try {
        if (-not $process.WaitForExit(30000)) {
            $process.Kill()
            $process.WaitForExit()
            throw "$Name timed out"
        }
        if ($process.ExitCode -ne 0) { throw "$Name failed with exit code $($process.ExitCode); see $resultPath" }
        $response = [IO.File]::ReadAllText($resultPath) | ConvertFrom-Json
        if ($response.error) { throw $response.error }
        $report.checks += @{ name=$Name; passed=$true }
        return $response
    } finally { $process.Dispose() }
}
try {
    $pdfPath = Join-Path $checkRoot 'sample.pdf'
    $previewPath = Join-Path $checkRoot 'preview.jpg'
    $null = Invoke-CheckWorker 'sample' @{ operation='scan'; output=$pdfPath; demo=$true; scan=@{scanner='demo-scanner'; dpi=200; color=$true; format='pdf'; source='flatbed'} }
    $inspection = Invoke-CheckWorker 'inspect' @{ operation='inspect'; path=$pdfPath }
    if ($inspection.pages -ne 1) { throw 'Expected one PDF page' }
    $null = Invoke-CheckWorker 'preview' @{ operation='preview'; path=$pdfPath; output=$previewPath }
    if ((Get-Item -LiteralPath $previewPath).Length -eq 0) { throw 'Empty preview' }
    $devices = Invoke-CheckWorker 'devices' @{ operation='devices' }
    $report.devices = $devices.devices
    $report.passed = $true
} catch {
    $report.error = $_.Exception.Message
} finally {
    $reportPath = Join-Path $checkRoot 'report.json'
    [IO.File]::WriteAllText($reportPath, ($report | ConvertTo-Json -Depth 10), $utf8)
    Write-Output "Report: $reportPath"
    Write-Output "Preview: $checkRoot\preview.jpg"
}
if (-not $report.passed) { throw $report.error }
Write-Output 'PASS: PDF generation, inspection, rendering, worker exit and device enumeration. Physical printer/scanner acceptance is still required.'
