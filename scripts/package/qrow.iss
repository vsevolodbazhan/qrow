; The Qrow Windows installer. scripts/package/windows.py compiles it with the
; pinned Inno Setup compiler and supplies these definitions:
;   Version       release version, such as 1.2.3 or 1.2.3-nightly.20261111.4
;   FileVersion   numeric version of the installer file, such as 1.2.3.0
;   Architecture  Inno Setup architecture identifier: x64compatible or arm64
;   Source        folder with qrow.exe and the license files
;   Icon          the application icon (.ico)
;   OutputDir     folder for the installer
;   OutputName    installer file name without .exe
#if !Defined(Version) || !Defined(FileVersion) || !Defined(Architecture) || !Defined(Source) || !Defined(Icon) || !Defined(OutputDir) || !Defined(OutputName)
  #error Run scripts/package/windows.py to compile the installer
#endif

[Setup]
; Never change AppId: upgrades and the uninstaller find earlier installs by it.
AppId={{9003EED7-6070-41D5-A2D5-1BD3A9008063}
AppName=Qrow
AppVersion={#Version}
AppVerName=Qrow {#Version}
AppPublisher=Qrow
AppPublisherURL=https://qrow.dev/
AppSupportURL=https://github.com/vsevolodbazhan/qrow/issues
VersionInfoVersion={#FileVersion}
VersionInfoProductVersion={#FileVersion}
VersionInfoProductTextVersion={#Version}
; A per-user install needs no administrator. The dialog lets the user install
; for all users instead.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
DefaultDirName={autopf}\Qrow
DisableProgramGroupPage=yes
DisableReadyPage=yes
ArchitecturesAllowed={#Architecture}
ArchitecturesInstallIn64BitMode={#Architecture}
WizardStyle=modern dynamic
SetupIconFile={#Icon}
UninstallDisplayIcon={app}\qrow.exe
UninstallDisplayName=Qrow
; Restart Manager closes a running Qrow before an upgrade replaces qrow.exe.
CloseApplications=yes
RestartApplications=no
Compression=lzma2/max
SolidCompression=yes
SourceDir={#Source}
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}

[Tasks]
Name: desktopicon; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "qrow.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "NOTICE"; DestDir: "{app}"; Flags: ignoreversion
Source: "THIRD_PARTY_NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Qrow"; Filename: "{app}\qrow.exe"
Name: "{autodesktop}\Qrow"; Filename: "{app}\qrow.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\qrow.exe"; Description: "{cm:LaunchProgram,Qrow}"; Flags: nowait postinstall skipifsilent
; An update from Qrow runs the installer silently with /QROWRELAUNCH=1 and
; quits. Start Qrow again when the files are in place, as the user who ran
; it, also when the installer runs as administrator for all users.
Filename: "{app}\qrow.exe"; Flags: nowait runasoriginaluser; Check: IsUpdateRelaunch

[Code]
function IsUpdateRelaunch: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:QROWRELAUNCH|0}') = '1');
end;
