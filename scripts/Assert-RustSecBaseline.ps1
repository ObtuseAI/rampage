param(
    [string]$BaselinePath = (Join-Path (Split-Path -Parent $PSScriptRoot) 'security/rustsec-baseline.json')
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$baseline = Get-Content -LiteralPath $BaselinePath -Raw | ConvertFrom-Json

if ($baseline.schema -ne 'rampage.rustsec-baseline.v1') {
    throw "unsupported RustSec baseline schema: $($baseline.schema)"
}
if ([DateTime]::Parse($baseline.reviewBy).ToUniversalTime() -lt [DateTime]::UtcNow.Date) {
    throw "RustSec baseline review expired on $($baseline.reviewBy)"
}

$targetTriples = [ordered]@{
    windows = 'x86_64-pc-windows-msvc'
    linux = 'x86_64-unknown-linux-gnu'
    macos = 'aarch64-apple-darwin'
}
$targetTrees = @{}
$auditErrorPath = [IO.Path]::GetTempFileName()
Push-Location $root
try {
    foreach ($entry in $targetTriples.GetEnumerator()) {
        $tree = (& cargo tree --locked --target $entry.Value --prefix none 2>&1 | Out-String)
        if ($LASTEXITCODE -ne 0) { throw "cargo tree failed for $($entry.Value): $tree" }
        $targetTrees[$entry.Key] = $tree
    }

    $auditJson = (& cargo audit --json 2>$auditErrorPath | Out-String)
    $auditError = (Get-Content -LiteralPath $auditErrorPath -Raw -ErrorAction SilentlyContinue)
    $auditExit = $LASTEXITCODE
} finally {
    Pop-Location
    Remove-Item -LiteralPath $auditErrorPath -Force -ErrorAction SilentlyContinue
}

try {
    $report = $auditJson | ConvertFrom-Json
} catch {
    throw "cargo audit did not return valid JSON: $auditJson $auditError"
}
if ($report.vulnerabilities.count -ne 0 -or $report.vulnerabilities.found) {
    throw "RustSec found $($report.vulnerabilities.count) published vulnerabilities"
}
if ($auditExit -ne 0) {
    throw "cargo audit exited with code $auditExit despite reporting no parsed vulnerability: $auditError"
}

$actual = @{}
foreach ($warningKind in $report.warnings.PSObject.Properties) {
    foreach ($warning in $warningKind.Value) {
        $id = $warning.advisory.id
        if ($actual.ContainsKey($id)) { throw "duplicate RustSec warning $id" }
        $actual[$id] = [pscustomobject]@{
            kind = $warning.kind
            package = $warning.package.name
        }
    }
}

$expected = @{}
foreach ($warning in $baseline.warnings) {
    if ($expected.ContainsKey($warning.id)) { throw "duplicate baseline warning $($warning.id)" }
    if ([string]::IsNullOrWhiteSpace($warning.path) -or [string]::IsNullOrWhiteSpace($warning.disposition)) {
        throw "baseline warning $($warning.id) lacks a path or disposition"
    }
    $expected[$warning.id] = $warning
}

$unexpected = @($actual.Keys | Where-Object { -not $expected.ContainsKey($_) } | Sort-Object)
$missing = @($expected.Keys | Where-Object { -not $actual.ContainsKey($_) } | Sort-Object)
if ($unexpected.Count -gt 0 -or $missing.Count -gt 0) {
    throw "RustSec baseline drift; unexpected=[$($unexpected -join ', ')]; missing=[$($missing -join ', ')]"
}

foreach ($id in $expected.Keys) {
    $wanted = $expected[$id]
    $found = $actual[$id]
    if ($wanted.kind -ne $found.kind -or $wanted.package -ne $found.package) {
        throw "$id changed identity; expected $($wanted.kind)/$($wanted.package), found $($found.kind)/$($found.package)"
    }

    $observedTargets = @()
    $packagePattern = "(?m)^$([regex]::Escape($wanted.package)) v"
    foreach ($target in $targetTriples.Keys) {
        if ($targetTrees[$target] -match $packagePattern) { $observedTargets += $target }
    }
    $declaredTargets = @($wanted.targets | Sort-Object)
    $observedTargets = @($observedTargets | Sort-Object)
    if (($declaredTargets -join ',') -ne ($observedTargets -join ',')) {
        throw "$id target scope drift; expected [$($declaredTargets -join ', ')], found [$($observedTargets -join ', ')]"
    }
}

[pscustomobject]@{
    result = 'PASS'
    vulnerabilities = 0
    reviewedWarnings = $expected.Count
    targets = @($targetTriples.Keys)
    reviewBy = $baseline.reviewBy
} | ConvertTo-Json -Depth 3
