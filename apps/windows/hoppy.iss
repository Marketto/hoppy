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
; Running copies are stopped first by CurUninstallStepChanged (see [Code]
// Stops hoppy.exe / hoppyw.exe, but only the copies running from this install folder (an
// unrelated "hoppy.exe" elsewhere is left alone). Upgrades from 0.1.0 run `hoppy.exe
// receive` in a console, which CloseApplications cannot close, nor the tray app.
// The folder goes to PowerShell as a single-quoted literal (quotes doubled) and is
// compared with StartsWith, not -like, so '[' ']' in the path are not wildcards.
function PsQuote(S: String): String;
begin
  StringChangeEx(S, '''', '''''', True);
  Result := '''' + S + '''';
end;

procedure StopHoppy();
var
  Script: String;
  ResultCode: Integer;
begin
  Script :=
    '$d = ' + PsQuote(AddBackslash(ExpandConstant('{app}'))) + '; ' +
    '$p = @(Get-Process -Name hoppy,hoppyw -ErrorAction SilentlyContinue | ' +
    'Where-Object { $_.Path -and $_.Path.StartsWith($d, [StringComparison]::OrdinalIgnoreCase) }); ' +
    '$p | Stop-Process -Force -ErrorAction SilentlyContinue; ' +
    '$p | Wait-Process -Timeout 5 -ErrorAction SilentlyContinue';
  Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "' + Script + '"',
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopHoppy();
  Result := '';
end;

// usUninstall comes after the user confirms and before [UninstallRun] / file removal.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    StopHoppy();
end;
