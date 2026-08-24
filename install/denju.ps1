$ErrorActionPreference = "Stop"

$Owner = "amxv"
$Repo = "denju"
$ManifestName = "release-manifest.txt"
$InstallDir = if ($env:DENJU_INSTALL_DIR) { $env:DENJU_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Denju\bin" }
$KnownAssets = @(
    "denju_darwin_amd64",
    "denju_darwin_arm64",
    "denju_linux_amd64",
    "denju_linux_arm64",
    "denju_windows_amd64.exe",
    "denju_windows_arm64.exe"
)

function Get-DenjuSha256([string]$Path) {
    $Stream = [System.IO.File]::OpenRead($Path)
    try {
        $Sha256 = [System.Security.Cryptography.SHA256]::Create()
        try {
            $Hash = $Sha256.ComputeHash($Stream)
        } finally {
            $Sha256.Dispose()
        }
    } finally {
        $Stream.Dispose()
    }
    return ([System.BitConverter]::ToString($Hash)).Replace("-", "").ToLowerInvariant()
}

$Architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
$Arch = switch ($Architecture) {
    "x64" { "amd64" }
    "arm64" { "arm64" }
    default { throw "Unsupported Denju architecture: $Architecture" }
}
$Asset = "denju_windows_${Arch}.exe"

$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("denju-install-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $TempDir | Out-Null
try {
    if ($env:DENJU_RELEASE_BASE_URL) {
        $ReleaseBase = $env:DENJU_RELEASE_BASE_URL.TrimEnd('/')
        $ManifestUrl = "$ReleaseBase/$ManifestName"
    } elseif ($env:DENJU_VERSION) {
        $ReleaseBase = "https://github.com/$Owner/$Repo/releases/download/v$($env:DENJU_VERSION)"
        $ManifestUrl = "$ReleaseBase/$ManifestName"
    } else {
        $ReleaseBase = $null
        $ManifestUrl = "https://github.com/$Owner/$Repo/releases/latest/download/$ManifestName"
    }

    $ManifestPath = Join-Path $TempDir $ManifestName
    Invoke-WebRequest -UseBasicParsing -Uri $ManifestUrl -OutFile $ManifestPath
    $Format = $null
    $Version = $null
    $ServerImage = $null
    $ExpectedSha = $null
    $ExpectedSize = $null
    $SeenAssets = @{}
    foreach ($RawLine in Get-Content $ManifestPath) {
        if ([string]::IsNullOrWhiteSpace($RawLine)) { continue }
        $Fields = $RawLine.Trim() -split '\s+'
        if ($Fields.Count -eq 2 -and $Fields[0] -eq "format") {
            if ($null -ne $Format) { throw "Duplicate release manifest format" }
            $Format = $Fields[1]
        } elseif ($Fields.Count -eq 2 -and $Fields[0] -eq "version") {
            if ($null -ne $Version) { throw "Duplicate release manifest version" }
            $Version = $Fields[1]
        } elseif ($Fields.Count -eq 4 -and $Fields[0] -eq "asset") {
            $AssetName = $Fields[1]
            if ($KnownAssets -notcontains $AssetName) { throw "Unsupported release manifest asset: $AssetName" }
            if ($SeenAssets.ContainsKey($AssetName)) { throw "Duplicate release manifest asset: $AssetName" }
            if ($Fields[2] -notmatch '^[A-Fa-f0-9]{64}$' -or $Fields[3] -notmatch '^[0-9]+$') {
                throw "Invalid release manifest asset entry: $RawLine"
            }
            [Int64]$ParsedSize = 0
            if (-not [Int64]::TryParse($Fields[3], [ref]$ParsedSize) -or $ParsedSize -lt 0) {
                throw "Invalid release manifest asset size: $RawLine"
            }
            $SeenAssets[$AssetName] = $true
            if ($AssetName -eq $Asset) {
                $ExpectedSha = $Fields[2].ToLowerInvariant()
                $ExpectedSize = $ParsedSize
            }
        } elseif ($Fields.Count -eq 2 -and $Fields[0] -eq "server_image") {
            if ($null -ne $ServerImage) { throw "Duplicate release manifest server image" }
            $ServerImage = $Fields[1]
        } else {
            throw "Invalid release manifest line: $RawLine"
        }
    }
    if ($Format -ne "denju-release-manifest-v1" -or -not $Version -or $Version -notmatch '^[A-Za-z0-9.+-]{1,64}$') {
        throw "Invalid Denju release manifest"
    }
    if ($SeenAssets.Count -ne $KnownAssets.Count) { throw "Release manifest must contain exactly six client assets" }
    foreach ($KnownAsset in $KnownAssets) {
        if (-not $SeenAssets.ContainsKey($KnownAsset)) { throw "Release manifest is missing client asset: $KnownAsset" }
    }
    $ExpectedServerImage = "ghcr.io/$Owner/denju-server:v$Version"
    if ($ServerImage -ne $ExpectedServerImage) { throw "Release manifest server image must be $ExpectedServerImage" }
    if ($env:DENJU_VERSION -and $env:DENJU_VERSION -ne $Version) {
        throw "Release manifest version $Version does not match requested $($env:DENJU_VERSION)"
    }
    if (-not $ExpectedSha) { throw "Release manifest has no asset for Windows/$Arch" }
    if (-not $ReleaseBase) { $ReleaseBase = "https://github.com/$Owner/$Repo/releases/download/v$Version" }

    $Staged = Join-Path $TempDir $Asset
    Invoke-WebRequest -UseBasicParsing -Uri "$ReleaseBase/$Asset" -OutFile $Staged
    if ((Get-Item $Staged).Length -ne $ExpectedSize) { throw "Size mismatch for $Asset" }
    $ActualSha = Get-DenjuSha256 $Staged
    if ($ActualSha -ne $ExpectedSha) { throw "Checksum mismatch for $Asset" }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $Target = Join-Path $InstallDir "denju.exe"
    $TemporaryTarget = "$Target.tmp.$PID"
    Copy-Item $Staged $TemporaryTarget
    Get-ChildItem -Path $InstallDir -Filter "denju.exe.old.*" -ErrorAction SilentlyContinue |
        Remove-Item -Force -ErrorAction SilentlyContinue
    $RetiredTarget = "$Target.old.$PID"
    $RetiredCurrent = $false
    try {
        if (Test-Path $Target) {
            Move-Item $Target $RetiredTarget
            $RetiredCurrent = $true
        }
        Move-Item $TemporaryTarget $Target
    } catch {
        Remove-Item -Force -ErrorAction SilentlyContinue $TemporaryTarget
        if ($RetiredCurrent -and -not (Test-Path $Target)) {
            Move-Item $RetiredTarget $Target -ErrorAction SilentlyContinue
        }
        throw
    }
    Remove-Item -Force -ErrorAction SilentlyContinue $RetiredTarget

    $StateDir = Join-Path $HOME ".denju"
    New-Item -ItemType Directory -Force -Path $StateDir | Out-Null
    $SourceTemporary = Join-Path $StateDir "install-source.json.tmp.$PID"
    Set-Content -NoNewline -Path $SourceTemporary -Value '{"version":1,"source":"standalone"}'
    Move-Item -Force $SourceTemporary (Join-Path $StateDir "install-source.json")

    if (-not $env:DENJU_TEST_HOME) {
        $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
        $PathEntries = @($UserPath -split ';' | Where-Object { $_ })
        if ($PathEntries -notcontains $InstallDir) {
            $NewPath = (($PathEntries + $InstallDir) -join ';')
            [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
            $env:Path = "$env:Path;$InstallDir"
        }
    }
    Write-Output "Installed denju $Version to $Target"
    Write-Output "Run: denju setup"
} finally {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $TempDir
}
