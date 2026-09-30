[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$SdkRoot,
    [ValidateSet('x86_64-apple-darwin', 'aarch64-apple-darwin')]
    [string[]]$Target = @('x86_64-apple-darwin', 'aarch64-apple-darwin')
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$sdkPath = (Resolve-Path -LiteralPath $SdkRoot).Path
foreach ($requiredFile in @('SDKSettings.json', 'usr/lib/libSystem.tbd', 'System/Library/Frameworks/AppKit.framework/AppKit.tbd')) {
    if (-not (Test-Path -LiteralPath (Join-Path $sdkPath $requiredFile) -PathType Leaf)) {
        throw "macOS SDK is incomplete: $requiredFile"
    }
}

$rustBin = Join-Path $env:USERPROFILE '.cargo/bin'
$cargoLookup = Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue
$cargoExecutable = if ($cargoLookup) { $cargoLookup.Source } else { Join-Path $rustBin 'cargo.exe' }
$rustcLookup = Get-Command rustc -CommandType Application -ErrorAction SilentlyContinue
$rustcExecutable = if ($rustcLookup) { $rustcLookup.Source } else { Join-Path $rustBin 'rustc.exe' }
if (-not (Test-Path -LiteralPath $cargoExecutable) -or -not (Test-Path -LiteralPath $rustcExecutable)) {
    throw 'Install Rust and both macOS targets with rustup before cross-building.'
}
$sysroot = (& $rustcExecutable --print sysroot).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve the Rust sysroot.' }
$hostLine = (& $rustcExecutable -vV) | Where-Object { $_ -match '^host: ' } | Select-Object -First 1
$rustHost = $hostLine.Substring(6)
$lldExecutable = Join-Path $sysroot "lib/rustlib/$rustHost/bin/rust-lld.exe"
if (-not (Test-Path -LiteralPath $lldExecutable)) { throw "Rust linker not found: $lldExecutable" }

$helperDirectory = Join-Path $projectRoot 'tools/cross/bin'
New-Item -ItemType Directory -Force $helperDirectory | Out-Null
$linkerWrapper = Join-Path $helperDirectory 'darwin-linker.exe'
& $rustcExecutable --edition 2024 (Join-Path $PSScriptRoot 'darwin-linker.rs') -o $linkerWrapper
if ($LASTEXITCODE -ne 0) { throw 'Cannot compile the Darwin linker response-file helper.' }

$environmentNames = @('PATH', 'SDKROOT', 'MACOSX_DEPLOYMENT_TARGET', 'CARGO_ENCODED_RUSTFLAGS', 'FRAMEFLOW_RUST_LLD')
$environmentNames += $Target | ForEach-Object { 'CARGO_TARGET_' + $_.Replace('-', '_').ToUpperInvariant() + '_LINKER' }
$previousEnvironment = @{}
foreach ($name in $environmentNames) { $previousEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }

Push-Location -LiteralPath $projectRoot
try {
    $env:PATH = "$(Split-Path -Parent $rustcExecutable);$env:PATH"
    $env:SDKROOT = $sdkPath
    $env:FRAMEFLOW_RUST_LLD = $lldExecutable
    $env:MACOSX_DEPLOYMENT_TARGET = '11.0'
    # Unit-separated flags preserve SDK paths containing spaces. These affect only
    # target code when --target is supplied; host build scripts remain Windows code.
    $env:CARGO_ENCODED_RUSTFLAGS = @(
        '-C', 'linker-flavor=ld64.lld',
        '-C', 'split-debuginfo=off',
        '-C', 'link-arg=-syslibroot',
        '-C', "link-arg=$sdkPath",
        '-L', "framework=$sdkPath/System/Library/Frameworks",
        '-L', "native=$sdkPath/usr/lib"
    ) -join [char]31
    foreach ($triple in $Target) {
        $linkerVariable = 'CARGO_TARGET_' + $triple.Replace('-', '_').ToUpperInvariant() + '_LINKER'
        [Environment]::SetEnvironmentVariable($linkerVariable, $linkerWrapper, 'Process')
        Write-Host "Building $triple with SDK $sdkPath"
        & $cargoExecutable build --locked --release --target $triple --target-dir (Join-Path $projectRoot 'target')
        if ($LASTEXITCODE -ne 0) { throw "Cross-build failed for $triple ($LASTEXITCODE)." }
        Write-Host "Mach-O: $(Join-Path $projectRoot "target/$triple/release/frameflow")"
    }
}
finally {
    Pop-Location
    foreach ($name in $environmentNames) { [Environment]::SetEnvironmentVariable($name, $previousEnvironment[$name], 'Process') }
}
