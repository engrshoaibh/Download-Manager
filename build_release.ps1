# Builds the Rust binaries and packages the release assets into .\release
# Assets: dm-gui.exe (desktop app), dm-daemon.exe (headless), dm-native-host.exe,
#         dm-extension.zip, Download-Manager-Pro-windows-x64.zip, SHA256SUMS.txt
$ErrorActionPreference = 'Stop'

$Root = $PSScriptRoot
$ReleaseDir = Join-Path $Root 'release'
if (Test-Path $ReleaseDir) { Remove-Item $ReleaseDir -Recurse -Force }
New-Item -ItemType Directory -Path $ReleaseDir | Out-Null

Write-Host 'Building binaries (release)...'
Push-Location (Join-Path $Root 'core')
try {
    # Windows PowerShell 5.1 turns cargo's stderr progress output into errors under 'Stop'.
    $ErrorActionPreference = 'Continue'
    cargo build --release --bin dm-gui --bin dm-daemon --bin dm-native-host
    $buildExit = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($buildExit -ne 0) { throw "cargo build failed (exit $buildExit)" }
} finally {
    Pop-Location
}

$Target = Join-Path $Root 'core\target\release'
foreach ($bin in 'dm-gui.exe', 'dm-daemon.exe', 'dm-native-host.exe') {
    $src = Join-Path $Target $bin
    if (-not (Test-Path $src)) { throw "Missing build output: $src" }
    Copy-Item $src -Destination $ReleaseDir -Force
}

Write-Host 'Packaging browser extension...'
Compress-Archive -Path (Join-Path $Root 'extension\*') -DestinationPath (Join-Path $ReleaseDir 'dm-extension.zip') -Force

Write-Host 'Creating portable bundle...'
$Stage = Join-Path $ReleaseDir 'bundle'
New-Item -ItemType Directory -Path $Stage | Out-Null
foreach ($bin in 'dm-gui.exe', 'dm-daemon.exe', 'dm-native-host.exe') {
    Copy-Item (Join-Path $ReleaseDir $bin) -Destination $Stage
}
Copy-Item (Join-Path $ReleaseDir 'dm-extension.zip') -Destination $Stage
Copy-Item (Join-Path $Root 'LICENSE') -Destination $Stage
Compress-Archive -Path (Join-Path $Stage '*') -DestinationPath (Join-Path $ReleaseDir 'Download-Manager-Pro-windows-x64.zip') -Force
Remove-Item $Stage -Recurse -Force

Write-Host 'Writing checksums...'
Get-ChildItem $ReleaseDir -File | Where-Object { $_.Name -ne 'SHA256SUMS.txt' } | ForEach-Object {
    $hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower()
    "$hash  $($_.Name)"
} | Out-File -FilePath (Join-Path $ReleaseDir 'SHA256SUMS.txt') -Encoding ascii

Write-Host "Release build complete. Assets in: $ReleaseDir"
Get-ChildItem $ReleaseDir | Format-Table Name, Length
