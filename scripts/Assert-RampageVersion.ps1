param(
    [Parameter(Mandatory)]
    [string]$Tag
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if ($Tag -notmatch '^v(?<version>\d+\.\d+\.\d+)(?:[-+][0-9A-Za-z.-]+)?$') {
    throw "release tag is not a supported semantic version: $Tag"
}
$expected = $Matches.version

function Read-TomlVersion([string]$Path, [string]$SectionPattern) {
    $content = Get-Content -LiteralPath $Path -Raw
    $pattern = "(?ms)^\[$SectionPattern\].*?^version\s*=\s*`"(?<version>[^`"]+)`""
    if ($content -notmatch $pattern) { throw "version is missing from $Path [$SectionPattern]" }
    $Matches.version
}

$versions = [ordered]@{
    cargo_workspace = Read-TomlVersion (Join-Path $root 'Cargo.toml') 'workspace\.package'
    tauri = (Get-Content -LiteralPath (Join-Path $root 'apps/desktop/src-tauri/tauri.conf.json') -Raw |
        ConvertFrom-Json).version
    desktop = (Get-Content -LiteralPath (Join-Path $root 'apps/desktop/package.json') -Raw |
        ConvertFrom-Json).version
    typescript_sdk = (Get-Content -LiteralPath (Join-Path $root 'packages/sdk-ts/package.json') -Raw |
        ConvertFrom-Json).version
    intelligence = Read-TomlVersion (Join-Path $root 'services/intelligence/pyproject.toml') 'project'
    python_sdk = Read-TomlVersion (Join-Path $root 'packages/sdk-python/pyproject.toml') 'project'
}

foreach ($entry in $versions.GetEnumerator()) {
    if ($entry.Value -ne $expected) {
        throw "$($entry.Key) version $($entry.Value) does not match release tag $Tag"
    }
}

[pscustomobject]@{
    result = 'PASS'
    tag = $Tag
    version = $expected
    surfaces = $versions.Count
} | ConvertTo-Json
