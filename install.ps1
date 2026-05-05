param(
    [string]$Version = $env:GHR_VERSION,
    [string]$InstallDir = $env:GHR_INSTALL_DIR
)

$ErrorActionPreference = "Stop"

$Repo = "chenyukang/ghr"
$BinName = "ghr"

if (-not $Version) {
    $Version = "latest"
}

if (-not $InstallDir) {
    $InstallDir = Join-Path $HOME ".local\bin"
}

if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
    throw "install.ps1 only supports Windows. Use install.sh on macOS or Linux."
}

$ProcessorArch = if ($env:PROCESSOR_ARCHITEW6432) {
    $env:PROCESSOR_ARCHITEW6432
} else {
    $env:PROCESSOR_ARCHITECTURE
}

switch ($ProcessorArch) {
    "AMD64" { $Target = "x86_64-pc-windows-msvc" }
    "ARM64" { $Target = "aarch64-pc-windows-msvc" }
    default { throw "unsupported Windows architecture: $ProcessorArch" }
}

$Headers = @{}
if ($env:GITHUB_TOKEN) {
    $Headers["Authorization"] = "Bearer $env:GITHUB_TOKEN"
}

if ($Version -eq "latest") {
    $Release = Invoke-RestMethod -Headers $Headers -Uri "https://api.github.com/repos/$Repo/releases/latest"
    $Tag = $Release.tag_name
    if (-not $Tag) {
        throw "failed to resolve latest release tag"
    }
} elseif ($Version.StartsWith("v")) {
    $Tag = $Version
} else {
    $Tag = "v$Version"
}

$Asset = "$BinName-$Tag-$Target"
$Archive = "$Asset.zip"
$BaseUrl = "https://github.com/$Repo/releases/download/$Tag"
$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) "ghr-install-$([System.Guid]::NewGuid())"
$ArchivePath = Join-Path $TempDir $Archive
$ChecksumPath = "$ArchivePath.sha256"
$ExtractDir = Join-Path $TempDir "extract"

New-Item -ItemType Directory -Force -Path $TempDir, $ExtractDir | Out-Null

try {
    Write-Host "ghr install: downloading $Archive"
    Invoke-WebRequest -Headers $Headers -Uri "$BaseUrl/$Archive" -OutFile $ArchivePath
    Invoke-WebRequest -Headers $Headers -Uri "$BaseUrl/$Archive.sha256" -OutFile $ChecksumPath

    $Expected = ([regex]::Match((Get-Content $ChecksumPath -Raw), "^[A-Fa-f0-9]+")).Value.ToLowerInvariant()
    $Actual = (Get-FileHash -Algorithm SHA256 $ArchivePath).Hash.ToLowerInvariant()
    if ($Expected -ne $Actual) {
        throw "checksum mismatch for $Archive`nexpected: $Expected`nactual:   $Actual"
    }

    Expand-Archive -Force -Path $ArchivePath -DestinationPath $ExtractDir
    $Binary = Get-ChildItem -Path $ExtractDir -Filter "$BinName.exe" -Recurse | Select-Object -First 1
    if (-not $Binary) {
        throw "archive did not contain $BinName.exe"
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $Destination = Join-Path $InstallDir "$BinName.exe"
    Copy-Item -Force $Binary.FullName $Destination

    $ResolvedInstallDir = (Resolve-Path $InstallDir).Path.TrimEnd("\")
    $PathContainsInstallDir = ($env:PATH -split ";") |
        Where-Object { $_.TrimEnd("\") -ieq $ResolvedInstallDir } |
        Select-Object -First 1
    if (-not $PathContainsInstallDir) {
        $UserPath = [System.Environment]::GetEnvironmentVariable("Path", "User")
        $UserPathEntries = if ($UserPath) { $UserPath -split ";" } else { @() }
        $UserPathContainsInstallDir = $UserPathEntries |
            Where-Object { $_.TrimEnd("\") -ieq $ResolvedInstallDir } |
            Select-Object -First 1
        if (-not $UserPathContainsInstallDir) {
            $NewUserPath = if ($UserPath) { "$UserPath;$ResolvedInstallDir" } else { $ResolvedInstallDir }
            [System.Environment]::SetEnvironmentVariable("Path", $NewUserPath, "User")
            Write-Host "ghr install: added $ResolvedInstallDir to your user PATH"
            Write-Host "Open a new terminal before running ghr."
        }
    }

    Write-Host "ghr install: installed $Tag to $Destination"
    Write-Host "Next: gh auth login"
    Write-Host "Run:  ghr"
} finally {
    Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
}
