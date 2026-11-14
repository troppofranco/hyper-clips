; Hyper Clips installer (Inno Setup 6). Built by CI; version comes from HC_VERSION.
; Per-user install, no administrator rights. Silent mode: /VERYSILENT /SUPPRESSMSGBOXES /NORESTART

#define AppVersion GetEnv("HC_VERSION")
#if AppVersion == ""
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{7A6C2E1B-3D54-4F8A-9B0C-5E21D7A4C913}
AppName=Hyper Clips
AppVersion={#AppVersion}
AppPublisher=troppofranco
AppPublisherURL=https://github.com/troppofranco/hyper-clips
DefaultDirName={localappdata}\Programs\HyperClips
DefaultGroupName=Hyper Clips
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=HyperClips-Setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\hyperclips.exe
WizardStyle=modern
; Close the running app before replacing its files, never restart the machine.
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "it"; MessagesFile: "compiler:Languages\Italian.isl"

[CustomMessages]
en.TaskStartup=Start Hyper Clips when Windows starts
it.TaskStartup=Avvia Hyper Clips all'avvio di Windows
en.DeleteData=Also delete your settings, clips database and logs?%n%nChoose No to keep them (recommended).
it.DeleteData=Eliminare anche impostazioni, database delle clip e log?%n%nScegli No per conservarli (consigliato).

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "startup"; Description: "{cm:TaskStartup}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\dist\hyperclips.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\dist\hyperclips-ui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\dist\hc-updater-helper.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\dist\hyperclips-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\crates\hc-core\lang\*.toml"; DestDir: "{app}\lang"; Flags: ignoreversion

[Icons]
Name: "{group}\Hyper Clips"; Filename: "{app}\hyperclips.exe"
Name: "{autodesktop}\Hyper Clips"; Filename: "{app}\hyperclips.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "HyperClips"; ValueData: """{app}\hyperclips.exe"""; Tasks: startup; Flags: uninsdeletevalue

[Run]
; Interactive install: offer to launch at the end. Silent install (used by the updater): relaunch.
Filename: "{app}\hyperclips.exe"; Description: "{cm:LaunchProgram,Hyper Clips}"; Flags: nowait postinstall skipifsilent
Filename: "{app}\hyperclips.exe"; Flags: nowait skipifnotsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/f /im hyperclips.exe"; Flags: runhidden; RunOnceId: "KillService"
Filename: "{sys}\taskkill.exe"; Parameters: "/f /im hyperclips-ui.exe"; Flags: runhidden; RunOnceId: "KillUi"

[Code]
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    // Default is to keep the data; never ask during a silent uninstall.
    if not UninstallSilent then
      if MsgBox(CustomMessage('DeleteData'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        DelTree(ExpandConstant('{userappdata}\HyperClips'), True, True, True);
  end;
end;
