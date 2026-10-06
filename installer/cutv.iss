; Inno Setup script — exe-only installer. Requires the GStreamer/FFmpeg
; runtime (vcpkg install + GST_PLUGIN_PATH/PATH) already set up on the target.
; Built by scripts/release.mjs, which passes /DAppVersion=<Cargo.toml version>.
; Installs per-user (no UAC prompt), so the in-app updater can run it with
; /VERYSILENT and nothing pops up (src/updater.rs).
#define AppName "CUTV"
#ifndef AppVersion
  #error Pass /DAppVersion=x.y.z (npm run release does this)
#endif
#define AppExe "cutv_rust.exe"

[Setup]
AppId={{6C1E2B7A-4F3D-4E8B-9A21-C07F5D3E1A42}
AppName={#AppName}
AppVersion={#AppVersion}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
CloseApplications=force
OutputDir=..\target\installer
OutputBaseFilename=cutv-setup-{#AppVersion}
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
Compression=lzma2
SolidCompression=yes
ChangesAssociations=yes
ArchitecturesInstallIn64BitMode=x64compatible

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked
Name: "contextmenu"; Description: "Add ""Cut with CUTV"" to the Explorer right-click menu for videos"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion

; Per-extension verbs: the PerceivedType-level SystemFileAssociationsideo key
; doesn't show in the main right-click menu on Windows 10 (see PHASES.md).
; HKA = HKCU for a per-user install, HKLM for an all-users one. Silent updates
; keep the user's original task choice (UsePreviousTasks defaults to yes).
[Registry]
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.mp4\shell\CutWithCUTV"; ValueType: string; ValueData: "Cut with CUTV"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.mp4\shell\CutWithCUTV"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExe}"",0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.mp4\shell\CutWithCUTV\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.mov\shell\CutWithCUTV"; ValueType: string; ValueData: "Cut with CUTV"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.mov\shell\CutWithCUTV"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExe}"",0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.mov\shell\CutWithCUTV\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.avi\shell\CutWithCUTV"; ValueType: string; ValueData: "Cut with CUTV"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.avi\shell\CutWithCUTV"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExe}"",0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.avi\shell\CutWithCUTV\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.mkv\shell\CutWithCUTV"; ValueType: string; ValueData: "Cut with CUTV"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.mkv\shell\CutWithCUTV"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExe}"",0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.mkv\shell\CutWithCUTV\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.webm\shell\CutWithCUTV"; ValueType: string; ValueData: "Cut with CUTV"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.webm\shell\CutWithCUTV"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#AppExe}"",0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.webm\shell\CutWithCUTV\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: contextmenu

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent
