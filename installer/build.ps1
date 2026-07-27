<#
.SYNOPSIS
    Build the UniTerm Windows installer (MSI).

.DESCRIPTION
    Compiles a release binary and packages it into a per-user MSI.

    The WiX toolset is provisioned automatically into `target/installer-tools/`: a pinned,
    checksummed copy of the official WiX 3.14 binaries archive. That keeps the build
    reproducible, needs no administrator rights, installs nothing system-wide, and behaves the
    same on a developer machine as in CI. A WiX already on PATH is used in preference.

.PARAMETER SkipBuild
    Package whatever is already in target/release instead of rebuilding.

.PARAMETER Version
    Override the version. Defaults to the version in Cargo.toml.

.PARAMETER OutDir
    Where to write the MSI. Defaults to target/installer.

.PARAMETER NoDownload
    Fail rather than downloading the WiX toolset. Use when the toolchain must come from
    somewhere controlled.

.PARAMETER PinnedWix
    Ignore any WiX on PATH or in $env:WIX and use only the pinned, checksummed copy. Release
    builds should set this so the output does not depend on whichever version a build machine
    happens to have installed.

.PARAMETER CertThumbprint
    Sign the MSI with the certificate in the user's store matching this thumbprint. Requires
    signtool.exe on PATH. Unsigned installers raise a SmartScreen warning on first download.

.EXAMPLE
    .\installer\build.ps1
    Build a release binary and produce target/installer/UniTerm-3.0.0-x64.msi

.EXAMPLE
    .\installer\build.ps1 -SkipBuild -Version 3.1.0
#>
[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [string]$Version,
    [string]$OutDir,
    [switch]$NoDownload,
    [switch]$PinnedWix,
    [string]$CertThumbprint
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Pinned toolchain. Bumping these two lines together is the only supported way to change it.
$WixVersion = '3.14.1'
$WixUrl = 'https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip'
$WixSha256 = '6AC824E1642D6F7277D0ED7EA09411A508F6116BA6FAE0AA5F2C7DAA2FF43D31'

$RepoDir = Split-Path -Parent $PSScriptRoot
$InstallerDir = $PSScriptRoot
$TargetDir = Join-Path $RepoDir 'target'
$BinDir = Join-Path $TargetDir 'release'
if (-not $OutDir) { $OutDir = Join-Path $TargetDir 'installer' }

function Write-Step($message) { Write-Host "==> $message" -ForegroundColor Cyan }
function Write-Note($message) { Write-Host "    $message" -ForegroundColor DarkGray }

<#
    Run an external program, failing on a non-zero exit code.

    Needed because Windows PowerShell wraps anything a native program writes to stderr in an
    ErrorRecord, and with $ErrorActionPreference = 'Stop' that aborts the script even when the
    program succeeded. cargo reports build progress on stderr, so this is not hypothetical.
    Exit codes are the only reliable success signal.
#>
function Invoke-Tool {
    param(
        [Parameter(Mandatory)][string]$What,
        [Parameter(Mandatory)][string]$Exe,
        [string[]]$Arguments = @()
    )
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        # Merge stderr into the output stream and print everything as plain host text. Without
        # this, each stderr line surfaces as an ErrorRecord, which litters CI logs with
        # NativeCommandError blocks that look like failures but are not.
        & $Exe @Arguments 2>&1 | ForEach-Object {
            if ($_ -is [System.Management.Automation.ErrorRecord]) {
                Write-Host $_.ToString()
            } else {
                Write-Host $_
            }
        }
        if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
    } finally {
        $ErrorActionPreference = $previous
    }
}

# ---------------------------------------------------------------------------------------
# Version
# ---------------------------------------------------------------------------------------

function Get-CargoVersion {
    $manifest = Join-Path $RepoDir 'Cargo.toml'
    foreach ($line in Get-Content $manifest) {
        # Stop at the first table after [package] so a dependency's version is never picked up.
        if ($line -match '^\s*\[' -and $line -notmatch '^\s*\[package\]') { break }
        if ($line -match '^\s*version\s*=\s*"([^"]+)"') { return $Matches[1] }
    }
    throw "Could not find a version in $manifest"
}

<#
    Convert a Cargo version into one an MSI accepts.

    MSI ProductVersion is `major.minor.build`, with major and minor at most 255 and build at most
    65535, and it has no concept of a pre-release suffix. Windows Installer also only compares
    those three fields when deciding whether an upgrade applies, so a fourth field would be
    silently ignored.
#>
function ConvertTo-MsiVersion($cargoVersion) {
    $core = ($cargoVersion -split '[-+]')[0]
    if ($core -ne $cargoVersion) {
        Write-Note "Cargo version '$cargoVersion' has a pre-release suffix; MSI does not support one, using '$core'."
    }
    $parts = $core -split '\.'
    if ($parts.Count -lt 3) { throw "Version '$cargoVersion' is not major.minor.patch" }
    $major, $minor, $build = [int]$parts[0], [int]$parts[1], [int]$parts[2]
    if ($major -gt 255 -or $minor -gt 255) { throw "MSI allows at most 255 for major and minor; got $major.$minor" }
    if ($build -gt 65535) { throw "MSI allows at most 65535 for the build field; got $build" }
    return "$major.$minor.$build"
}

# ---------------------------------------------------------------------------------------
# WiX toolset
# ---------------------------------------------------------------------------------------

function Resolve-WixBin {
    # Prefer whatever the environment already provides, unless a reproducible build was asked
    # for. Build machines often ship their own WiX, and silently using it would make the output
    # depend on the machine.
    if (-not $PinnedWix) {
        $candle = Get-Command 'candle.exe' -ErrorAction SilentlyContinue
        if ($candle) {
            Write-Note "Using WiX already on PATH: $(Split-Path -Parent $candle.Source)"
            return Split-Path -Parent $candle.Source
        }
        if ($env:WIX -and (Test-Path (Join-Path $env:WIX 'bin\candle.exe'))) {
            Write-Note "Using WiX from `$env:WIX"
            return (Join-Path $env:WIX 'bin')
        }
    }

    $localRoot = Join-Path $TargetDir "installer-tools\wix-$WixVersion"
    $localCandle = Join-Path $localRoot 'candle.exe'
    if (Test-Path $localCandle) {
        Write-Note "Using provisioned WiX at $localRoot"
        return $localRoot
    }

    if ($NoDownload) {
        throw @"
WiX $WixVersion is not available and -NoDownload was given.

Provide it one of these ways:
  * put candle.exe and light.exe on PATH, or
  * set `$env:WIX to a WiX installation, or
  * extract $WixUrl into $localRoot
"@
    }

    Write-Step "Provisioning WiX $WixVersion (once) into target/installer-tools"
    Write-Note "Nothing is installed system-wide and no administrator rights are needed."
    New-Item -ItemType Directory -Force -Path $localRoot | Out-Null
    $zip = Join-Path $TargetDir "installer-tools\wix-$WixVersion.zip"
    if (-not (Test-Path $zip)) {
        Write-Note "Downloading $WixUrl"
        Invoke-WebRequest -Uri $WixUrl -OutFile $zip -UseBasicParsing -TimeoutSec 900
    }

    $actual = (Get-FileHash $zip -Algorithm SHA256).Hash
    if ($actual -ne $WixSha256) {
        Remove-Item $zip -Force
        throw "WiX archive checksum mismatch.`n  expected $WixSha256`n  actual   $actual`nThe download was discarded."
    }
    Write-Note "Checksum verified."
    Expand-Archive -Path $zip -DestinationPath $localRoot -Force
    if (-not (Test-Path $localCandle)) { throw "candle.exe not found after extracting $zip" }
    return $localRoot
}

# ---------------------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------------------

$cargoVersion = if ($Version) { $Version } else { Get-CargoVersion }
$msiVersion = ConvertTo-MsiVersion $cargoVersion
Write-Step "UniTerm $cargoVersion (MSI version $msiVersion)"

if (-not $SkipBuild) {
    Write-Step 'Building release binary'
    Push-Location $RepoDir
    try {
        Invoke-Tool -What 'cargo build' -Exe 'cargo' -Arguments @('build', '--release')
    } finally { Pop-Location }
} else {
    Write-Note 'Skipping cargo build (-SkipBuild).'
}

$exe = Join-Path $BinDir 'UniTerm.exe'
if (-not (Test-Path $exe)) {
    throw "$exe not found. Run without -SkipBuild, or build it first with 'cargo build --release'."
}

$wixBin = Resolve-WixBin
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$objDir = Join-Path $TargetDir 'installer-obj'
New-Item -ItemType Directory -Force -Path $objDir | Out-Null

$wixObj = Join-Path $objDir 'uniterm.wixobj'
$msi = Join-Path $OutDir "UniTerm-$cargoVersion-x64.msi"

Write-Step 'Compiling installer authoring (candle)'
Invoke-Tool -What 'candle' -Exe (Join-Path $wixBin 'candle.exe') -Arguments @(
    '-nologo'
    '-arch', 'x64'
    "-dVersion=$msiVersion"
    "-dBinDir=$BinDir"
    "-dRepoDir=$RepoDir"
    '-ext', 'WixUIExtension'
    '-ext', 'WixUtilExtension'
    '-out', $wixObj
    (Join-Path $InstallerDir 'uniterm.wxs')
)

Write-Step 'Linking MSI (light)'
# Two validation checks are suppressed, both deliberately:
#   ICE61 - AllowSameVersionUpgrades lets a rebuild of the same version replace itself, which is
#           what you want while developing.
#   ICE91 - warns that files land in a per-user directory that does not vary with ALLUSERS. That
#           is the entire point of a per-user install.
Invoke-Tool -What 'light' -Exe (Join-Path $wixBin 'light.exe') -Arguments @(
    '-nologo'
    '-ext', 'WixUIExtension'
    '-ext', 'WixUtilExtension'
    '-cultures:en-us'
    '-sice:ICE61'
    '-sice:ICE91'
    '-spdb'
    '-out', $msi
    $wixObj
)

if ($CertThumbprint) {
    Write-Step 'Signing'
    $signtool = Get-Command 'signtool.exe' -ErrorAction SilentlyContinue
    if (-not $signtool) { throw 'signtool.exe is not on PATH. It ships with the Windows SDK.' }
    Invoke-Tool -What 'signtool' -Exe $signtool.Source -Arguments @(
        'sign'
        '/sha1', $CertThumbprint
        '/fd', 'SHA256'
        '/tr', 'http://timestamp.digicert.com'
        '/td', 'SHA256'
        $msi
    )
} else {
    Write-Note 'Not signed. Windows SmartScreen will warn on first download; pass -CertThumbprint to sign.'
}

$size = [math]::Round((Get-Item $msi).Length / 1MB, 1)
Write-Host ''
Write-Step "Built $msi ($size MB)"
Write-Host ''
Write-Host '    Install interactively:  ' -NoNewline -ForegroundColor DarkGray
Write-Host (Split-Path -Leaf $msi)
Write-Host '    Install silently:       ' -NoNewline -ForegroundColor DarkGray
Write-Host "msiexec /i `"$(Split-Path -Leaf $msi)`" /qn"
Write-Host '    Uninstall:              ' -NoNewline -ForegroundColor DarkGray
Write-Host 'Settings > Apps, or msiexec /x with the same file'
Write-Host ''
Write-Note 'Installs to %LOCALAPPDATA%\Programs\UniTerm for the current user; no UAC prompt.'
