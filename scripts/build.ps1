param([string]$OutputDirectory = 'dist', [string]$Target = '')
$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    if (-not (Test-Path -LiteralPath 'assets/license-public-key.hex')) { throw 'Missing license verification public key.' }
    $targetArgs = @()
    if ($Target) { $targetArgs = @('--target', $Target) }
    cargo fmt --check
    if ($LASTEXITCODE -ne 0) { throw 'Formatting check failed' }
    cargo clippy --all-targets --locked @targetArgs -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'Static checks failed' }
    cargo test --all-targets --locked @targetArgs
    if ($LASTEXITCODE -ne 0) { throw 'Tests failed' }
    cargo build --release --locked --bin lan-print @targetArgs
    if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
    $targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
    if ($Target) { $targetRoot = Join-Path $targetRoot $Target }
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
