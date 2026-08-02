param(
    [string]$Executable = 'dist\rampage-intelligence.exe',
    [int]$Port = 47932
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$resolvedExecutable = (Resolve-Path (Join-Path $root $Executable)).Path
$smokeRoot = Join-Path $root ('output\intelligence-smoke-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $smokeRoot | Out-Null
$oldPort = $env:RAMPAGE_INTELLIGENCE_PORT
$oldData = $env:RAMPAGE_DATA_DIR
$oldModels = $env:RAMPAGE_ENABLE_MODELS
$oldToken = $env:RAMPAGE_TOKEN
$smokeToken = 'rampage-intelligence-smoke-only'
$env:RAMPAGE_INTELLIGENCE_PORT = $Port.ToString()
$env:RAMPAGE_DATA_DIR = $smokeRoot
$env:RAMPAGE_ENABLE_MODELS = 'false'
$env:RAMPAGE_TOKEN = $smokeToken
$service = Start-Process -FilePath $resolvedExecutable -PassThru -WindowStyle Hidden
$env:RAMPAGE_INTELLIGENCE_PORT = $oldPort
$env:RAMPAGE_DATA_DIR = $oldData
$env:RAMPAGE_ENABLE_MODELS = $oldModels
$env:RAMPAGE_TOKEN = $oldToken

function Stop-ProcessTree([int]$RootProcessId) {
    $children = @(Get-CimInstance Win32_Process | Where-Object ParentProcessId -eq $RootProcessId)
    foreach ($child in $children) { Stop-ProcessTree -RootProcessId $child.ProcessId }
    Stop-Process -Id $RootProcessId -Force -ErrorAction SilentlyContinue
}

try {
    $health = $null
    for ($attempt = 0; $attempt -lt 300; $attempt++) {
        try {
            $health = Invoke-RestMethod "http://127.0.0.1:$Port/health"
            break
        } catch {
            Start-Sleep -Milliseconds 100
        }
    }
    if (-not $health) { throw 'packaged intelligence did not start' }
    if ($health.authority -ne 'proposal_only') { throw 'intelligence gained execution authority' }
    $body = @{
        project_id = [guid]::NewGuid().ToString()
        principal_id = [guid]::NewGuid().ToString()
        objective = 'Explain available local capacity'
    } | ConvertTo-Json
    $goal = Invoke-RestMethod "http://127.0.0.1:$Port/v1/goals" `
        -Method Post -Headers @{ 'x-rampage-token' = $smokeToken } `
        -ContentType 'application/json' -Body $body
    if ($goal.capability_state -ne 'deterministic_only') {
        throw 'model-disabled bundle did not fail into deterministic mode'
    }
    [pscustomobject]@{
        result = 'PASS'
        health = $health.status
        authority = $health.authority
        capability = $health.capability
        workflow = $goal.capability_state
        bytes = (Get-Item $resolvedExecutable).Length
        data_dir = $smokeRoot
    } | ConvertTo-Json
} finally {
    if ($service) { Stop-ProcessTree -RootProcessId $service.Id }
}
