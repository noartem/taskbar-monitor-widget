; Inno Setup script for the system-wide installation of Taskbar Monitor.
;   iscc /DAppVersion=1.2.3 /DArch=x64 /DSourceExe=..\target\x86_64-pc-windows-msvc\release\taskbar-monitor.exe taskbar-monitor.iss
;
; Installs to Program Files (administrator rights), and by default registers a Task Scheduler task that starts
; the widget at logon with highest privileges, so it runs as administrator without a UAC prompt.

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef Arch
  #define Arch "x64"
#endif
#ifndef SourceExe
  #define SourceExe "..\target\x86_64-pc-windows-msvc\release\taskbar-monitor.exe"
#endif

#define AppName "Taskbar Monitor"
#define AppExe "taskbar-monitor.exe"
#define TaskName "TaskbarMonitor"

[Setup]
AppId={{8F2C1E7A-5B4D-4E3A-9C61-2D7B0A9E4F13}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=noartem
AppPublisherURL=https://github.com/noartem/taskbar-monitor-widget
AppSupportURL=https://github.com/noartem/taskbar-monitor-widget/issues
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=admin
#if Arch == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif
MinVersion=10.0
OutputDir=..\dist
OutputBaseFilename=TaskbarMonitor-{#AppVersion}-{#Arch}-setup
SetupIconFile=..\assets\app.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=no

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"
Name: "pt"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"

[CustomMessages]
en.AutostartAdmin=Start with Windows as administrator (no UAC prompt)
ru.AutostartAdmin=Запускать вместе с Windows от имени администратора (без запроса UAC)
en.StartupGroup=Startup:
ru.StartupGroup=Автозапуск:
en.LaunchApp=Launch {#AppName}
ru.LaunchApp=Запустить {#AppName}
en.SettingsShortcut={#AppName} Settings
ru.SettingsShortcut=Настройки {#AppName}
es.AutostartAdmin=Iniciar con Windows como administrador (sin aviso de UAC)
es.StartupGroup=Inicio:
es.LaunchApp=Iniciar {#AppName}
es.SettingsShortcut=Ajustes de {#AppName}
fr.AutostartAdmin=Démarrer avec Windows en tant qu'administrateur (sans invite UAC)
fr.StartupGroup=Démarrage :
fr.LaunchApp=Lancer {#AppName}
fr.SettingsShortcut=Paramètres de {#AppName}
pt.AutostartAdmin=Iniciar com o Windows como administrador (sem aviso do UAC)
pt.StartupGroup=Inicialização:
pt.LaunchApp=Iniciar o {#AppName}
pt.SettingsShortcut=Configurações do {#AppName}
de.AutostartAdmin=Mit Windows als Administrator starten (ohne UAC-Abfrage)
de.StartupGroup=Autostart:
de.LaunchApp={#AppName} starten
de.SettingsShortcut={#AppName}-Einstellungen

[Tasks]
Name: "autostart"; Description: "{cm:AutostartAdmin}"; GroupDescription: "{cm:StartupGroup}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "{#AppExe}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autoprograms}\{cm:SettingsShortcut}"; Filename: "{app}\{#AppExe}"; Parameters: "--settings"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
; Registers the elevated logon task for the user who runs the installer (also records it in config.toml).
Filename: "{app}\{#AppExe}"; Parameters: "--set-autostart elevated"; Flags: runhidden waituntilterminated; Tasks: autostart
; Start right away: through the task (as administrator, in the user's session) or as a normal process.
Filename: "{sys}\schtasks.exe"; Parameters: "/Run /TN {#TaskName}"; Description: "{cm:LaunchApp}"; Flags: runhidden nowait postinstall; Tasks: autostart
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchApp}"; Flags: nowait postinstall runasoriginaluser; Tasks: not autostart

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM {#AppExe}"; Flags: runhidden; RunOnceId: "StopApp"
Filename: "{sys}\schtasks.exe"; Parameters: "/Delete /TN {#TaskName} /F"; Flags: runhidden; RunOnceId: "DeleteTask"
Filename: "{sys}\reg.exe"; Parameters: "delete HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v TaskbarMonitor /f"; Flags: runhidden; RunOnceId: "DeleteRunKey"

[Code]
// Stop a running instance so its executable can be replaced.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM {#AppExe}', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Result := '';
end;
