#ifndef SourceDir
  #error Pass /DSourceDir with the directory containing the release executables
#endif
#ifndef AppVersion
  #error Pass /DAppVersion with the Cargo package version
#endif

[Setup]
AppId={{6B2DFB89-4830-4F98-91C5-55717CC7AC09}
AppName=ReimsBridge
AppVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\ReimsBridge
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\reimsbridge-tray.exe
AppMutex=Local\ReimsBridge.Tray
CloseApplications=yes
RestartApplications=no
OutputBaseFilename=ReimsBridge-Windows-x64-Setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Files]
Source: "{#SourceDir}\reimsbridge-tray.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\reimsbridge-windows.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\ReimsBridge"; Filename: "{app}\reimsbridge-tray.exe"; WorkingDir: "{app}"

[Run]
Filename: "{app}\reimsbridge-tray.exe"; Description: "Launch ReimsBridge"; Flags: nowait postinstall skipifsilent

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  RunValue = 'ReimsBridge';
  TrayCommandTail = '\reimsbridge-tray.exe" --login';

function InstalledLoginCommand(): String;
begin
  Result := '"' + ExpandConstant('{app}\reimsbridge-tray.exe') + '" --login';
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  PreviousCommand: String;
begin
  if CurStep = ssPostInstall then
    if RegQueryStringValue(HKCU, RunKey, RunValue, PreviousCommand) then
      if Length(PreviousCommand) >= Length(TrayCommandTail) then
        if CompareText(Copy(PreviousCommand, Length(PreviousCommand) -
          Length(TrayCommandTail) + 1, Length(TrayCommandTail)), TrayCommandTail) = 0 then
          if not RegWriteStringValue(HKCU, RunKey, RunValue, InstalledLoginCommand()) then
            Log('Could not update the existing ReimsBridge login entry');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  CurrentCommand: String;
begin
  if CurUninstallStep = usUninstall then
    if RegQueryStringValue(HKCU, RunKey, RunValue, CurrentCommand) then
      if CompareText(CurrentCommand, InstalledLoginCommand()) = 0 then
        if not RegDeleteValue(HKCU, RunKey, RunValue) then
          Log('Could not remove the installed ReimsBridge login entry');
end;
