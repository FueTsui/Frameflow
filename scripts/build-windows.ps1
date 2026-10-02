[CmdletBinding()]
param(
    [ValidateSet('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc', 'x86_64-pc-windows-gnu')]
    [string]$Target = 'x86_64-pc-windows-msvc',
    [string]$BuildDirectory,
    [string]$Toolchain,
    [string]$InnoCompiler,
    [string]$FFmpegDirectory,
    [switch]$SkipBuild,
    [switch]$SkipInstaller
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$targetRoot = if ($BuildDirectory) { [IO.Path]::GetFullPath($BuildDirectory) } else { Join-Path $projectRoot 'target' }
$distRoot = Join-Path $projectRoot 'dist'
$stageRoot = $null
$completed = $false

function Write-Sha256([string]$Path) {
    $digest = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    "$digest  $([IO.Path]::GetFileName($Path))" | Set-Content -LiteralPath "$Path.sha256" -Encoding ascii
}

function Read-ToolOutput([string]$Path, [string]$Argument) {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Path
    $start.Arguments = $Argument
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    try {
        if (-not $process.Start()) { throw "Cannot start $Path." }
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(30000)) {
            $process.Kill()
            throw "Timed out validating $Path $Argument."
        }
        $output = $stdout.GetAwaiter().GetResult() + $stderr.GetAwaiter().GetResult()
        if ($process.ExitCode -ne 0) { throw "$Path $Argument failed ($($process.ExitCode))." }
        return $output.Trim()
    }
    finally { $process.Dispose() }
}

function Read-ToolArchitecture([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    $reader = [IO.BinaryReader]::new($stream)
    try {
        if ($reader.ReadUInt16() -ne 0x5A4D) { throw "Not a Windows executable: $Path" }
        $stream.Position = 0x3C
        $peOffset = $reader.ReadUInt32()
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) { throw "Invalid PE header: $Path" }
        switch ($reader.ReadUInt16()) {
            0x8664 { return 'x64' }
            0xAA64 { return 'arm64' }
            default { throw "Unsupported FFmpeg executable architecture: $Path" }
        }
    }
    finally { $reader.Dispose(); $stream.Dispose() }
}

function Read-BundledTool([string]$Name, [string]$Directory, [string]$Architecture) {
    $path = Join-Path $Directory "$Name.exe"
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Bundled FFmpeg is required: missing $path. Place ffmpeg.exe and ffprobe.exe in tools/bin, or use -FFmpegDirectory."
    }
    $machine = Read-ToolArchitecture $path
    if ($machine -ne $Architecture) { throw "$Name is $machine, but the package is $Architecture. Use -FFmpegDirectory with matching tools." }
    $versionOutput = Read-ToolOutput $path '-version'
    $versionMatch = [regex]::Match($versionOutput, "(?m)^$Name version (\S+)[^\r\n]*")
    if (-not $versionMatch.Success) { throw "$path did not report a valid $Name version." }
    $buildOutput = Read-ToolOutput $path '-buildconf'
    if ($buildOutput -notmatch '--enable-static' -or $buildOutput -match '--enable-shared') {
        throw "Use a self-contained static FFmpeg build for packaging: $path"
    }
    $licenseOutput = Read-ToolOutput $path '-L'
    if ($licenseOutput -notmatch 'GNU General Public License' -or $licenseOutput -notmatch 'either version 3') {
        throw "The bundled license documentation expects GPL-3.0-or-later. Update it before packaging another license variant: $path"
    }
    return [pscustomobject]@{
        Path = $path
        Metadata = [ordered]@{
            version = $versionMatch.Groups[1].Value
            version_line = $versionMatch.Value
            sha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
            architecture = $machine
            path = "tools/$Name.exe"
        }
        VersionOutput = $versionOutput
        BuildOutput = $buildOutput
        LicenseOutput = $licenseOutput
    }
}

