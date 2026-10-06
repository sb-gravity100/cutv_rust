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
ArchitecturesInstallIn64BitMode=x64compatible

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent
