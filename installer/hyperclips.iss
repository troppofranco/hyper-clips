; Hyper Clips — per-user installer (no administrator rights).
; Built by .github/workflows/release.yml with:  ISCC.exe installer\hyperclips.iss
; The version comes from the HC_VERSION environment variable.

#define AppVersion GetEnv("HC_VERSION")
#if AppVersion == ""
  #define AppVersion "0.0.0-dev"
#endif
#define AppName "Hyper Clips"

[Setup]
AppId={{8E3B7C2A-5F41-4A8B-9C3E-2D7F6A1B0C9E}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Hyper Clips contributors
DefaultDirName={localappdata}\Programs\HyperClips
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=HyperClips-Setup-{#AppVersion}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\hyperclips.exe
UninstallDisplayName={#AppName}
CloseApplications=force
RestartApplications=no
LicenseFile=..\LICENSE
ShowLanguageDialog=auto
; No web pages, no bundled offers, no telemetry.

[Languages]
Name: "it"; MessagesFile: "compiler:Languages\Italian.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
it.Autostart=Avvia Hyper Clips con Windows
en.Autostart=Start Hyper Clips with Windows
it.DeleteData=Eliminare anche le clip, la libreria e le impostazioni di Hyper Clips?%n%nScegli No per conservarli (consigliato).
en.DeleteData=Also delete Hyper Clips clips, library and settings?%n%nChoose No to keep them (recommended).

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "startmenu"; Description: "{cm:CreateQuickLaunchIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "autostart"; Description: "{cm:Autostart}"; Flags: unchecked

[Files]
Source: "..\target\release\hyperclips.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\hyperclips-studio.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\hc-updater-helper.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\locales\*"; DestDir: "{app}\locales"; Flags: recursesubdirs ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\{#AppName}"; Filename: "{app}\hyperclips.exe"; AppUserModelID: "HyperClips.HyperClips"; Tasks: startmenu
Name: "{userdesktop}\{#AppName}"; Filename: "{app}\hyperclips.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "HyperClips"; ValueData: """{app}\hyperclips.exe"" --background"; Flags: uninsdeletevalue; Tasks: autostart

[Run]
Filename: "{app}\hyperclips.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{app}\hyperclips.exe"; Parameters: "--quit"; Flags: runhidden waituntilterminated; RunOnceId: "QuitCore"

[Code]
procedure DeleteWerExclusion(Exe: String);
begin
  RegDeleteValue(HKCU, 'Software\Microsoft\Windows\Windows Error Reporting\ExcludedApplications', Exe);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Videos: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    DeleteWerExclusion('hyperclips.exe');
    DeleteWerExclusion('hyperclips-studio.exe');
    DeleteWerExclusion('hc-updater-helper.exe');
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'HyperClips');
    { Default answer is No: user data is kept unless explicitly removed. }
    if (not UninstallSilent) and
       (MsgBox(ExpandConstant('{cm:DeleteData}'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES) then
    begin
      DelTree(ExpandConstant('{userappdata}\HyperClips'), True, True, True);
      DelTree(ExpandConstant('{localappdata}\HyperClips'), True, True, True);
      if RegQueryStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders', 'My Video', Videos) then
        DelTree(Videos + '\Hyper Clips', True, True, True);
    end;
  end;
end;
