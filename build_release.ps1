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

Write-Host 'Fetching yt-dlp for the installer...'
# Shipped so YouTube works right after install; the app's updater keeps it current afterwards.
$VendorDir = Join-Path $Root 'installer\vendor'
New-Item -ItemType Directory -Path $VendorDir -Force | Out-Null
$YtDlp = Join-Path $VendorDir 'yt-dlp.exe'
$YtBase = 'https://github.com/yt-dlp/yt-dlp/releases/latest/download'
$ProgressPreference = 'SilentlyContinue'
Invoke-WebRequest -Uri "$YtBase/yt-dlp.exe" -OutFile $YtDlp -UseBasicParsing
$sums = (Invoke-WebRequest -Uri "$YtBase/SHA2-256SUMS" -UseBasicParsing).Content
if ($sums -is [byte[]]) { $sums = [System.Text.Encoding]::ASCII.GetString($sums) }
$expected = ($sums -split "`n" | Where-Object { $_ -match '\syt-dlp\.exe\s*$' } | Select-Object -First 1) -replace '\s.*$', ''
if (-not $expected) { throw 'yt-dlp.exe checksum not found in SHA2-256SUMS' }
$actual = (Get-FileHash $YtDlp -Algorithm SHA256).Hash
if ($actual -ne $expected.Trim().ToUpper()) { throw "yt-dlp.exe checksum mismatch (expected $expected, got $actual)" }
Write-Host "yt-dlp.exe verified ($((Get-Item $YtDlp).Length) bytes)"

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
