param([string]$OutputDirectory = 'dist', [string]$Target = '')
$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    if (Test-Path -LiteralPath $OutputDirectory) { throw 'Output directory already exists; choose a new empty path.' }
    if (-not (Test-Path -LiteralPath 'assets/license-public-key.hex')) { throw 'Missing license verification public key.' }
    $compiler = rustc -vV
    if ($LASTEXITCODE -ne 0) { throw 'Cannot query Rust host target.' }
    $buildHost = ($compiler | Where-Object { $_ -like 'host: *' }) -replace '^host: ', ''
    $supportedTargets = @('x86_64-pc-windows-msvc', 'x86_64-pc-windows-gnu')
    if ($buildHost -notin $supportedTargets) { throw 'Run this script on a Windows x64 Rust host.' }
    if (-not $Target) { $Target = if ($env:CARGO_BUILD_TARGET) { $env:CARGO_BUILD_TARGET } else { $buildHost } }
    if ($Target -notin $supportedTargets) { throw "Unsupported Windows target: $Target" }
    $targetArgs = @('--target', $Target)
    cargo fmt --check
    if ($LASTEXITCODE -ne 0) { throw 'Formatting check failed' }
    cargo clippy --all-targets --locked @targetArgs -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'Static checks failed' }
    cargo test --all-targets --locked @targetArgs
    if ($LASTEXITCODE -ne 0) { throw 'Tests failed' }
    cargo build --release --locked --bin lan-print @targetArgs
    if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
    $metadata = cargo metadata --no-deps --format-version 1 --locked
    if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve Cargo target directory.' }
    $targetRoot = Join-Path ($metadata | ConvertFrom-Json).target_directory $Target
    # Always stage into a new directory to exclude stale issuer tools.
    if (Test-Path -LiteralPath $OutputDirectory) { throw 'Output directory already exists; choose a new empty path.' }
    New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
    $outputRoot = (Resolve-Path -LiteralPath $OutputDirectory).Path
    $releaseExe = Join-Path $outputRoot 'LanPrint.exe'
    Copy-Item -LiteralPath (Join-Path $targetRoot 'release/lan-print.exe') -Destination $releaseExe
    Copy-Item -LiteralPath README.md -Destination (Join-Path $outputRoot 'README.md')
    Copy-Item -LiteralPath docs -Destination $outputRoot -Recurse -Force
    $scriptRoot = Join-Path $outputRoot 'scripts'
    New-Item -ItemType Directory -Force -Path $scriptRoot | Out-Null
    Copy-Item -LiteralPath scripts/check-compatibility.ps1 -Destination (Join-Path $scriptRoot 'check-compatibility.ps1')
    $releaseHash = (Get-FileHash -LiteralPath $releaseExe -Algorithm SHA256).Hash
    Set-Content -LiteralPath (Join-Path $outputRoot 'SHA256SUMS.txt') -Value "$releaseHash  LanPrint.exe" -Encoding ascii
    Write-Output "Ready: $releaseExe"
} finally { Pop-Location }
