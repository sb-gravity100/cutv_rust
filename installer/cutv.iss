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
ChangesEnvironment=yes
ArchitecturesInstallIn64BitMode=x64compatible

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked
Name: "contextmenu"; Description: "Add ""Cut with CUTV"" to the Explorer right-click menu for videos"
Name: "addtopath"; Description: "Add CUTV to PATH (run ""cutv <video>"" from a terminal)"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "cutv.cmd"; DestDir: "{app}"; Flags: ignoreversion

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

[Code]
// PATH entry for the "addtopath" task: user PATH for a per-user install, system
// PATH for an all-users one. Added once (no duplicates), removed on uninstall.
function EnvRoot: Integer;
begin
  if IsAdminInstallMode then Result := HKEY_LOCAL_MACHINE else Result := HKEY_CURRENT_USER;
end;

function EnvKey: String;
begin
  if IsAdminInstallMode then Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else Result := 'Environment';
end;

procedure CurStepChanged(CurStep: TSetupStep);
var Path, Dir: String;
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then begin
    Dir := ExpandConstant('{app}');
    if not RegQueryStringValue(EnvRoot, EnvKey, 'Path', Path) then Path := '';
    if Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') = 0 then begin
      if (Path <> '') and (Copy(Path, Length(Path), 1) <> ';') then Path := Path + ';';
      RegWriteExpandStringValue(EnvRoot, EnvKey, 'Path', Path + Dir);
      Log('Added to PATH: ' + Dir);
    end;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var Path, Dir: String; P: Integer;
begin
  if CurUninstallStep <> usPostUninstall then exit;
  if not RegQueryStringValue(EnvRoot, EnvKey, 'Path', Path) then exit;
  Dir := ExpandConstant('{app}');
  Path := ';' + Path + ';';
  P := Pos(';' + Uppercase(Dir) + ';', Uppercase(Path));
  if P = 0 then exit;
  Delete(Path, P, Length(Dir) + 1);
  Path := Copy(Path, 2, Length(Path) - 2);
  RegWriteExpandStringValue(EnvRoot, EnvKey, 'Path', Path);
  Log('Removed from PATH: ' + Dir);
end;
