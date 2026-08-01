param(
    [ValidateSet('debug', 'release')]
    [string]$Profile = 'release',
    [switch]$NoBundle
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$triple = (& rustc --print host-tuple).Trim()
$cargoArgs = @('build', '-p', 'rampage-controller', '-p', 'rampage-agent', '-p', 'rampage-cli')
if ($Profile -eq 'release') { $cargoArgs += '--release' }
& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { throw 'Rust sidecar build failed' }

$source = Join-Path $root "target\$Profile"
$destination = Join-Path $root 'apps\desktop\src-tauri\binaries'
New-Item -ItemType Directory -Force -Path $destination | Out-Null
foreach ($name in @('rampage-controller', 'rampage-agent', 'rampage')) {
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
Copy-Item -Force (Join-Path $root 'dist\rampage-intelligence.exe') `
    (Join-Path $destination "rampage-intelligence-$triple.exe")

if (-not $NoBundle) {
    & pnpm --dir (Join-Path $root 'apps\desktop') tauri build
    if ($LASTEXITCODE -ne 0) { throw 'Tauri bundle failed' }
}
