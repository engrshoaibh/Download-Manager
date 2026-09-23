$ErrorActionPreference = 'Stop'

Write-Host "Creating release directory..."
$ReleaseDir = "$PSScriptRoot\release"
if (!(Test-Path $ReleaseDir)) {
    New-Item -ItemType Directory -Path $ReleaseDir | Out-Null
}

Write-Host "Building core binaries (Release)..."
Set-Location "$PSScriptRoot\core"
cargo build --release

Write-Host "Copying core binaries..."
Copy-Item ".\target\release\dm-daemon.exe" -Destination $ReleaseDir -Force
Copy-Item ".\target\release\dm-native-host.exe" -Destination $ReleaseDir -Force

Write-Host "Building Tauri UI (Release)..."
Set-Location "$PSScriptRoot\ui"
npm install
npm run tauri build

Write-Host "Copying Tauri installer..."
# The installer is usually at ui/src-tauri/target/release/bundle/msi/ or nsis/
$MsiPath = Get-ChildItem -Path ".\src-tauri\target\release\bundle\msi" -Filter "*.msi" -ErrorAction SilentlyContinue
if ($MsiPath) {
    Copy-Item $MsiPath.FullName -Destination $ReleaseDir -Force
}
$NsisPath = Get-ChildItem -Path ".\src-tauri\target\release\bundle\nsis" -Filter "*.exe" -ErrorAction SilentlyContinue
if ($NsisPath) {
    Copy-Item $NsisPath.FullName -Destination $ReleaseDir -Force
}

Write-Host "Packaging Extension..."
Set-Location "$PSScriptRoot\extension"
Compress-Archive -Path ".\*" -DestinationPath "$ReleaseDir\dm-extension.zip" -Force

Set-Location $PSScriptRoot
Write-Host "Production build complete! Check the 'release' directory."
