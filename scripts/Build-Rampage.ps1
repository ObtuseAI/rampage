param(
    [ValidateSet('debug', 'release')]
    [string]$Profile = 'release',
    [switch]$NoBundle,
    [string]$TauriConfig
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$triple = (& rustc --print host-tuple).Trim()
$cargoArgs = @(
    'build',
    '-p', 'rampage-controller',
    '-p', 'rampage-agent',
    '-p', 'rampage-cli',
    '-p', 'rampage-relay'
)
if ($Profile -eq 'release') { $cargoArgs += '--release' }
& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { throw 'Rust sidecar build failed' }

$source = Join-Path $root "target\$Profile"
$destination = Join-Path $root 'apps\desktop\src-tauri\binaries'
New-Item -ItemType Directory -Force -Path $destination | Out-Null
foreach ($name in @('rampage-controller', 'rampage-agent', 'rampage', 'rampage-relay')) {
    $extension = if ($IsWindows) { '.exe' } else { '' }
    Copy-Item -Force (Join-Path $source "$name$extension") `
        (Join-Path $destination "$name-$triple$extension")
}

$intelligenceProject = Join-Path $root 'services\intelligence'
& uv run --project $intelligenceProject --with pyinstaller pyinstaller `
    --noconfirm --clean --onefile `
    --name rampage-intelligence `
    --paths (Join-Path $intelligenceProject 'src') `
    --copy-metadata genai-prices `
    --copy-metadata pydantic-ai `
    --copy-metadata pydantic-ai-slim `
    --copy-metadata dbos `
    (Join-Path $intelligenceProject 'rampage_intelligence_entry.py')
if ($LASTEXITCODE -ne 0) { throw 'Intelligence sidecar build failed' }
$intelligenceExtension = if ($IsWindows) { '.exe' } else { '' }
Copy-Item -Force (Join-Path $root "dist\rampage-intelligence$intelligenceExtension") `
    (Join-Path $destination "rampage-intelligence-$triple$intelligenceExtension")

if (-not $NoBundle) {
    $tauriArgs = @('--dir', (Join-Path $root 'apps\desktop'), 'tauri', 'build')
    if ($TauriConfig) {
        $tauriArgs += @('--config', $TauriConfig)
    }
    & pnpm @tauriArgs
    if ($LASTEXITCODE -ne 0) { throw 'Tauri bundle failed' }
}
