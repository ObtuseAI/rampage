param()

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$pagePath = Join-Path $repoRoot 'docs\index.html'
$readmePath = Join-Path $repoRoot 'README.md'

if (-not (Test-Path -LiteralPath $pagePath -PathType Leaf)) {
    throw 'GitHub Pages entry point is missing'
}

$page = Get-Content -LiteralPath $pagePath -Raw
$readme = Get-Content -LiteralPath $readmePath -Raw

foreach ($required in @(
    'Your machines.',
    'assets/rampage-arena-live.png',
    'assets/og-rampage.png',
    'architecture-graph',
    'AI DIRECT AUTHORITY',
    'https://github.com/ObtuseAI/rampage/releases'
)) {
    if (-not $page.Contains($required)) {
        throw "showcase is missing required content: $required"
    }
}

foreach ($required in @('```mermaid', 'docs/assets/rampage-arena-live.png', 'Built—not imagined')) {
    if (-not $readme.Contains($required)) {
        throw "README showcase is missing required content: $required"
    }
}

$localReferences = [regex]::Matches($page, '(?:href|src)="([^"#:?]+)"') |
    ForEach-Object { $_.Groups[1].Value } |
    Where-Object { $_ -and -not $_.StartsWith('/') } |
    Sort-Object -Unique

$missing = @()
foreach ($reference in $localReferences) {
    $candidate = Join-Path (Split-Path -Parent $pagePath) $reference
    if (-not (Test-Path -LiteralPath $candidate)) {
        $missing += $reference
    }
}
if ($missing.Count -gt 0) {
    throw "showcase has missing local references: $($missing -join ', ')"
}

Add-Type -AssemblyName System.Drawing
$ogPath = Join-Path $repoRoot 'docs\assets\og-rampage.png'
$og = [System.Drawing.Image]::FromFile($ogPath)
try {
    if ($og.Width -ne 1200 -or $og.Height -ne 630) {
        throw "social preview must be 1200x630, found $($og.Width)x$($og.Height)"
    }
} finally {
    $og.Dispose()
}

[pscustomobject]@{
    result = 'PASS'
    page = $pagePath
    local_references = $localReferences.Count
    social_preview = '1200x630'
    screenshot = 'present'
    architecture_graph = 'present'
} | ConvertTo-Json

