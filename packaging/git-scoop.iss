#if Ver != 101122304
  #error This package requires Inno Setup 6.7.1.
#endif
#ifndef PayloadDir
  #error Supply PayloadDir.
#endif
#ifndef AppVersion
  #error Supply AppVersion.
#endif
[Setup]
#ifdef TestMode
AppId={{A2AC2655-FC33-4B3A-BC68-1D05488F5DC7}
#else
AppId={{07D6E8C4-2A55-423E-BA34-9E5CCFE5C0B6}
#endif
AppName=Git Scoop
AppVersion={#AppVersion}
AppPublisher=Git Scoop contributors
DefaultDirName={localappdata}\Programs\GitScoop
DefaultGroupName=Git Scoop
DisableProgramGroupPage=no
PrivilegesRequired=lowest
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
MinVersion=10.0.22000
UninstallDisplayIcon={app}\git-scoop.exe
LicenseFile={#PayloadDir}\LICENSE.txt
InfoAfterFile={#PayloadDir}\INSTALLATION.txt
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
SetupIconFile=..\assets\branding\git-scoop.ico
CloseApplications=yes
RestartApplications=no
#ifdef TestMode
OutputBaseFilename=GitScoop-{#AppVersion}-windows-x64-setup-validation
UsePreviousAppDir=no
UsePreviousGroup=no
#else
OutputBaseFilename=GitScoop-{#AppVersion}-windows-x64-setup
#endif
[Files]
Source: "{#PayloadDir}\git-scoop.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\THIRD-PARTY-NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\INSTALLATION.txt"; DestDir: "{app}"; Flags: ignoreversion
[Icons]
Name: "{group}\Git Scoop"; Filename: "{app}\git-scoop.exe"; WorkingDir: "{app}"
Name: "{group}\Uninstall Git Scoop"; Filename: "{uninstallexe}"
[Run]
Filename: "{app}\git-scoop.exe"; Description: "Launch Git Scoop"; Flags: nowait postinstall skipifsilent
