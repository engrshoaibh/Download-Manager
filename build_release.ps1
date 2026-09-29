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

Write-Host "Copying core binaries to Tauri app for bundling..."
Copy-Item ".\target\release\dm-daemon.exe" -Destination "$PSScriptRoot\ui\src-tauri\" -Force
Copy-Item ".\target\release\dm-native-host.exe" -Destination "$PSScriptRoot\ui\src-tauri\" -Force

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

Write-Host "Generating updater.json..."
$Version = (Get-Content ".\src-tauri\tauri.conf.json" | ConvertFrom-Json).version
$NsisZip = Get-ChildItem -Path ".\src-tauri\target\release\bundle\nsis" -Filter "*.zip" -ErrorAction SilentlyContinue
$NsisSig = Get-ChildItem -Path ".\src-tauri\target\release\bundle\nsis" -Filter "*.zip.sig" -ErrorAction SilentlyContinue

if ($NsisZip -and $NsisSig) {
    $SigContent = Get-Content $NsisSig.FullName -Raw
    $FileName = $NsisZip.Name
    # Ensure URL matches the GitHub Release format (v$Version)
    $DownloadUrl = "https://github.com/engrshoaibh/Download-Manager/releases/download/v$Version/$FileName"
    
    $UpdaterObj = @{
        version = "v$Version"
        notes = "Update v$Version is now available!"
        pub_date = (Get-Date).ToString("yyyy-MM-ddTHH:mm:ssZ")
        platforms = @{
            "windows-x86_64" = @{
                signature = $SigContent.Trim()
                url = $DownloadUrl
            }
        }
    }
    $UpdaterObj | ConvertTo-Json -Depth 5 | Out-File -FilePath "$ReleaseDir\updater.json" -Encoding utf8
    
    # Also copy the zip and sig to release dir so they are uploaded
    Copy-Item $NsisZip.FullName -Destination $ReleaseDir -Force
    Copy-Item $NsisSig.FullName -Destination $ReleaseDir -Force
}

Write-Host "Packaging Extension..."
Set-Location "$PSScriptRoot\extension"
Compress-Archive -Path ".\*" -DestinationPath "$ReleaseDir\dm-extension.zip" -Force

Set-Location $PSScriptRoot
Write-Host "Production build complete! Check the 'release' directory."
