# Builds the Rust binaries and packages ONE standalone installer into .\release
# Output: DownloadManagerPro-Setup-<version>.exe  +  SHA256SUMS.txt (used by the in-app updater)
# Requires Inno Setup 6 (ISCC.exe) - preinstalled on GitHub's windows runners,
# locally:  winget install JRSoftware.InnoSetup
$ErrorActionPreference = 'Stop'

$Root = $PSScriptRoot
$ReleaseDir = Join-Path $Root 'release'
if (Test-Path $ReleaseDir) { Remove-Item $ReleaseDir -Recurse -Force }
New-Item -ItemType Directory -Path $ReleaseDir | Out-Null

# Version comes from the crate so the installer, the app and the updater always agree.
$Version = (Select-String -Path (Join-Path $Root 'core\Cargo.toml') -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches.Groups[1].Value
if (-not $Version) { throw 'Could not read version from core/Cargo.toml' }
Write-Host "Version: $Version"

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

foreach ($bin in 'dm-gui.exe', 'dm-daemon.exe', 'dm-native-host.exe') {
    $binPath = Join-Path $Root "core\target\release\$bin"
    if (-not (Test-Path $binPath)) { throw "Missing build output: $bin" }
    # Fresh Windows installs lack the VC++ runtime; core/.cargo/config.toml links it statically.
    $ascii = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($binPath))
    if ($ascii -match '(?i)vcruntime140\.dll') { throw "$bin still depends on VCRUNTIME140.dll (crt-static not applied)" }
}

# YouTube tools shipped so downloads work right after install (the app otherwise fetches
# them on first use). Each download is checked against its project's published SHA-256.
$VendorDir = Join-Path $Root 'installer\vendor'
if (Test-Path $VendorDir) { Remove-Item $VendorDir -Recurse -Force }
New-Item -ItemType Directory -Path $VendorDir | Out-Null
$ProgressPreference = 'SilentlyContinue'

function Get-VerifiedFile([string]$Url, [string]$OutFile, [string]$SumsUrl, [string]$Name) {
    Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing
    $sums = (Invoke-WebRequest -Uri $SumsUrl -UseBasicParsing).Content
    if ($sums -is [byte[]]) { $sums = [System.Text.Encoding]::ASCII.GetString($sums) }
    $lines = @($sums -split "`r?`n" | Where-Object { $_ -match '[0-9a-fA-F]{64}' })
    $line = $lines | Where-Object { $_ -match "(^|[\s*/\\])$([regex]::Escape($Name))\s*$" } | Select-Object -First 1
    if (-not $line -and $lines.Count -eq 1) { $line = $lines[0] }  # single-file checksum
    if (-not $line) { throw "Checksum for $Name not found at $SumsUrl" }
    $expected = ([regex]::Match($line, '[0-9a-fA-F]{64}')).Value.ToUpper()
    $actual = (Get-FileHash $OutFile -Algorithm SHA256).Hash
    if ($actual -ne $expected) { throw "$Name checksum mismatch (expected $expected, got $actual)" }
    Write-Host "$Name verified ($((Get-Item $OutFile).Length) bytes)"
}

Write-Host 'Fetching yt-dlp...'
$YtBase = 'https://github.com/yt-dlp/yt-dlp/releases/latest/download'
Get-VerifiedFile "$YtBase/yt-dlp.exe" (Join-Path $VendorDir 'yt-dlp.exe') "$YtBase/SHA2-256SUMS" 'yt-dlp.exe'

Write-Host 'Fetching FFmpeg...'
$FfBase = 'https://github.com/yt-dlp/FFmpeg-Builds/releases/latest/download'
$FfName = 'ffmpeg-master-latest-win64-gpl.zip'
$FfZip = Join-Path $env:TEMP $FfName
Get-VerifiedFile "$FfBase/$FfName" $FfZip "$FfBase/checksums.sha256" $FfName
$FfOut = Join-Path $env:TEMP 'dm-ffmpeg'
if (Test-Path $FfOut) { Remove-Item $FfOut -Recurse -Force }
Expand-Archive $FfZip -DestinationPath $FfOut
$FfBin = Get-ChildItem $FfOut -Recurse -Filter 'ffmpeg.exe' | Where-Object { $_.Directory.Name -eq 'bin' } | Select-Object -First 1
if (-not $FfBin) { throw 'ffmpeg.exe not found in FFmpeg archive' }
Copy-Item (Join-Path $FfBin.DirectoryName 'ffmpeg.exe') $VendorDir
Copy-Item (Join-Path $FfBin.DirectoryName 'ffprobe.exe') $VendorDir
# GPL build: ship its licence alongside the binaries.
$FfLicense = Get-ChildItem $FfOut -Recurse -Filter 'LICENSE*' | Select-Object -First 1
if ($FfLicense) { Copy-Item $FfLicense.FullName (Join-Path $VendorDir 'FFmpeg-LICENSE.txt') }
Remove-Item $FfZip, $FfOut -Recurse -Force

Write-Host 'Fetching Deno...'
$DenoBase = 'https://github.com/denoland/deno/releases/latest/download'
$DenoName = 'deno-x86_64-pc-windows-msvc.zip'
$DenoZip = Join-Path $env:TEMP $DenoName
Get-VerifiedFile "$DenoBase/$DenoName" $DenoZip "$DenoBase/$DenoName.sha256sum" $DenoName
Expand-Archive $DenoZip -DestinationPath $VendorDir -Force
Remove-Item $DenoZip -Force
if (-not (Test-Path (Join-Path $VendorDir 'deno.exe'))) { throw 'deno.exe not found in Deno archive' }

Write-Host 'Locating Inno Setup...'
$Iscc = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source
if (-not $Iscc) {
    $candidates = @(
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
    )
    $Iscc = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $Iscc) { throw 'Inno Setup not found. Install it with: winget install JRSoftware.InnoSetup' }

Write-Host 'Building installer...'
$ErrorActionPreference = 'Continue'
& $Iscc "/DAppVersion=$Version" "/DSourceDir=$Root" "/DOutputDir=$ReleaseDir" (Join-Path $Root 'installer\DownloadManagerPro.iss')
$isccExit = $LASTEXITCODE
$ErrorActionPreference = 'Stop'
if ($isccExit -ne 0) { throw "Inno Setup failed (exit $isccExit)" }

$Installer = Get-ChildItem $ReleaseDir -Filter 'DownloadManagerPro-Setup-*.exe' | Select-Object -First 1
if (-not $Installer) { throw 'Installer was not produced' }

Write-Host 'Writing checksums...'
$hash = (Get-FileHash $Installer.FullName -Algorithm SHA256).Hash.ToLower()
"$hash  $($Installer.Name)" | Out-File -FilePath (Join-Path $ReleaseDir 'SHA256SUMS.txt') -Encoding ascii

Write-Host "Release build complete: $($Installer.FullName)"
Get-ChildItem $ReleaseDir | Format-Table Name, Length
