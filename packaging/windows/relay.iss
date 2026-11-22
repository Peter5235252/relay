; Inno Setup script for the Windows build of Relay.
; The release workflow sets RELAY_VERSION before invoking ISCC.

#define MyAppName "Relay"
#define MyAppPublisher "Relay contributors"
#define MyAppURL "https://github.com/Peter5235252/relay"
#define MyAppExeName "relay.exe"
#define RepoRoot AddBackslash(SourcePath) + "..\.."

#ifndef MyAppVersion
  #define MyAppVersion GetEnv("RELAY_VERSION")
#endif

[Setup]
AppId={{7D3B2B9E-6B7A-4A4E-9C0B-6C9E3A2F1D10}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultDirName={autopf}\Relay
DefaultGroupName=Relay
DisableProgramGroupPage=yes
LicenseFile={#RepoRoot}\LICENSE
OutputDir={#RepoRoot}\dist\release
OutputBaseFilename=relay-setup-windows-x64
SetupIconFile={#RepoRoot}\packaging\icons\relay.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
MinVersion=10.0.22000
ChangesEnvironment=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "addtopath"; Description: "Add the Relay CLI to the user PATH"; GroupDescription: "Command line:"; Flags: checkedonce

[Files]
Source: "{#RepoRoot}\dist\relay.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\dist\relay-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\dist\relay-clear.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Relay"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Relay CLI"; Filename: "{app}\relay-cli.exe"; WorkingDir: "{%USERPROFILE}"
Name: "{group}\{cm:UninstallProgram,Relay}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Relay"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,Relay}"; Flags: nowait postinstall skipifsilent

[Registry]
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}'))

[Code]
function NeedsAddPath(Param: string): boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', OrigPath) then
  begin
    Result := true;
    exit;
  end;
  Result := Pos(';' + Uppercase(Param) + ';', ';' + Uppercase(OrigPath) + ';') = 0;
end;
