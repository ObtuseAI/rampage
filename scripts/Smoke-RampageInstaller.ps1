param(
    [string]$Installer = 'target\release\bundle\nsis\Rampage_0.2.0_x64-setup.exe'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$resolvedInstaller = (Resolve-Path (Join-Path $root $Installer)).Path
$installRoot = Join-Path $root ('output\nsis-install-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $installRoot | Out-Null
$uninstaller = Join-Path $installRoot 'uninstall.exe'
$desktopDirectory = [Environment]::GetFolderPath([Environment+SpecialFolder]::DesktopDirectory)
$desktopShortcut = Join-Path $desktopDirectory 'Rampage.lnk'
$programsDirectory = [Environment]::GetFolderPath([Environment+SpecialFolder]::Programs)
$shellShortcut = Join-Path $programsDirectory 'Rampage Shell.lnk'
$shortcutBackup = Join-Path $root ('output\preexisting-rampage-shortcut-' + [guid]::NewGuid().ToString('N') + '.lnk')
$shellShortcutBackup = Join-Path $root ('output\preexisting-rampage-shell-' + [guid]::NewGuid().ToString('N') + '.lnk')
$hadShortcut = Test-Path -LiteralPath $desktopShortcut
$hadShellShortcut = Test-Path -LiteralPath $shellShortcut
if ($hadShortcut) { Copy-Item -LiteralPath $desktopShortcut -Destination $shortcutBackup }
if ($hadShellShortcut) { Copy-Item -LiteralPath $shellShortcut -Destination $shellShortcutBackup }
$uninstallRegistryPath = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Rampage'
$installPreferencePath = 'HKCU:\Software\obtuse\Rampage'
$uninstallRegistryBackup = if (Test-Path -LiteralPath $uninstallRegistryPath) {
    Get-ItemProperty -LiteralPath $uninstallRegistryPath
} else { $null }
$installPreferenceBackup = if (Test-Path -LiteralPath $installPreferencePath) {
    (Get-Item -LiteralPath $installPreferencePath).GetValue('')
} else { $null }
$installCompleted = $false
$uninstallExit = $null
$shortcutRemoved = $false
$shellShortcutRemoved = $false
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
    if (-not (Test-Path -LiteralPath $shellShortcut -PathType Leaf)) {
        throw "installer did not create the required Rampage Shell shortcut at $shellShortcut"
    }
    $shellLink = (New-Object -ComObject WScript.Shell).CreateShortcut($shellShortcut)
    if ([IO.Path]::GetFileName($shellLink.TargetPath) -ne 'cmd.exe' -or
        $shellLink.Arguments -notlike "*$installRoot*" -or
        $shellLink.Arguments -notlike '*set PATH=*') {
        throw "Rampage Shell shortcut does not expose the installed CLI: $($shellLink.TargetPath) $($shellLink.Arguments)"
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
        $shellShortcutRemoved = -not (Test-Path -LiteralPath $shellShortcut)
        if (-not $shortcutRemoved -and -not $cleanupError) {
            $cleanupError = "uninstall left the Rampage desktop launcher at $desktopShortcut"
        }
        if (-not $shellShortcutRemoved -and -not $cleanupError) {
            $cleanupError = "uninstall left the Rampage Shell launcher at $shellShortcut"
        }
    }
    if ($hadShortcut -and (Test-Path -LiteralPath $shortcutBackup)) {
        Copy-Item -LiteralPath $shortcutBackup -Destination $desktopShortcut -Force
        Remove-Item -LiteralPath $shortcutBackup -Force
    }
    if ($hadShellShortcut -and (Test-Path -LiteralPath $shellShortcutBackup)) {
        Copy-Item -LiteralPath $shellShortcutBackup -Destination $shellShortcut -Force
        Remove-Item -LiteralPath $shellShortcutBackup -Force
    }
    if ($uninstallRegistryBackup) {
        New-Item -Path $uninstallRegistryPath -Force | Out-Null
        foreach ($property in $uninstallRegistryBackup.PSObject.Properties) {
            if ($property.Name -notlike 'PS*') {
                Set-ItemProperty -LiteralPath $uninstallRegistryPath -Name $property.Name `
                    -Value $property.Value -Force
            }
        }
    } elseif (Test-Path -LiteralPath $uninstallRegistryPath) {
        Remove-Item -LiteralPath $uninstallRegistryPath -Recurse -Force
    }
    if ($null -ne $installPreferenceBackup) {
        New-Item -Path $installPreferencePath -Force | Out-Null
        Set-Item -LiteralPath $installPreferencePath -Value $installPreferenceBackup
    } elseif (Test-Path -LiteralPath $installPreferencePath) {
        Remove-Item -LiteralPath $installPreferencePath -Recurse -Force
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
    shell_shortcut = $shellShortcut
    shell_shortcut_removed_on_uninstall = $shellShortcutRemoved
    controller = $desktopSmoke.controller
    intelligence = $desktopSmoke.intelligence
    nodes = $desktopSmoke.nodes
    offers = $desktopSmoke.offers
    sidecar_leak = $false
    install_root = $installRoot
} | ConvertTo-Json