Push-Location -LiteralPath $projectRoot
try {
    $architecture = if ($Target.StartsWith('aarch64')) { 'arm64' } else { 'x64' }
    $ffmpegRoot = if ($FFmpegDirectory) { [IO.Path]::GetFullPath($FFmpegDirectory) } else { Join-Path $projectRoot 'tools/bin' }
    $bundledFFmpeg = Read-BundledTool 'ffmpeg' $ffmpegRoot $architecture
    $bundledFFprobe = Read-BundledTool 'ffprobe' $ffmpegRoot $architecture
    if ($bundledFFmpeg.Metadata.version -ne $bundledFFprobe.Metadata.version) {
        throw 'ffmpeg and ffprobe must come from the same FFmpeg build.'
    }
    $ffmpegLicensePath = Join-Path $projectRoot 'docs/FFmpeg-GPL-3.0.txt'
    if (-not (Test-Path -LiteralPath $ffmpegLicensePath -PathType Leaf)) { throw 'The full FFmpeg GPLv3 license text is missing.' }
    $ffmpegSource = [ordered]@{
        vendor = 'Custom FFmpeg build; consult the supplied provider documentation'
        release_url = $null
        source_url = $null
        corresponding_source_bundled = $false
    }
    if ($bundledFFmpeg.Metadata.version -eq '9.0.1-essentials_build-www.gyan.dev') {
        $ffmpegSource.vendor = 'Gyan Doshi / gyan.dev'
        $ffmpegSource.release_url = 'https://github.com/GyanD/codexffmpeg/releases/tag/9.0.1'
        $ffmpegSource.source_url = 'https://github.com/FFmpeg/FFmpeg/commit/bf1b838f2a'
    }
    elseif ($bundledFFmpeg.Metadata.version -eq '9.0.2-essentials_build-www.gyan.dev') {
        $ffmpegSource.vendor = 'Gyan Doshi / gyan.dev'
        $ffmpegSource.release_url = 'https://github.com/GyanD/codexffmpeg/releases/tag/9.0.2'
        $ffmpegSource.source_url = 'https://github.com/FFmpeg/FFmpeg/commit/946fcce07b'
    }
    if (-not $SkipInstaller) {
        if (-not $InnoCompiler) {
            $compilerCommand = Get-Command ISCC.exe -CommandType Application -ErrorAction SilentlyContinue
            if ($compilerCommand) { $InnoCompiler = $compilerCommand.Source }
            else {
                foreach ($candidate in @(
                    (Join-Path ([Environment]::GetEnvironmentVariable('ProgramFiles(x86)')) 'Inno Setup 6/ISCC.exe'),
                    (Join-Path $env:ProgramFiles 'Inno Setup 6/ISCC.exe')
                )) {
                    if (Test-Path -LiteralPath $candidate -PathType Leaf) { $InnoCompiler = $candidate; break }
                }
            }
        }
        if (-not $InnoCompiler -or -not (Test-Path -LiteralPath $InnoCompiler -PathType Leaf)) {
            throw 'Inno Setup 6 was not found. Install it or use -SkipInstaller for only the portable ZIP.'
        }
        $InnoCompiler = (Resolve-Path -LiteralPath $InnoCompiler).ProviderPath
        $innoRoot = Split-Path -Parent $InnoCompiler
        $chineseLanguage = Join-Path $innoRoot 'Languages/ChineseSimplified.isl'
        $hasChineseLanguage = Test-Path -LiteralPath $chineseLanguage -PathType Leaf
        if (-not $hasChineseLanguage) { Write-Warning 'ChineseSimplified.isl is absent; the installer will use the bundled English messages.' }
    }

    if (-not $SkipBuild) {
        $cargoExecutable = Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue
        $cargoCommand = if ($cargoExecutable) { $cargoExecutable.Source } else { Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' }
        if (-not (Test-Path -LiteralPath $cargoCommand -PathType Leaf)) { throw 'Cargo was not found. Install Rust stable first.' }
        $cargoArguments = @()
        if ($Toolchain) { $cargoArguments += "+$Toolchain" }
        elseif ($Target.EndsWith('-msvc')) { $cargoArguments += '+stable-x86_64-pc-windows-msvc' }
        $cargoArguments += @('build', '--locked', '--release', '--target', $Target, '--target-dir', $targetRoot)
        & $cargoCommand @cargoArguments
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE). The selected toolchain must already be installed and healthy." }
    }

    $binaryPath = Join-Path $targetRoot "$Target/release/frameflow.exe"
    if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) { throw "Executable not found: $binaryPath. Run without -SkipBuild first." }
    $manifest = Get-Content -LiteralPath (Join-Path $projectRoot 'Cargo.toml') -Raw
    $versionMatch = [regex]::Match($manifest, '(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.-]+)?)"')
    if (-not $versionMatch.Success) { throw 'Cannot read the package version from Cargo.toml.' }
    $version = $versionMatch.Groups[1].Value
    $fileVersion = [Diagnostics.FileVersionInfo]::GetVersionInfo($binaryPath)
    if ($fileVersion.ProductVersion -ne $version) { throw "Executable version '$($fileVersion.ProductVersion)' differs from Cargo version '$version'. Rebuild before packaging." }
    if (-not $fileVersion.Comments -or -not $fileVersion.Comments.Contains("target=$Target; profile=release")) {
        throw 'Executable is missing matching release build provenance. Rebuild with the current build.rs before packaging.'
    }
    $packageName = "Frameflow-$version-windows-$architecture"
    $installerName = "Frameflow-$version-windows-$architecture-setup"
    $stageRoot = Join-Path $distRoot (Join-Path '.staging' ([guid]::NewGuid().ToString('N')))
    $packageRoot = Join-Path $stageRoot $packageName
    New-Item -ItemType Directory -Path $packageRoot -Force | Out-Null
    Copy-Item -LiteralPath $binaryPath -Destination (Join-Path $packageRoot 'frameflow.exe')
    foreach ($documentName in @('README.md', 'LICENSE', 'LICENSE-MIT', 'LICENSE-APACHE', 'THIRD_PARTY_NOTICES.md', 'CHANGELOG.md', 'FFmpeg功能检查-0.2.1.md')) {
        $documentPath = Join-Path $projectRoot $documentName
        if (Test-Path -LiteralPath $documentPath -PathType Leaf) { Copy-Item -LiteralPath $documentPath -Destination $packageRoot }
    }
    $documentationRoot = Join-Path $projectRoot 'docs'
    if (Test-Path -LiteralPath $documentationRoot -PathType Container) {
        $packageDocs = Join-Path $packageRoot 'docs'
        New-Item -ItemType Directory -Path $packageDocs -Force | Out-Null
        Get-ChildItem -LiteralPath $documentationRoot -Filter '*.md' -File | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $packageDocs }
        Copy-Item -LiteralPath $ffmpegLicensePath -Destination $packageDocs
    }
    $screenshotRoot = Join-Path $projectRoot 'docs/screenshots'
    if (Test-Path -LiteralPath $screenshotRoot -PathType Container) {
        $packageScreenshots = Join-Path $packageRoot 'docs/screenshots'
        New-Item -ItemType Directory -Path $packageScreenshots -Force | Out-Null
        Get-ChildItem -LiteralPath $screenshotRoot -Filter '*.png' -File | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $packageScreenshots }
    }
    $toolsRoot = Join-Path $packageRoot 'tools'
    New-Item -ItemType Directory -Path $toolsRoot | Out-Null
    foreach ($tool in @($bundledFFmpeg, $bundledFFprobe)) {
        $toolName = [IO.Path]::GetFileNameWithoutExtension($tool.Path)
        $stagedToolPath = Join-Path $toolsRoot "$toolName.exe"
        Copy-Item -LiteralPath $tool.Path -Destination $stagedToolPath
        if ((Get-FileHash -LiteralPath $stagedToolPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $tool.Metadata.sha256) {
            throw "Bundled $toolName changed while packaging."
        }
        $tool.VersionOutput | Set-Content -LiteralPath (Join-Path $toolsRoot "$toolName-version.txt") -Encoding UTF8
        $tool.BuildOutput | Set-Content -LiteralPath (Join-Path $toolsRoot "$toolName-buildconf.txt") -Encoding UTF8
        $tool.LicenseOutput | Set-Content -LiteralPath (Join-Path $toolsRoot "$toolName-license.txt") -Encoding UTF8
    }
    # Preserve provider notices when supplied next to the tools or in their archive root.
    $providerDocs = Join-Path $toolsRoot 'provider-notices'
    $noticeRoots = @(@($ffmpegRoot, (Split-Path -Parent $ffmpegRoot)) | Select-Object -Unique)
    for ($noticeIndex = 0; $noticeIndex -lt $noticeRoots.Count; $noticeIndex++) {
        $notices = Get-ChildItem -LiteralPath $noticeRoots[$noticeIndex] -File | Where-Object {
            $_.Name -match '^(LICENSE|COPYING|NOTICE|README|AUTHORS|CREDITS)([.-].*)?$'
        }
        foreach ($notice in $notices) {
            $noticeDestination = Join-Path $providerDocs ([string]$noticeIndex)
            New-Item -ItemType Directory -Path $noticeDestination -Force | Out-Null
            Copy-Item -LiteralPath $notice.FullName -Destination $noticeDestination
        }
    }
    $ffmpegSource | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $toolsRoot 'SOURCE_INFO.json') -Encoding UTF8
    @'
