$ErrorActionPreference = "Stop"

$Repo = if ($env:TOKENBOARD_REPO) { $env:TOKENBOARD_REPO } else { "james-uea/tokenboard" }
$Version = if ($env:TOKENBOARD_VERSION) { $env:TOKENBOARD_VERSION } else { "latest" }
$InstallDir = if ($env:TOKENBOARD_INSTALL_DIR) {
    $env:TOKENBOARD_INSTALL_DIR
} else {
    Join-Path $env:LOCALAPPDATA "Microsoft\WindowsApps"
}

function Get-TokenboardAsset {
    if (-not [Environment]::Is64BitOperatingSystem) {
        throw "Windows release builds currently support x86_64 only."
    }
    "tokenboard-x86_64-pc-windows-msvc.exe"
}

function Get-ReleasePath {
    if ($Version -ne "latest") {
        "download/$Version"
        return
    }

    $headers = @{ Accept = "application/vnd.github+json" }
    if ($env:GITHUB_TOKEN) {
        $headers["Authorization"] = "Bearer $env:GITHUB_TOKEN"
    }

    try {
        $release = Invoke-RestMethod `
            -Uri "https://api.github.com/repos/$Repo/releases/latest" `
            -Headers $headers `
            -UseBasicParsing

        if ($release.tag_name) {
            Write-Host "Resolved latest release: $($release.tag_name)"
            "download/$($release.tag_name)"
            return
        }
    } catch {
        Write-Warning "Could not resolve latest release tag; falling back to GitHub latest redirect."
    }

    "latest/download"
}

function Invoke-Download {
    param(
        [Parameter(Mandatory = $true)][string]$Url,
        [Parameter(Mandatory = $true)][string]$OutFile
    )

    $headers = @{}
    if ($env:GITHUB_TOKEN) {
        $headers["Authorization"] = "Bearer $env:GITHUB_TOKEN"
    }
    Invoke-WebRequest -Uri $Url -OutFile $OutFile -Headers $headers -UseBasicParsing
}

function Get-ExpectedChecksum {
    param([Parameter(Mandatory = $true)][string]$ChecksumFile)

    if (-not (Test-Path -LiteralPath $ChecksumFile)) {
        return $null
    }

    $content = Get-Content -LiteralPath $ChecksumFile -Raw
    $match = [regex]::Match($content, "\b[a-fA-F0-9]{64}\b")
    if ($match.Success) {
        $match.Value.ToLowerInvariant()
    } else {
        $null
    }
}

function Test-Checksum {
    param(
        [Parameter(Mandatory = $true)][string]$File,
        [Parameter(Mandatory = $true)][string]$ChecksumFile
    )

    $expected = Get-ExpectedChecksum -ChecksumFile $ChecksumFile
    if (-not $expected) {
        Write-Warning "No checksum asset found; skipping checksum verification."
        return
    }

    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $File).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "Checksum verification failed for $(Split-Path -Leaf $File)."
    }
    Write-Host "Checksum verified."
}

$Asset = Get-TokenboardAsset
$ReleasePath = Get-ReleasePath
$TempDir = Join-Path ([IO.Path]::GetTempPath()) ("tokenboard-install-" + [Guid]::NewGuid())
$AssetPath = Join-Path $TempDir $Asset
$ChecksumPath = "$AssetPath.sha256"
$InstallPath = Join-Path $InstallDir "tokenboard.exe"

try {
    New-Item -ItemType Directory -Force -Path $TempDir | Out-Null
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

    Write-Host "Installing Tokenboard from $Repo ($Version)"
    Write-Host "Detected release asset: $Asset"

    Invoke-Download `
        -Url "https://github.com/$Repo/releases/$ReleasePath/$Asset" `
        -OutFile $AssetPath

    try {
        Invoke-Download `
            -Url "https://github.com/$Repo/releases/$ReleasePath/$Asset.sha256" `
            -OutFile $ChecksumPath
    } catch {
        Remove-Item -LiteralPath $ChecksumPath -Force -ErrorAction SilentlyContinue
    }

    Test-Checksum -File $AssetPath -ChecksumFile $ChecksumPath
    Copy-Item -LiteralPath $AssetPath -Destination $InstallPath -Force

    Write-Host "Installed $InstallPath"
    & $InstallPath --version
    Write-Host 'Next: run `tokenboard setup` to sign in with GitHub.'

    $pathEntries = ($env:PATH -split [IO.Path]::PathSeparator) | ForEach-Object {
        $_.TrimEnd("\")
    }
    if ($pathEntries -notcontains $InstallDir.TrimEnd("\")) {
        Write-Warning "$InstallDir is not on PATH. Add it to PATH or run $InstallPath directly."
    }
} finally {
    Remove-Item -LiteralPath $TempDir -Recurse -Force -ErrorAction SilentlyContinue
}
