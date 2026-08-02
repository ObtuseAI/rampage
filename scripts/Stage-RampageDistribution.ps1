param(
    [Parameter(Mandatory)]
    [ValidateSet('windows-x64', 'linux-x64', 'macos-arm64')]
    [string]$Platform,
    [string]$OutputRoot = 'output/distribution',
    [switch]$RequirePlatformSignature
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$bundleRoot = Join-Path $root 'target/release/bundle'
$resolvedOutputRoot = [IO.Path]::GetFullPath((Join-Path $root $OutputRoot))
$allowedOutputRoot = [IO.Path]::GetFullPath((Join-Path $root 'output/distribution'))
$allowedPrefix = $allowedOutputRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) +
    [IO.Path]::DirectorySeparatorChar
if ($resolvedOutputRoot -ne $allowedOutputRoot -and
    -not $resolvedOutputRoot.StartsWith($allowedPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "distribution output must remain under $allowedOutputRoot"
}
if (-not (Test-Path -LiteralPath $bundleRoot -PathType Container)) {
    throw "Tauri bundle directory is missing: $bundleRoot"
}

$stageRoot = Join-Path $resolvedOutputRoot $Platform
New-Item -ItemType Directory -Path $stageRoot -Force | Out-Null

$bundleFiles = Get-ChildItem -LiteralPath $bundleRoot -File -Recurse
$selected = switch ($Platform) {
    'windows-x64' { $bundleFiles | Where-Object { $_.Extension -eq '.msi' -or $_.Name -like '*-setup.exe' } }
    'linux-x64' { $bundleFiles | Where-Object { $_.Extension -eq '.deb' -or $_.Name -like '*.AppImage' } }
    'macos-arm64' { $bundleFiles | Where-Object { $_.Extension -eq '.dmg' } }
}
if (-not $selected) {
    throw "no expected $Platform bundle artifacts were produced under $bundleRoot"
}

foreach ($asset in $selected) {
    Copy-Item -LiteralPath $asset.FullName -Destination (Join-Path $stageRoot $asset.Name) -Force
}

$macApp = $null
if ($Platform -eq 'macos-arm64') {
    $macApp = Get-ChildItem -LiteralPath (Join-Path $bundleRoot 'macos') -Directory -Filter '*.app' |
        Select-Object -First 1
    if (-not $macApp) { throw 'macOS app bundle is missing' }
    $appArchive = Join-Path $stageRoot "$($macApp.Name).zip"
    & ditto -c -k --sequesterRsrc --keepParent $macApp.FullName $appArchive
    if ($LASTEXITCODE -ne 0) { throw 'macOS app archive creation failed' }
}

$signatureVerified = $false
if ($RequirePlatformSignature) {
    switch ($Platform) {
        'windows-x64' {
            if (-not $IsWindows) { throw 'Windows signature verification requires Windows' }
            $subjects = @(
                Get-ChildItem -LiteralPath (Join-Path $root 'apps/desktop/src-tauri/binaries') -File -Filter '*.exe'
                Get-Item -LiteralPath (Join-Path $root 'target/release/rampage-desktop.exe')
                Get-ChildItem -LiteralPath $stageRoot -File | Where-Object { $_.Extension -in @('.exe', '.msi') }
            )
            foreach ($subject in $subjects) {
                $signature = Get-AuthenticodeSignature -LiteralPath $subject.FullName
                if ($signature.Status -ne 'Valid') {
                    throw "Authenticode verification failed for $($subject.FullName): $($signature.Status)"
                }
            }
            $signatureVerified = $true
        }
        'macos-arm64' {
            if (-not $IsMacOS) { throw 'macOS signature verification requires macOS' }
            & codesign --verify --deep --strict --verbose=2 $macApp.FullName
            if ($LASTEXITCODE -ne 0) { throw 'codesign verification failed' }
            & spctl --assess --type execute --verbose=2 $macApp.FullName
            if ($LASTEXITCODE -ne 0) { throw 'Gatekeeper assessment failed' }
            & xcrun stapler validate $macApp.FullName
            if ($LASTEXITCODE -ne 0) { throw 'notarization ticket validation failed' }
            $signatureVerified = $true
        }
        'linux-x64' {
            throw 'Linux native package signing is channel-specific; verify the GitHub attestation instead'
        }
    }
}

$assets = Get-ChildItem -LiteralPath $stageRoot -File | Sort-Object Name | ForEach-Object {
    $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    [pscustomobject]@{
        name = $_.Name
        bytes = $_.Length
        sha256 = $hash
    }
}
$checksumLines = $assets | ForEach-Object { "$($_.sha256)  $($_.name)" }
Set-Content -LiteralPath (Join-Path $stageRoot "SHA256SUMS-$Platform") -Value $checksumLines -Encoding utf8NoBOM

$sourceCommit = (& git -C $root rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'could not resolve source commit' }
$manifest = [ordered]@{
    schema = 'rampage.distribution-manifest.v1'
    platform = $Platform
    source_commit = $sourceCommit
    generated_at = (Get-Date).ToUniversalTime().ToString('o')
    platform_signature_required = [bool]$RequirePlatformSignature
    platform_signature_verified = $signatureVerified
    assets = @($assets)
}
$manifest | ConvertTo-Json -Depth 6 |
    Set-Content -LiteralPath (Join-Path $stageRoot "distribution-manifest-$Platform.json") -Encoding utf8NoBOM

$manifest | ConvertTo-Json -Depth 6
