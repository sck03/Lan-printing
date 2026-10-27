param([string]$OutputDirectory = 'dist/windows7-x86', [string]$DependencyDirectory = 'artifacts/legacy-deps')
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
Push-Location $projectRoot
try {
    if (Test-Path -LiteralPath $OutputDirectory) { throw 'Output directory already exists; choose a new path.' }
    $compiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework/v4.0.30319/csc.exe'
    if (-not (Test-Path -LiteralPath $compiler)) { throw 'Build on Windows with the .NET Framework C# compiler installed.' }
    $dependencyRoot = [IO.Path]::GetFullPath($DependencyDirectory)
    New-Item -ItemType Directory -Force -Path $dependencyRoot | Out-Null
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    function Get-Dependency([string]$Name, [string]$Url, [string]$Hash) {
        $path = Join-Path $dependencyRoot $Name
        if (-not (Test-Path -LiteralPath $path)) {
            $temporary = "$path.download"
            try {
                $client = New-Object Net.WebClient
                try { $client.DownloadFile($Url, $temporary) } finally { $client.Dispose() }
                if ((Get-FileHash -LiteralPath $temporary -Algorithm SHA256).Hash -ne $Hash) { throw "Dependency checksum mismatch: $Name" }
                Move-Item -LiteralPath $temporary -Destination $path
            } finally { if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary } }
        }
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $Hash) { throw "Dependency checksum mismatch: $Name" }
        return $path
    }
    $referenceArchive = Get-Dependency 'net40-1.0.3.nupkg' 'https://api.nuget.org/v3-flatcontainer/microsoft.netframework.referenceassemblies.net40/1.0.3/microsoft.netframework.referenceassemblies.net40.1.0.3.nupkg' '54d6e20a1b61caf79395d6d71d091265e81f5a5705ac4ae52af45ca143c3c694'
    $pdfArchive = Get-Dependency 'pdfium-win-x86-5418.tgz' 'https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/5418/pdfium-win-x86.tgz' 'ab580310cf0239f5aaa14fda0137de8980bc1602846e14c0d1819018cd4d11fb'
    $referenceRoot = Join-Path $dependencyRoot 'net40'
    New-Item -ItemType Directory -Force -Path $referenceRoot | Out-Null
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::OpenRead($referenceArchive)
    $references = @('mscorlib.dll', 'System.dll', 'System.Core.dll', 'System.Drawing.dll', 'System.Numerics.dll', 'System.Web.Extensions.dll', 'System.Windows.Forms.dll')
    try {
        foreach ($name in $references) {
            $entry = $archive.GetEntry("build/.NETFramework/v4.0/$name")
            if ($null -eq $entry) { throw "Missing .NET 4.0 reference assembly: $name" }
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, (Join-Path $referenceRoot $name), $true)
        }
    } finally { $archive.Dispose() }
    # Extract only the runtime DLL/license/version, never headers/import libs.
    $pdfRoot = Join-Path $dependencyRoot 'pdfium-5418'
    New-Item -ItemType Directory -Force -Path $pdfRoot | Out-Null
    python scripts/legacy-package.py extract-pdfium $pdfArchive $pdfRoot
    if ($LASTEXITCODE -ne 0) { throw 'PDFium extraction failed.' }
    if (Test-Path -LiteralPath $OutputDirectory) { throw 'Output directory already exists; choose a new path.' }
    New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
    $outputRoot = (Resolve-Path -LiteralPath $OutputDirectory).Path
    $sourceRoot = Join-Path $projectRoot 'legacy/windows7'
    $sources = @('Program.cs', 'Http.cs', 'Json.cs', 'License.cs', 'Store.cs', 'Multipart.cs', 'Devices.cs', 'Pdf.cs', 'Worker.cs', 'Server.cs')
    $arguments = @('/nologo', '/nostdlib+', '/target:winexe', '/platform:x86', '/optimize+', '/checked+', '/warn:4', '/warnaserror+', '/nowarn:0649', '/langversion:4', '/codepage:65001', '/utf8output')
    # CS0649: DTO fields are populated by the strict JSON deserializer.
    $arguments += $references | ForEach-Object { '/reference:"{0}"' -f (Join-Path $referenceRoot $_) }
    $arguments += '/win32manifest:"{0}"' -f (Join-Path $sourceRoot 'app.manifest')
    $arguments += '/win32icon:"{0}"' -f (Join-Path $projectRoot 'assets/lan-print.ico')
    foreach ($name in @('index.html', 'activation.html', 'app.css', 'app.js', 'activation.js', 'profiles.js', 'network.js')) {
        $arguments += '/resource:"{0}",LanPrint.{1}' -f (Join-Path $projectRoot "web/$name"), $name
    }
    $arguments += '/resource:"{0}",LanPrint.license-public-key.hex' -f (Join-Path $projectRoot 'assets/license-public-key.hex')
    $sourceArguments = $sources | ForEach-Object { '"{0}"' -f (Join-Path $sourceRoot $_) }
    $responsePath = Join-Path $dependencyRoot 'compile.rsp'
    $releaseArguments = $arguments + @('/main:LanPrint.Legacy.Program', ('/out:"{0}"' -f (Join-Path $outputRoot 'LanPrintLegacy.exe'))) + $sourceArguments
    [IO.File]::WriteAllLines($responsePath, [string[]]$releaseArguments, (New-Object Text.UTF8Encoding($true)))
    & $compiler /noconfig "@$responsePath"
    if ($LASTEXITCODE -ne 0) { throw 'Legacy x86 compilation failed.' }
    Copy-Item -LiteralPath (Join-Path $sourceRoot 'LanPrintLegacy.exe.config') -Destination $outputRoot
    Copy-Item -LiteralPath (Join-Path $pdfRoot 'pdfium.dll') -Destination $outputRoot
    Copy-Item -LiteralPath (Join-Path $pdfRoot 'PDFIUM-LICENSE.txt') -Destination $outputRoot
    Copy-Item -LiteralPath docs/WINDOWS7.md -Destination (Join-Path $outputRoot 'README.md')
    Copy-Item -LiteralPath scripts/check-legacy.ps1 -Destination $outputRoot
    $testRoot = Join-Path $dependencyRoot 'tests'
    New-Item -ItemType Directory -Force -Path $testRoot | Out-Null
    $testExe = Join-Path $testRoot 'LegacyTests.exe'
    $testArguments = @($arguments | Where-Object { $_ -ne '/target:winexe' }) + @('/target:exe', '/main:LanPrint.Legacy.Tests', ('/out:"{0}"' -f $testExe)) + $sourceArguments + @('"{0}"' -f (Join-Path $sourceRoot 'Tests.cs'))
    [IO.File]::WriteAllLines($responsePath, [string[]]$testArguments, (New-Object Text.UTF8Encoding($true)))
    & $compiler /noconfig "@$responsePath"
    if ($LASTEXITCODE -ne 0) { throw 'Legacy test compilation failed.' }
    Copy-Item -LiteralPath (Join-Path $outputRoot 'pdfium.dll') -Destination $testRoot -Force
    & $testExe
    if ($LASTEXITCODE -ne 0) { throw 'Legacy tests failed.' }
    $hashes = @('LanPrintLegacy.exe', 'LanPrintLegacy.exe.config', 'pdfium.dll', 'PDFIUM-LICENSE.txt', 'README.md', 'check-legacy.ps1') | ForEach-Object {
        '{0}  {1}' -f (Get-FileHash -LiteralPath (Join-Path $outputRoot $_) -Algorithm SHA256).Hash, $_
    }
    [IO.File]::WriteAllLines((Join-Path $outputRoot 'SHA256SUMS.txt'), [string[]]$hashes, [Text.Encoding]::ASCII)
    python scripts/legacy-package.py verify $outputRoot
    if ($LASTEXITCODE -ne 0) { throw 'Legacy package verification failed.' }
    python scripts/host-smoke.py (Join-Path $outputRoot 'LanPrintLegacy.exe') --legacy
    if ($LASTEXITCODE -ne 0) { throw 'Legacy packaged host verification failed.' }
    Write-Output "Ready: $outputRoot"
} finally { Pop-Location }
