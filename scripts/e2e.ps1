param(
    [int]$Port = 47911
)

$ErrorActionPreference = 'Stop'
$e2eRoot = Join-Path (Resolve-Path .) ('output\e2e-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $e2eRoot | Out-Null
$controllerExe = (Resolve-Path 'target\debug\rampage-controller.exe').Path
$cliExe = (Resolve-Path 'target\debug\rampage.exe').Path
$agentExe = (Resolve-Path 'target\debug\rampage-agent.exe').Path
$controllerUrl = "http://127.0.0.1:$Port"
$oldBind = $env:RAMPAGE_BIND
$oldData = $env:RAMPAGE_DATA_DIR
$oldToken = $env:RAMPAGE_TOKEN
$env:RAMPAGE_BIND = "127.0.0.1:$Port"
$env:RAMPAGE_DATA_DIR = $e2eRoot
$controllerProcess = Start-Process -FilePath $controllerExe -PassThru -WindowStyle Hidden
$agentProcess = $null
$env:RAMPAGE_BIND = $oldBind
$env:RAMPAGE_DATA_DIR = $oldData

try {
    $ready = $false
    for ($attempt = 0; $attempt -lt 60; $attempt++) {
        try {
            Invoke-RestMethod "$controllerUrl/health" | Out-Null
            $ready = $true
            break
        } catch {
            Start-Sleep -Milliseconds 100
        }
    }
    if (-not $ready) { throw 'controller did not become ready' }
    $unauthorized = Invoke-WebRequest "$controllerUrl/v1/nodes" -SkipHttpErrorCheck
    if ($unauthorized.StatusCode -ne 401) { throw 'protected local API accepted a tokenless request' }
    $env:RAMPAGE_TOKEN = (Get-Content -Raw (Join-Path $e2eRoot 'controller.token')).Trim()
    $headers = @{ 'x-rampage-token' = $env:RAMPAGE_TOKEN }

    $invite = (& $cliExe --controller $controllerUrl invite | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'invite failed' }
    & $agentExe --controller $controllerUrl --key-file (Join-Path $e2eRoot 'agent.key') `
        --enrollment-code $invite.enrollment_code --display-name 'E2E Node' `
        --device-kind desktop --register | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'agent enrollment failed' }

    $initialHealth = Invoke-RestMethod "$controllerUrl/health"
    Stop-Process -Id $controllerProcess.Id -Force
    $controllerProcess.WaitForExit()
    $env:RAMPAGE_BIND = "127.0.0.1:$Port"
    $env:RAMPAGE_DATA_DIR = $e2eRoot
    $controllerProcess = Start-Process -FilePath $controllerExe -PassThru -WindowStyle Hidden
    $env:RAMPAGE_BIND = $oldBind
    $env:RAMPAGE_DATA_DIR = $oldData
    $ready = $false
    for ($attempt = 0; $attempt -lt 60; $attempt++) {
        try {
            $restartedHealth = Invoke-RestMethod "$controllerUrl/health"
            $ready = $true
            break
        } catch {
            Start-Sleep -Milliseconds 100
        }
    }
    if (-not $ready) { throw 'restarted controller did not become ready' }
    if ($initialHealth.mesh_endpoint_id -ne $restartedHealth.mesh_endpoint_id) {
        throw 'mesh identity did not survive restart'
    }
    $restoredNodes = Invoke-RestMethod "$controllerUrl/v1/nodes" -Headers $headers
    if ($restoredNodes.Count -ne 1) { throw 'enrolled node did not survive restart' }

    $plan = (& $cliExe --controller $controllerUrl plan --value 'mesh proof' | Out-String) |
        ConvertFrom-Json
    if (-not $plan.selected_node) { throw 'placement plan selected no node' }
    $lease = (& $cliExe --controller $controllerUrl run --value 'mesh proof' | Out-String) |
        ConvertFrom-Json
    & $agentExe --controller $controllerUrl --key-file (Join-Path $e2eRoot 'agent.key') `
        --work-once | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'worker execution failed' }

    $events = Invoke-RestMethod "$controllerUrl/v1/events?after=0&limit=1000" -Headers $headers
    if (-not ($events.event_type -contains 'job.receipted')) { throw 'signed receipt missing' }

    $agentProcess = Start-Process -FilePath $agentExe -PassThru -WindowStyle Hidden -ArgumentList @(
        '--controller', $controllerUrl,
        '--key-file', (Join-Path $e2eRoot 'agent.key'),
        '--serve'
    )
    $shardPlan = (& $cliExe --controller $controllerUrl shard-plan '1,2,3' '4,5,6' '7,8,9' | Out-String) |
        ConvertFrom-Json
    if (-not $shardPlan.admissible -or $shardPlan.placements.Count -ne 3 -or $shardPlan.mutated) {
        throw "shard plan was not a non-mutating complete placement: $($shardPlan | ConvertTo-Json -Compress)"
    }
    $shardStatus = (& $cliExe --controller $controllerUrl shard-run '1,2,3' '4,5,6' '7,8,9' `
        --minimum-successes 3 --timeout-seconds 30 | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $shardStatus.status -ne 'succeeded' -or
        $shardStatus.succeeded -ne 3 -or -not $shardStatus.threshold_met) {
        throw "pooled shard execution failed: $($shardStatus | ConvertTo-Json -Compress)"
    }
    $shardResults = @($shardStatus.members | ForEach-Object { $_.result })
    if ($shardResults -notcontains '2.000000000000' -or
        $shardResults -notcontains '5.000000000000' -or
        $shardResults -notcontains '8.000000000000') {
        throw "pooled shard results were incomplete: $($shardResults -join ',')"
    }
    $events = Invoke-RestMethod "$controllerUrl/v1/events?after=0&limit=1000" -Headers $headers
    if (-not ($events.event_type -contains 'shard_set.admitted')) {
        throw 'authoritative shard-set admission event missing'
    }
    Stop-Process -Id $agentProcess.Id -Force
    $agentProcess.WaitForExit()
    $agentProcess = $null
    Stop-Process -Id $controllerProcess.Id -Force
    $controllerProcess.WaitForExit()
    $env:RAMPAGE_BIND = "127.0.0.1:$Port"
    $env:RAMPAGE_DATA_DIR = $e2eRoot
    $controllerProcess = Start-Process -FilePath $controllerExe -PassThru -WindowStyle Hidden
    $env:RAMPAGE_BIND = $oldBind
    $env:RAMPAGE_DATA_DIR = $oldData
    $ready = $false
    for ($attempt = 0; $attempt -lt 60; $attempt++) {
        try {
            Invoke-RestMethod "$controllerUrl/health" | Out-Null
            $ready = $true
            break
        } catch {
            Start-Sleep -Milliseconds 100
        }
    }
    if (-not $ready) { throw 'controller did not recover after pooled shard execution' }
    $recoveredShardStatus = (& $cliExe --controller $controllerUrl shard-status $shardStatus.set_id |
        Out-String) | ConvertFrom-Json
    if ($recoveredShardStatus.status -ne 'succeeded' -or $recoveredShardStatus.succeeded -ne 3) {
        throw "durable shard status did not survive restart: $($recoveredShardStatus | ConvertTo-Json -Compress)"
    }
    & $cliExe --controller $controllerUrl stop | Out-Null
    $stopped = Invoke-RestMethod "$controllerUrl/health"
    if (-not $stopped.kill_latch) { throw 'kill latch did not set' }
    & $cliExe --controller $controllerUrl resume --confirm-owner-resume | Out-Null
    $resumed = Invoke-RestMethod "$controllerUrl/health"
    if ($resumed.kill_latch) { throw 'explicit resume failed' }

    [pscustomobject]@{
        result = 'PASS'
        node = $plan.selected_node
        lease = $lease.lease_id
        evidence_events = $events.Count
        receipted = $events.event_type -contains 'job.receipted'
        shard_set = $shardStatus.set_id
        shard_receipts = $shardStatus.succeeded
        shard_threshold_met = $shardStatus.threshold_met
        shard_restart_recovery = $true
        stop_resume = $true
        restart_recovery = $true
        tokenless_request_denied = $true
        artifacts = $e2eRoot
    } | ConvertTo-Json
} finally {
    $env:RAMPAGE_TOKEN = $oldToken
    if ($controllerProcess -and -not $controllerProcess.HasExited) {
        Stop-Process -Id $controllerProcess.Id -Force
    }
    if ($agentProcess -and -not $agentProcess.HasExited) {
        Stop-Process -Id $agentProcess.Id -Force
    }
}
