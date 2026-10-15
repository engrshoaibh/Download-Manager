; Inno Setup script - builds one self-contained installer for Download Manager Pro.
; Compile with:  ISCC.exe /DAppVersion=0.1.16 /DSourceDir=<repo root> installer\DownloadManagerPro.iss

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir ".."
#endif
#ifndef OutputDir
  #define OutputDir "..\release"
#endif

#define AppName "Download Manager Pro"
#define AppExe "dm-gui.exe"

[Setup]
AppId={{6E0B5C0E-6F6B-4C2B-9F5A-2D1A7D0C4B11}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Shoaib Hassan
AppPublisherURL=https://github.com/engrshoaibh/Download-Manager
AppSupportURL=https://github.com/engrshoaibh/Download-Manager/issues
AppUpdatesURL=https://github.com/engrshoaibh/Download-Manager/releases
VersionInfoVersion={#AppVersion}
; Per-user install: no admin prompt, and the folder stays writable for updates.
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
SetupIconFile={#SourceDir}\core\assets\app-icon.ico
LicenseFile={#SourceDir}\LICENSE
OutputDir={#OutputDir}
OutputBaseFilename=DownloadManagerPro-Setup-{#AppVersion}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Upgrades: close the running app first and relaunch it afterwards.
CloseApplications=yes
RestartApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"
Name: "startup"; Description: "Start {#AppName} when I sign in to Windows"; GroupDescription: "Startup:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\core\target\release\dm-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\core\target\release\dm-daemon.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\core\target\release\dm-native-host.exe"; DestDir: "{app}"; Flags: ignoreversion
; Native messaging manifest: lets the browser extension start the app when it is closed.
Source: "{#SourceDir}\installer\com.dmpro.native_host.json"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\extension\*"; DestDir: "{app}\browser-extension"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon
Name: "{autoprograms}\{#AppName} browser extension folder"; Filename: "{app}\browser-extension"

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "DownloadManagerPro"; ValueData: """{app}\{#AppExe}"" --background"; Flags: uninsdeletevalue; Tasks: startup
; Register the native messaging host for Chromium browsers (per user, no admin needed).
Root: HKCU; Subkey: "Software\Google\Chrome\NativeMessagingHosts\com.dmpro.native_host"; ValueType: string; ValueName: ""; ValueData: "{app}\com.dmpro.native_host.json"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Microsoft\Edge\NativeMessagingHosts\com.dmpro.native_host"; ValueType: string; ValueName: ""; ValueData: "{app}\com.dmpro.native_host.json"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\com.dmpro.native_host"; ValueType: string; ValueName: ""; ValueData: "{app}\com.dmpro.native_host.json"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Chromium\NativeMessagingHosts\com.dmpro.native_host"; ValueType: string; ValueName: ""; ValueData: "{app}\com.dmpro.native_host.json"; Flags: uninsdeletekey

[Run]
Filename: "{app}\{#AppExe}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: files; Name: "{app}\*.bak"
Type: files; Name: "{app}\*.partial"
