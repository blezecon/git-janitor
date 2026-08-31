# git-janitor (git-jan) installer for Windows (PowerShell)
# Repository: https://github.com/blezecon/git-janitor
#
# Quick Install:
#   irm https://raw.githubusercontent.com/blezecon/git-janitor/release/install.ps1 | iex
#
# Uninstall:
#   irm https://raw.githubusercontent.com/blezecon/git-janitor/release/install.ps1 | iex -args "-Uninstall"

[CmdletBinding()]
param(
    [switch]$Uninstall,
    [string]$InstallDir = "$env:USERPROFILE\.local\bin",
    [string]$Version = ""
)

$ErrorActionPreference = "Stop"
$Repo = "blezecon/git-janitor"
$BinName = "git-janitor.exe"
$AliasName = "git-jan.exe"

function Write-Header() {
    Write-Host ""
    Write-Host "🧹 git-janitor (git-jan) Installer" -ForegroundColor Cyan
    Write-Host ""
}

function Write-Info($msg) {
    Write-Host "==> " -ForegroundColor Blue -NoNewline
    Write-Host $msg
}

function Write-Success($msg) {
    Write-Host "✓ " -ForegroundColor Green -NoNewline
    Write-Host $msg
}

function Write-Warn($msg) {
    Write-Host "⚠ " -ForegroundColor Yellow -NoNewline
    Write-Host $msg
}

function Remove-FromPath([string]$dir) {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -split ';' -contains $dir) {
        $newPath = ($userPath -split ';' | Where-Object { $_ -ne $dir -and $_ -ne "" }) -join ';'
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
        $env:Path = ($env:Path -split ';' | Where-Object { $_ -ne $dir -and $_ -ne "" }) -join ';'
        Write-Success "Removed $dir from User PATH."
    }
}

function Add-ToPath([string]$dir) {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not ($userPath -split ';' -contains $dir)) {
        $newPath = if ($userPath) { "$userPath;$dir" } else { $dir }
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
        $env:Path = "$env:Path;$dir"
        Write-Success "Added $dir to User PATH."
    }
}

function Download-WithAnimation([string]$url, [string]$outFile, [string]$label) {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $webClient = New-Object System.Net.WebClient
    $webClient.Headers.Add("User-Agent", "git-janitor-installer")

    $downloadTask = $webClient.DownloadFileTaskAsync([System.Uri]$url, $outFile)
    $frames = @('⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏')
    $frameIdx = 0

    while (-not $downloadTask.IsCompleted) {
        $frame = $frames[$frameIdx % $frames.Length]
        Write-Host "`r$frame Downloading $label (please wait)..." -ForegroundColor Cyan -NoNewline
        Start-Sleep -Milliseconds 80
        $frameIdx++
    }

    Write-Host "`r" -NoNewline
    Write-Host (" " * 60) -NoNewline
    Write-Host "`r" -NoNewline

    if ($downloadTask.IsFaulted) {
        throw $downloadTask.Exception.InnerException
    }
    Write-Success "Downloaded $label"
}

if ($Uninstall) {
    Write-Header
    Write-Info "Uninstalling git-janitor from $InstallDir..."
    $binPath = Join-Path $InstallDir $BinName
    $aliasPath = Join-Path $InstallDir $AliasName

    if (Test-Path $binPath) {
        Remove-Item $binPath -Force
        Write-Success "Removed $binPath"
    }
    if (Test-Path $aliasPath) {
        Remove-Item $aliasPath -Force
        Write-Success "Removed $aliasPath"
    }

    Remove-FromPath $InstallDir
    Write-Success "git-janitor has been completely uninstalled."
    return
}

if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    Write-Error "Git is not installed on this system. Please install Git before installing git-janitor."
    return
}

Write-Header

# Detect Architecture
$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    "AMD64" { "x86_64" }
    "ARM64" { "aarch64" }
    default {
        Write-Error "Unsupported CPU architecture: $env:PROCESSOR_ARCHITECTURE. Supported: AMD64, ARM64"
        return
    }
}

# Resolve Latest Release Version if not provided
if (-not $Version) {
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        $releaseUrl = "https://api.github.com/repos/$Repo/releases/latest"
        $releaseInfo = Invoke-RestMethod -Uri $releaseUrl -Headers @{ "User-Agent" = "git-janitor-installer" }
        $Version = $releaseInfo.tag_name
    } catch {
        $Version = "v0.1.0"
    }
}

Write-Info "Target Platform: windows-$arch ($Version)"

if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

$archiveName = "git-janitor-$Version-windows-$arch.zip"
$downloadUrl = "https://github.com/$Repo/releases/download/$Version/$archiveName"
$tempZip = Join-Path $env:TEMP $archiveName
$tempExtractDir = Join-Path $env:TEMP "git-janitor-extract-$(Get-Random)"

try {
    Download-WithAnimation -url $downloadUrl -outFile $tempZip -label $archiveName

    Write-Info "Extracting binary..."
    Expand-Archive -Path $tempZip -DestinationPath $tempExtractDir -Force

    $sourceExe = Join-Path $tempExtractDir $BinName
    if (-not (Test-Path $sourceExe)) {
        throw "Archive did not contain $BinName"
    }

    Copy-Item $sourceExe (Join-Path $InstallDir $BinName) -Force
    Copy-Item $sourceExe (Join-Path $InstallDir $AliasName) -Force

    Write-Success "Installed $BinName and $AliasName to $InstallDir"

    Add-ToPath $InstallDir

    Write-Host ""
    Write-Host "✨ git-janitor successfully installed!" -ForegroundColor Green
    Write-Host "You can now run:"
    Write-Host "  git-janitor --help" -ForegroundColor Cyan
    Write-Host "  git-jan --help" -ForegroundColor Cyan
    Write-Host "  git jan --help" -ForegroundColor Cyan
    Write-Host ""
}
finally {
    if (Test-Path $tempZip) { Remove-Item $tempZip -Force -ErrorAction SilentlyContinue }
    if (Test-Path $tempExtractDir) { Remove-Item $tempExtractDir -Recurse -Force -ErrorAction SilentlyContinue }
}
