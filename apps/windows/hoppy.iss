; Hoppy installer for Windows (Inno Setup 6). Built by .github/workflows/release.yml:
;   iscc /DAppVersion=1.2.3 /DSourceExe=path\to\hoppy.exe /DSourceGuiExe=path\to\hoppyw.exe apps\windows\hoppy.iss
; hoppy.exe = CLI (console); hoppyw.exe = tray receiver + "Send to" GUI (no console).
; Per-user install, no admin rights: %LOCALAPPDATA%\Programs\Hoppy.

#ifndef AppVersion
  #define AppVersion "0.0.0-dev"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\hoppy.exe"
#endif
#ifndef SourceGuiExe
  #define SourceGuiExe "..\..\target\release\hoppyw.exe"
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
UninstallDisplayIcon={app}\hoppyw.exe
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
Source: "{#SourceGuiExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Hoppy"; Filename: "{app}\hoppyw.exe"; Comment: "Hoppy"

[Run]
; Adds "Send to → Hoppy" (hoppyw send) and the tray receiver at login (hoppyw), see
; crates/hoppy-cli/src/platform.rs.
Filename: "{app}\hoppy.exe"; Parameters: "install"; Flags: runhidden waituntilterminated
Filename: "{app}\hoppyw.exe"; Description: "{cm:StartNow}"; Flags: postinstall nowait skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM hoppyw.exe /F"; Flags: runhidden; RunOnceId: "StopHoppyTray"
Filename: "{sys}\taskkill.exe"; Parameters: "/IM hoppy.exe /F"; Flags: runhidden; RunOnceId: "StopHoppy"
Filename: "{app}\hoppy.exe"; Parameters: "uninstall"; Flags: runhidden; RunOnceId: "RemoveShortcuts"

[Messages]
it.FinishedLabel=Hoppy è installato e riceve file in background: trovi la sua icona nell'area di notifica, vicino all'orologio (clic per inviare file). Per inviare da Esplora file: tasto destro su un file → Mostra altre opzioni → Invia a → Hoppy (oppure Maiusc+tasto destro).
en.FinishedLabel=Hoppy is installed and receives files in the background: look for its icon in the notification area, near the clock (click it to send files). To send from File Explorer: right-click a file → Show more options → Send to → Hoppy (or Shift+right-click).

[Code]
// Upgrades from 0.1.0 run `hoppy.exe receive` in a console; stop every running copy so
// the files can be replaced (CloseApplications cannot close a console or a tray app).
procedure StopHoppy(const Image: String);
var
  ResultCode: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM ' + Image + ' /F', '', SW_HIDE,
    ewWaitUntilTerminated, ResultCode);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopHoppy('hoppyw.exe');
  StopHoppy('hoppy.exe');
  Result := '';
end;
