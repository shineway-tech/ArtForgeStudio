#define AppName "ArtForgeStudio"
#define AppExeName "ArtForgeStudio.exe"
#define AppVersion GetEnv("ARTFORGE_APP_VERSION")
#define PackageDir GetEnv("ARTFORGE_PACKAGE_DIR")
#define ReleaseDir GetEnv("ARTFORGE_RELEASE_DIR")

#if AppVersion == ""
  #error "ARTFORGE_APP_VERSION is required"
#endif
#if PackageDir == ""
  #error "ARTFORGE_PACKAGE_DIR is required"
#endif
#if ReleaseDir == ""
  #error "ARTFORGE_RELEASE_DIR is required"
#endif

[Setup]
AppId={{DB6417C1-ACF9-41D6-956F-898E69F7CE3E}
AppName={#AppName}
AppVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#ReleaseDir}
OutputBaseFilename={#AppName}_{#AppVersion}_windows_x64_setup
SetupIconFile=..\native-client\assets\app.ico
UninstallDisplayIcon={app}\{#AppExeName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "chinesesimp"; MessagesFile: "languages\ChineseSimplified.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务："; Flags: unchecked

[Files]
Source: "{#PackageDir}\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs

[Dirs]
Name: "{app}\data\input"; Flags: uninsneveruninstall
Name: "{app}\data\out"; Flags: uninsneveruninstall
Name: "{app}\data\prompt"; Flags: uninsneveruninstall

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExeName}"; Description: "启动 {#AppName}"; Flags: nowait postinstall skipifsilent
