param(
    [switch]$SkipOllama
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    & cargo fmt --all -- --check
    if ($LASTEXITCODE -ne 0) { throw 'Rust formatting failed' }
    & cargo test --workspace
    if ($LASTEXITCODE -ne 0) { throw 'Rust workspace tests failed' }
    & cargo clippy --workspace --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'Rust clippy failed' }
    & pnpm check
    if ($LASTEXITCODE -ne 0) { throw 'desktop or TypeScript SDK validation failed' }
    & uv run --project services\intelligence ruff check services\intelligence packages\sdk-python
    if ($LASTEXITCODE -ne 0) { throw 'Python linting failed' }
    & uv run --project services\intelligence mypy services\intelligence\src
    if ($LASTEXITCODE -ne 0) { throw 'Python strict typing failed' }
    & uv run --project services\intelligence pytest services\intelligence\tests
    if ($LASTEXITCODE -ne 0) { throw 'intelligence tests failed' }
    $oldPythonPath = $env:PYTHONPATH
    $env:PYTHONPATH = 'packages\sdk-python\src'
    & uv run --project packages\sdk-python --with pytest pytest packages\sdk-python\tests
    $env:PYTHONPATH = $oldPythonPath
    if ($LASTEXITCODE -ne 0) { throw 'Python SDK tests failed' }

    # E2E scripts execute standalone binaries, not Cargo test harnesses. Rebuild them explicitly so
    # process evidence can never be produced by stale target/debug executables.
    & cargo build -p rampage-controller -p rampage-agent -p rampage-cli -p rampage-relay
    if ($LASTEXITCODE -ne 0) { throw 'debug sidecar build failed' }
    & .\scripts\e2e.ps1
    & .\scripts\mesh-e2e.ps1
    & .\scripts\model-gateway-e2e.ps1
    if (-not $SkipOllama) { & .\scripts\ollama-e2e.ps1 }
} finally {
    Pop-Location
}
