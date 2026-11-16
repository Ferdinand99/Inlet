# Installs the Setup.exe silently, checks the installed app and removes it again.
#
#   pwsh packaging/windows/smoke-test.ps1 -Setup <setup.exe> -Version <version>
#
# Checks that the installer ends with success and installs per user (under
# %LocalAppData%, with an entry in HKCU, no admin rights needed), that the installed
# inlet.exe prints its version, that the Start menu shortcut exists, and that the
# uninstaller removes the files, the shortcut and the registry entry. Runs on a
# throwaway CI runner: it refuses to run when Inlet is already installed.
param(
    [Parameter(Mandatory = $true)][string]$Setup,
    [Parameter(Mandatory = $true)][string]$Version
)

$ErrorActionPreference = 'Stop'
$id = 'no.opland.inlet'
$root = Join-Path $env:LOCALAPPDATA $id
$current = Join-Path $root 'current'
$exe = Join-Path $current 'inlet.exe'
$update = Join-Path $root 'Update.exe'
$programs = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$uninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$id"
$temp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$logs = Join-Path $temp 'inlet-smoke'
New-Item -ItemType Directory -Force -Path $logs | Out-Null

function Show-Logs {
    Get-ChildItem -Path $logs -Filter *.log -ErrorAction SilentlyContinue | ForEach-Object {
        Write-Host "---- $($_.Name)"
        Get-Content $_.FullName
    }
}

function Fail([string]$message) {
    Show-Logs
    throw $message
}

if (Test-Path $root) { throw "$root exists already; refusing to touch it" }

# The installed shortcut: Velopack names it after the package title.
function Get-InletShortcuts {
    Get-ChildItem -Path $programs -Filter 'Inlet*.lnk' -Recurse -ErrorAction SilentlyContinue
}

Write-Host "Installing $Setup"
$install = Start-Process -FilePath $Setup -ArgumentList @('--silent', '--verbose', '--log', (Join-Path $logs 'setup.log')) `
    -Wait -PassThru
if ($install.ExitCode -ne 0) { Fail "Setup exited with code $($install.ExitCode)" }

if (-not (Test-Path $exe)) { Fail "$exe was not installed" }
if (-not (Test-Path $update)) { Fail "$update (the updater) was not installed" }
foreach ($name in @('LICENSE', 'THIRD-PARTY-NOTICES')) {
    if (-not (Test-Path (Join-Path $current $name))) { Fail "$name was not installed next to inlet.exe" }
}
if (-not (Test-Path $uninstallKey)) { Fail "no uninstall entry at $uninstallKey" }
if (-not (Get-InletShortcuts)) { Fail "no Inlet shortcut under $programs" }

# A silent install must not leave a running Inlet behind (the installer's hook
# calls must exit at once).
if (Get-Process -Name inlet -ErrorAction SilentlyContinue) { Fail 'Inlet is running after the silent install' }

# inlet.exe is a GUI program; with redirected output it still prints to the file.
$out = Join-Path $logs 'version.txt'
$run = Start-Process -FilePath $exe -ArgumentList '--version' -Wait -PassThru -NoNewWindow `
    -RedirectStandardOutput $out
if ($run.ExitCode -ne 0) { Fail "inlet.exe --version exited with code $($run.ExitCode)" }
$printed = (Get-Content -Raw $out).Trim()
if ($printed -ne "inlet $Version") { Fail "inlet.exe --version printed '$printed', expected 'inlet $Version'" }
Write-Host "Installed app prints: $printed"

Write-Host 'Uninstalling'
$remove = Start-Process -FilePath $update -ArgumentList @('--uninstall', '--silent', '--verbose', '--log', (Join-Path $logs 'uninstall.log')) `
    -Wait -PassThru
if ($remove.ExitCode -ne 0) { Fail "Update.exe --uninstall exited with code $($remove.ExitCode)" }

# The uninstaller removes the folder's contents; wait briefly for the last handles.
for ($i = 0; $i -lt 20 -and (Test-Path $exe); $i++) { Start-Sleep -Milliseconds 500 }
if (Test-Path $exe) { Fail "$exe is still there after the uninstall" }
if (Test-Path $uninstallKey) { Fail "$uninstallKey is still there after the uninstall" }
if (Get-InletShortcuts) { Fail 'the Inlet shortcut is still there after the uninstall' }

Show-Logs
Write-Host 'Windows package smoke test passed'
