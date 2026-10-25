; Installer Windows di UberPhoto (Inno Setup 6).
; Compilazione: iscc /DMyAppVersion=0.3.0 packaging\windows\installer.iss
; Richiede che packaging\windows\package.ps1 sia già stato eseguito.

#define MyAppName "UberPhoto"
#ifndef MyAppVersion
  #define MyAppVersion "0.0.0"
#endif
#define MyAppPublisher "Mirgen"
#define MyAppURL "https://github.com/Ogalesha/UberPhoto"
#define MyAppExeName "uberphoto.exe"

[Setup]
AppId={{23639FEF-BC57-443A-8E6D-73AD176640A5}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\..\target
OutputBaseFilename=uberphoto-windows-x86_64-setup
SetupIconFile=..\..\assets\UberPhoto.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "italian"; MessagesFile: "compiler:Languages\Italian.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\..\target\windows-package\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

; La disinstallazione rimuove anche i dati utente (indice, impostazioni,
; preset, cronologia e cache), così non restano residui nel profilo.
[UninstallDelete]
Type: filesandordirs; Name: "{userappdata}\{#MyAppName}"
Type: filesandordirs; Name: "{localappdata}\{#MyAppName}"