已内置 ffmpeg.exe 和 ffprobe.exe，安装后或解压后即可使用，无需另外下载。
如需使用自己的 FFmpeg，可在左侧“设置”中选择包含这两个程序的文件夹。
版本、构建参数、许可声明和来源记录保存在本目录；完整 GPLv3 许可见 ../docs/FFmpeg-GPL-3.0.txt。
详细来源与源码说明见 ../docs/ffmpeg-bundle.md。

ffmpeg.exe and ffprobe.exe are bundled and ready to use.
To override them, select your FFmpeg folder from the Settings page.
See the adjacent version/build/license records and ../docs/ffmpeg-bundle.md for provenance.
'@ | Set-Content -LiteralPath (Join-Path $toolsRoot 'README.txt') -Encoding UTF8
    $binaryHash = (Get-FileHash -LiteralPath $binaryPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $signature = Get-AuthenticodeSignature -LiteralPath $binaryPath
    $signatureDescription = if ($signature.Status -eq 'NotSigned') { 'Unsigned (no Authenticode signature)' } else { [string]$signature.Status }
    @"
Frameflow $version / Windows $architecture
Target: $Target
Compiler: $($fileVersion.Comments)
Executable SHA-256: $binaryHash
Code signature: $signatureDescription
FFmpeg: bundled / $($bundledFFmpeg.Metadata.version)
FFmpeg SHA-256: $($bundledFFmpeg.Metadata.sha256)
ffprobe SHA-256: $($bundledFFprobe.Metadata.sha256)
FFmpeg license: GPL-3.0-or-later; see docs/FFmpeg-GPL-3.0.txt and tools/*-license.txt.
Installer: per-user; administrator privileges not required; uninstall retains user preferences.
Default installation directory: %LOCALAPPDATA%\Programs\Frameflow
"@ | Set-Content -LiteralPath (Join-Path $packageRoot 'BUILD_INFO.txt') -Encoding UTF8
    [ordered]@{
        version = $version
        target = $Target
        architecture = $architecture
        compiler_provenance = $fileVersion.Comments
        binary_sha256 = $binaryHash
        authenticode = [string]$signature.Status
        ffmpeg_bundled = $true
        ffmpeg = $bundledFFmpeg.Metadata
        ffprobe = $bundledFFprobe.Metadata
        ffmpeg_license = 'GPL-3.0-or-later'
        ffmpeg_source = $ffmpegSource
        installer_scope = 'per-user'
        installer_default_directory = '%LOCALAPPDATA%\Programs\Frameflow'
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $packageRoot 'BUILD_INFO.json') -Encoding UTF8

    $stagedArchive = Join-Path $stageRoot "$packageName.zip"
    Compress-Archive -LiteralPath $packageRoot -DestinationPath $stagedArchive -CompressionLevel Optimal
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($stagedArchive)
    try {
        $binaryEntry = $zip.GetEntry("$packageName/frameflow.exe")
        if (-not $binaryEntry) { throw 'Portable ZIP has no executable.' }
        $entryStream = $binaryEntry.Open()
        $hasher = [Security.Cryptography.SHA256]::Create()
        try { $archivedHash = ([BitConverter]::ToString($hasher.ComputeHash($entryStream))).Replace('-', '').ToLowerInvariant() }
        finally { $entryStream.Dispose(); $hasher.Dispose() }
        if ($archivedHash -ne $binaryHash) { throw 'Portable executable differs from the release build.' }
        foreach ($tool in @($bundledFFmpeg, $bundledFFprobe)) {
            $toolEntry = $zip.GetEntry("$packageName/$($tool.Metadata.path)")
            if (-not $toolEntry) { throw "Portable ZIP has no $($tool.Metadata.path)." }
            $entryStream = $toolEntry.Open()
            $hasher = [Security.Cryptography.SHA256]::Create()
            try { $archivedHash = ([BitConverter]::ToString($hasher.ComputeHash($entryStream))).Replace('-', '').ToLowerInvariant() }
            finally { $entryStream.Dispose(); $hasher.Dispose() }
            if ($archivedHash -ne $tool.Metadata.sha256) { throw "Portable $($tool.Metadata.path) differs from its validated source." }
        }
    }
    finally { $zip.Dispose() }
    $artifacts = @($stagedArchive)
    if (-not $SkipInstaller) {
        $compilerArguments = @('/Qp', "/DAppVersion=$version", "/DPackageRoot=$packageRoot", "/DProjectRoot=$projectRoot", "/DOutputDirectory=$stageRoot", "/DOutputName=$installerName", "/DArchitecture=$architecture")
        if ($hasChineseLanguage) { $compilerArguments += "/DChineseLanguageFile=$chineseLanguage" }
        $compilerArguments += (Join-Path $PSScriptRoot 'frameflow.iss')
        & $InnoCompiler @compilerArguments
        if ($LASTEXITCODE -ne 0) { throw "Inno Setup compilation failed ($LASTEXITCODE)." }
        $stagedInstaller = Join-Path $stageRoot "$installerName.exe"
        if (-not (Test-Path -LiteralPath $stagedInstaller -PathType Leaf)) { throw 'Installer compiler did not create its output.' }
        $artifacts += $stagedInstaller
    }
    foreach ($artifact in $artifacts) {
        $destination = Join-Path $distRoot ([IO.Path]::GetFileName($artifact))
        Copy-Item -LiteralPath $artifact -Destination $destination -Force
        Write-Sha256 $destination
        Write-Host "Package: $destination"
    }
    $completed = $true
}
finally {
    Pop-Location
    if ($completed -and $stageRoot -and (Test-Path -LiteralPath $stageRoot -PathType Container)) {
        # Remove only this run's GUID stage, after checking its resolved workspace boundary.
        $resolvedStage = (Resolve-Path -LiteralPath $stageRoot).ProviderPath
        $allowedParent = [IO.Path]::GetFullPath((Join-Path $distRoot '.staging')).TrimEnd('\') + '\'
        $stageItem = Get-Item -LiteralPath $resolvedStage
        if (-not $resolvedStage.StartsWith($allowedParent, [StringComparison]::OrdinalIgnoreCase) -or
            $stageItem.Name -notmatch '^[a-f0-9]{32}$' -or
            ($stageItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Refusing to clean an unexpected staging path: $resolvedStage"
        }
        Remove-Item -LiteralPath $resolvedStage -Recurse -Force
    }
}
