; Stable per-user installation identity. Invoke via build-windows.ps1.
#ifndef AppVersion
  #error AppVersion must be supplied by build-windows.ps1
#endif

#ifdef InstallerQA
  ; Isolated acceptance runs have their own uninstall identity and no shortcuts.
  #ifndef QAAppId
    #error InstallerQA requires a unique QAAppId
  #endif
  #ifndef QADefaultDir
    #error InstallerQA requires a temporary QADefaultDir
  #endif
  #define InstallerAppId QAAppId
  #define InstallerAppName "Frameflow Installer QA"
  #define InstallerDefaultDir QADefaultDir
#else
  #define InstallerAppId "{{D295879C-B658-4909-9644-761727F06486}"
  #define InstallerAppName "帧流 Frameflow"
  #define InstallerDefaultDir "C:\Programs\Frameflow"
#endif

[Setup]
AppId={#InstallerAppId}
AppName={#InstallerAppName}
AppVersion={#AppVersion}
AppVerName={#InstallerAppName} {#AppVersion}
AppPublisher=Frameflow
DefaultDirName={#InstallerDefaultDir}
UsePreviousAppDir=yes
AppendDefaultDirName=no
DisableDirPage=no
DefaultGroupName={#InstallerAppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
MinVersion=10.0
#if Architecture == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif
OutputDir={#OutputDirectory}
OutputBaseFilename={#OutputName}
SetupIconFile={#ProjectRoot}\assets\Frameflow.ico
UninstallDisplayIcon={app}\frameflow.exe
UninstallDisplayName={#InstallerAppName}
VersionInfoVersion={#AppVersion}
VersionInfoDescription=Frameflow Setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
WizardResizable=yes
CloseApplications=yes
CloseApplicationsFilter=*.exe,*.dll
RestartApplications=no
ChangesAssociations=no
SignedUninstaller=no
UsePreviousLanguage=yes
LanguageDetectionMethod=uilanguage

[Languages]
#ifdef ChineseLanguageFile
Name: "chinesesimplified"; MessagesFile: "{#ChineseLanguageFile}"
#endif
Name: "english"; MessagesFile: "compiler:Default.isl"

#ifndef InstallerQA
[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
#endif

[Files]
; Replace only files shipped by this package, even for same-version repairs.
; No InstallDelete/UninstallDelete entries: user files and preferences survive.
Source: "{#PackageRoot}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

#ifndef InstallerQA
[Icons]
Name: "{userprograms}\帧流 Frameflow"; Filename: "{app}\frameflow.exe"; WorkingDir: "{app}"
Name: "{userdesktop}\帧流 Frameflow"; Filename: "{app}\frameflow.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\frameflow.exe"; Description: "{cm:LaunchProgram,帧流 Frameflow}"; Flags: nowait postinstall skipifsilent unchecked
#endif
