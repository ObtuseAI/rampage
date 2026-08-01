param(
    [string]$Installer = 'target\release\bundle\nsis\Rampage_0.1.0_x64-setup.exe'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$resolvedInstaller = (Resolve-Path (Join-Path $root $Installer)).Path
$installRoot = Join-Path $root ('output\nsis-install-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $installRoot | Out-Null
$uninstaller = Join-Path $installRoot 'uninstall.exe'
$desktopDirectory = [Environment]::GetFolderPath([Environment+SpecialFolder]::DesktopDirectory)
$desktopShortcut = Join-Path $desktopDirectory 'Rampage.lnk'
$shortcutBackup = Join-Path $root ('output\preexisting-rampage-shortcut-' + [guid]::NewGuid().ToString('N') + '.lnk')
$hadShortcut = Test-Path -LiteralPath $desktopShortcut
if ($hadShortcut) { Copy-Item -LiteralPath $desktopShortcut -Destination $shortcutBackup }
$installCompleted = $false
$uninstallExit = $null
$shortcutRemoved = $false
$desktopSmoke = $null

try {
    $install = Start-Process -FilePath $resolvedInstaller `
        -ArgumentList @('/S', ("/D=$installRoot")) -Wait -PassThru
    if ($install.ExitCode -ne 0) { throw "NSIS install failed with exit code $($install.ExitCode)" }
    $installCompleted = $true
    $required = @(
        'rampage-desktop.exe',
        'rampage-controller.exe',
        'rampage-agent.exe',
        'rampage.exe',
        'rampage-intelligence.exe'
    )
    foreach ($name in $required) {
        if (-not (Test-Path -LiteralPath (Join-Path $installRoot $name))) {
            throw "installed package is missing $name"
        }
    }
    if (-not (Test-Path -LiteralPath $desktopShortcut -PathType Leaf)) {
        throw "installer did not create the required desktop launcher at $desktopShortcut"
    }
    $shortcutTarget = (New-Object -ComObject WScript.Shell).CreateShortcut($desktopShortcut).TargetPath
    $expectedTarget = Join-Path $installRoot 'rampage-desktop.exe'
    if ([IO.Path]::GetFullPath($shortcutTarget) -ne [IO.Path]::GetFullPath($expectedTarget)) {
        throw "desktop launcher targets '$shortcutTarget', expected '$expectedTarget'"
    }
    $desktopSmokeJson = & (Join-Path $PSScriptRoot 'Smoke-RampageDesktop.ps1') `
        -Executable (Join-Path $installRoot 'rampage-desktop.exe') | Out-String
    $desktopSmoke = $desktopSmokeJson | ConvertFrom-Json
    if ($desktopSmoke.result -ne 'PASS') { throw 'installed desktop smoke failed' }

} finally {
    $cleanupError = $null
    if (Test-Path -LiteralPath $uninstaller) {
        $uninstall = Start-Process -FilePath $uninstaller -ArgumentList '/S' -Wait -PassThru
        $uninstallExit = $uninstall.ExitCode
        if ($uninstall.ExitCode -ne 0) {
            $cleanupError = "NSIS uninstall returned exit code $($uninstall.ExitCode)"
        }
    } elseif ($installCompleted) {
        $cleanupError = 'installed package omitted its uninstaller'
    }
    if ($installCompleted) {
        $shortcutRemoved = -not (Test-Path -LiteralPath $desktopShortcut)
        if (-not $shortcutRemoved -and -not $cleanupError) {
            $cleanupError = "uninstall left the Rampage desktop launcher at $desktopShortcut"
        }
    }
    if ($hadShortcut -and (Test-Path -LiteralPath $shortcutBackup)) {
        Copy-Item -LiteralPath $shortcutBackup -Destination $desktopShortcut -Force
        Remove-Item -LiteralPath $shortcutBackup -Force
    }
    if ($cleanupError) { throw $cleanupError }
}

[pscustomobject]@{
    result = 'PASS'
    installer = $resolvedInstaller
    install_exit = $install.ExitCode
    uninstall_exit = $uninstallExit
    payloads = $required.Count
    desktop_shortcut = $desktopShortcut
    shortcut_target = $shortcutTarget
    shortcut_removed_on_uninstall = $shortcutRemoved
    controller = $desktopSmoke.controller
    intelligence = $desktopSmoke.intelligence
    nodes = $desktopSmoke.nodes
    offers = $desktopSmoke.offers
    sidecar_leak = $false
    install_root = $installRoot
} | ConvertTo-Json
