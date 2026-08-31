# git-janitor (git-jan) uninstaller for Windows (PowerShell)
# Repository: https://github.com/blezecon/git-janitor
#
# Usage:
#   irm https://raw.githubusercontent.com/blezecon/git-janitor/release/uninstall.ps1 | iex

[CmdletBinding()]
param(
    [string]$InstallDir = "$env:USERPROFILE\.local\bin"
)

$ErrorActionPreference = "Stop"
$BinName = "git-janitor.exe"
$AliasName = "git-jan.exe"

function Write-Header() {
    Write-Host ""
    Write-Host "🧹 git-janitor (git-jan) Uninstaller" -ForegroundColor Cyan
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

function Remove-FromPath([string]$dir) {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -split ';' -contains $dir) {
        $newPath = ($userPath -split ';' | Where-Object { $_ -ne $dir -and $_ -ne "" }) -join ';'
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
        $env:Path = ($env:Path -split ';' | Where-Object { $_ -ne $dir -and $_ -ne "" }) -join ';'
        Write-Success "Removed $dir from User PATH."
    }
}

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

Write-Host ""
Write-Success "git-janitor has been completely uninstalled."
Write-Host ""
