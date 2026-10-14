; Hoppy installer for Windows (Inno Setup 6). Built by .github/workflows/release.yml:
;   iscc /DAppVersion=1.2.3 /DSourceExe=path\to\hoppy.exe apps\windows\hoppy.iss
; Per-user install, no admin rights: %LOCALAPPDATA%\Programs\Hoppy.

#ifndef AppVersion
  #define AppVersion "0.0.0-dev"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\hoppy.exe"
#endif

[Setup]
AppId={{6F3C2B1A-8D4E-4B7A-9C55-48F0A1D3E7B2}
AppName=Hoppy
AppVersion={#AppVersion}
AppPublisher=The Hoppy contributors
AppPublisherURL=https://github.com/Marketto/hoppy
AppSupportURL=https://github.com/Marketto/hoppy/issues
DefaultDirName={localappdata}\Programs\Hoppy
DisableDirPage=yes
DisableProgramGroupPage=yes
DefaultGroupName=Hoppy
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\..\dist
OutputBaseFilename=Hoppy-Setup-x64
SetupIconFile=..\..\assets\brand\hoppy.ico
UninstallDisplayIcon={app}\hoppy.exe
WizardStyle=modern
Compression=lzma2
SolidCompression=yes
CloseApplications=force
LicenseFile=..\..\LICENSE

[Languages]
Name: "it"; MessagesFile: "compiler:Languages\Italian.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
it.StartNow=Avvia Hoppy adesso (pronto a ricevere)
en.StartNow=Start Hoppy now (ready to receive)

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Hoppy"; Filename: "{app}\hoppy.exe"; Parameters: "receive"; Comment: "Hoppy"

[Run]
; Adds "Send to → Hoppy" and the minimized receiver at login (see crates/hoppy-cli/src/platform.rs).
Filename: "{app}\hoppy.exe"; Parameters: "install"; Flags: runhidden waituntilterminated
Filename: "{app}\hoppy.exe"; Parameters: "receive"; Description: "{cm:StartNow}"; Flags: postinstall nowait runminimized skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM hoppy.exe /F"; Flags: runhidden; RunOnceId: "StopHoppy"
Filename: "{app}\hoppy.exe"; Parameters: "uninstall"; Flags: runhidden; RunOnceId: "RemoveShortcuts"

[Messages]
it.FinishedLabel=Hoppy è installato. Tasto destro su un file → Invia a → Hoppy.
en.FinishedLabel=Hoppy is installed. Right-click a file → Send to → Hoppy.
