param(
    [string]$DesktopExecutable = 'target\release\rampage-desktop.exe',
    [int]$ControllerPort = 47941
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$desktopPath = if ([System.IO.Path]::IsPathRooted($DesktopExecutable)) {
    $DesktopExecutable
} else {
    Join-Path $root $DesktopExecutable
}
$desktopExe = (Resolve-Path $desktopPath).Path
$controllerExe = (Resolve-Path (Join-Path $root 'target\release\rampage-controller.exe')).Path
$cliExe = (Resolve-Path (Join-Path $root 'target\release\rampage.exe')).Path
$smokeRoot = Join-Path $root ('output\worker-desktop-smoke-' + [guid]::NewGuid().ToString('N'))
$ownerData = Join-Path $smokeRoot 'owner'
$workerData = Join-Path $smokeRoot 'worker'
New-Item -ItemType Directory -Path $ownerData, $workerData | Out-Null
$controllerUrl = "http://127.0.0.1:$ControllerPort"
$oldBind = $env:RAMPAGE_BIND
$oldData = $env:RAMPAGE_DATA_DIR
$oldToken = $env:RAMPAGE_TOKEN
$oldDiagnosticExit = $env:RAMPAGE_DIAGNOSTIC_EXIT_AFTER_MS
$controller = $null
$desktop = $null
$restartDesktop = $null

function Stop-ProcessTree([int]$RootProcessId) {
    $children = @(Get-CimInstance Win32_Process | Where-Object ParentProcessId -eq $RootProcessId)
    foreach ($child in $children) { Stop-ProcessTree -RootProcessId $child.ProcessId }
    Stop-Process -Id $RootProcessId -Force -ErrorAction SilentlyContinue
}

try {
    $env:RAMPAGE_BIND = "127.0.0.1:$ControllerPort"
    $env:RAMPAGE_DATA_DIR = $ownerData
    $controller = Start-Process -FilePath $controllerExe -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $ownerData 'controller.stdout.log') `
        -RedirectStandardError (Join-Path $ownerData 'controller.stderr.log')
    $ready = $false
    for ($attempt = 0; $attempt -lt 100; $attempt++) {
        if ($controller.HasExited) {
            $controllerError = Get-Content -Raw (Join-Path $ownerData 'controller.stderr.log') `
                -ErrorAction SilentlyContinue
            throw "release controller exited before readiness (exit=$($controller.ExitCode)): $controllerError"
        }
        try {
            Invoke-RestMethod "$controllerUrl/health" | Out-Null
            $ready = $true
            break
        } catch { Start-Sleep -Milliseconds 100 }
    }
    if (-not $ready) { throw 'release controller did not become ready' }
    $env:RAMPAGE_TOKEN = (Get-Content -Raw (Join-Path $ownerData 'controller.token')).Trim()
    $invite = (& $cliExe --controller $controllerUrl invite | Out-String)
    if ($LASTEXITCODE -ne 0) { throw 'release CLI could not create a complete invitation' }
    $parsedInvite = $invite | ConvertFrom-Json
    if (-not $parsedInvite.controller_mesh.signature) { throw 'invitation has no signed mesh endpoint' }
    Set-Content -LiteralPath (Join-Path $workerData 'remote-invite.json') -Value $invite -NoNewline

    $env:RAMPAGE_DATA_DIR = $workerData
    $env:RAMPAGE_BIND = $oldBind
    $env:RAMPAGE_TOKEN = $oldToken
    $env:RAMPAGE_DIAGNOSTIC_EXIT_AFTER_MS = '150000'
    $desktop = Start-Process -FilePath $desktopExe -PassThru
    $env:RAMPAGE_DIAGNOSTIC_EXIT_AFTER_MS = $oldDiagnosticExit

    $joined = $false
    $node = $null
    $nodeOffer = $null
    $headers = @{ 'x-rampage-token' = (Get-Content -Raw (Join-Path $ownerData 'controller.token')).Trim() }
    for ($attempt = 0; $attempt -lt 600; $attempt++) {
        if ($desktop.HasExited) {
            throw "packaged worker desktop exited before enrollment (exit=$($desktop.ExitCode))"
        }
        try {
            $nodes = @(Invoke-RestMethod "$controllerUrl/v1/nodes" -Headers $headers)
            $offers = @(Invoke-RestMethod "$controllerUrl/v1/offers" -Headers $headers)
            $node = $nodes | Where-Object { $_ -and $_.node_id } | Select-Object -First 1
            $nodeOffer = $offers | Where-Object {
                $_ -and "$($_.node_id)" -eq "$($node.node_id)"
            } | Select-Object -First 1
            if ($node -and $nodeOffer -and $nodeOffer.mesh_endpoint.signature) {
                $joined = $true
                break
            }
        } catch { }
        Start-Sleep -Milliseconds 100
    }
    if (-not $joined) {
        $phase = Get-Content -Raw (Join-Path $workerData 'agent.phase') -ErrorAction SilentlyContinue
        throw "packaged worker desktop did not publish a signed artifact endpoint over direct QUIC (phase=$phase, node=$($node | ConvertTo-Json -Compress), offer=$($nodeOffer | ConvertTo-Json -Compress))"
    }

    # Cross the one-minute topology-refresh boundary that previously stranded physical workers.
    # A passing release must continue rotating signed offers before and after that refresh.
    $offerIds = [System.Collections.Generic.HashSet[string]]::new()
    [void]$offerIds.Add("$($nodeOffer.offer_id)")
    $heartbeatDeadline = [DateTime]::UtcNow.AddSeconds(70)
    while ([DateTime]::UtcNow -lt $heartbeatDeadline) {
        if ($desktop.HasExited) {
            throw "packaged worker desktop exited while proving sustained heartbeat (exit=$($desktop.ExitCode))"
        }
        try {
            $offers = @(Invoke-RestMethod "$controllerUrl/v1/offers" -Headers $headers)
            $nodeOffer = $offers | Where-Object {
                $_ -and "$($_.node_id)" -eq "$($node.node_id)"
            } | Select-Object -First 1
            if ($nodeOffer -and $nodeOffer.mesh_endpoint.signature) {
                [void]$offerIds.Add("$($nodeOffer.offer_id)")
            }
        } catch { }
        Start-Sleep -Seconds 1
    }
    if ($offerIds.Count -lt 10) {
        $phase = Get-Content -Raw (Join-Path $workerData 'agent.phase') -ErrorAction SilentlyContinue
        throw "packaged worker crossed the topology-refresh boundary with only $($offerIds.Count) signed offers (phase=$phase)"
    }

    $artifactSource = Join-Path $smokeRoot 'artifact-source.bin'
    $artifactRetrieved = Join-Path $smokeRoot 'artifact-retrieved.bin'
    $artifactBytes = [byte[]](0, 1, 2, 127, 128, 254, 255)
    [System.IO.File]::WriteAllBytes($artifactSource, $artifactBytes)
    $env:RAMPAGE_TOKEN = $headers['x-rampage-token']
    $put = (& $cliExe --controller $controllerUrl artifact-put $artifactSource --storage-class cache | Out-String) |
        ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or -not $put.digest) { throw 'packaged CLI could not put artifact' }
    $nodeId = $node.node_id
    $replica = (& $cliExe --controller $controllerUrl artifact-replicate $put.digest $nodeId `
        --storage-class cache --media-type application/octet-stream | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $replica.artifact.digest -ne $put.digest) {
        throw 'packaged worker could not accept encrypted artifact replica over direct QUIC'
    }
    $retrieved = (& $cliExe --controller $controllerUrl artifact-retrieve $put.digest $nodeId `
        $artifactRetrieved | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $retrieved.digest -ne $put.digest) {
        throw 'packaged worker could not return artifact replica over direct QUIC'
    }
    $roundTrip = [System.IO.File]::ReadAllBytes($artifactRetrieved)
    if (-not [System.Linq.Enumerable]::SequenceEqual[byte]($artifactBytes, $roundTrip)) {
        throw 'packaged artifact round-trip changed binary payload'
    }

    $benchmark = (& $cliExe --controller $controllerUrl benchmark --cores-per-node 1 `
        --iterations-per-core 1000 --timeout-seconds 30 | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $benchmark.status -ne 'succeeded' -or
        -not $benchmark.all_results_signed -or $benchmark.nodes.Count -ne 1 -or
        -not $benchmark.nodes[0].receipt_id) {
        throw 'packaged worker did not return a signed sustained benchmark receipt'
    }

    $preRestartOfferId = $nodeOffer.offer_id
    Stop-ProcessTree -RootProcessId $controller.Id
    $controller.WaitForExit()
    $env:RAMPAGE_BIND = "127.0.0.1:$ControllerPort"
    $env:RAMPAGE_DATA_DIR = $ownerData
    $controller = Start-Process -FilePath $controllerExe -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $ownerData 'controller.restart.stdout.log') `
        -RedirectStandardError (Join-Path $ownerData 'controller.restart.stderr.log')
    $controllerRecovered = $false
    for ($attempt = 0; $attempt -lt 100; $attempt++) {
        if ($controller.HasExited) {
            $controllerError = Get-Content -Raw (Join-Path $ownerData 'controller.restart.stderr.log') `
                -ErrorAction SilentlyContinue
            throw "release controller restart failed (exit=$($controller.ExitCode)): $controllerError"
        }
        try {
            Invoke-RestMethod "$controllerUrl/health" | Out-Null
            $controllerRecovered = $true
            break
        } catch { Start-Sleep -Milliseconds 100 }
    }
    if (-not $controllerRecovered) { throw 'release controller did not recover on its durable mesh port' }
    $workerRecovered = $false
    for ($attempt = 0; $attempt -lt 300; $attempt++) {
        if ($desktop.HasExited) {
            throw "packaged worker desktop exited instead of surviving controller restart (exit=$($desktop.ExitCode))"
        }
        try {
            $offers = @(Invoke-RestMethod "$controllerUrl/v1/offers" -Headers $headers)
            $recoveredOffer = $offers | Where-Object {
                $_ -and "$($_.node_id)" -eq "$($node.node_id)" -and
                    "$($_.offer_id)" -ne "$preRestartOfferId"
            } | Select-Object -First 1
            if ($recoveredOffer -and $recoveredOffer.mesh_endpoint.signature) {
                $nodeOffer = $recoveredOffer
                $workerRecovered = $true
                break
            }
        } catch { }
        Start-Sleep -Milliseconds 100
    }
    if (-not $workerRecovered) {
        throw 'packaged worker did not reconnect autonomously after the owner controller restarted'
    }

    $desktopId = $desktop.Id
    $firstOfferId = $nodeOffer.offer_id
    $null = $desktop.CloseMainWindow()
    Start-Sleep -Milliseconds 750
    if ($desktop.HasExited) { throw 'closing the worker window exited instead of keeping its contribution in the tray' }
    if (-not $desktop.WaitForExit(155000)) { Stop-ProcessTree -RootProcessId $desktopId; throw 'worker diagnostic exit did not complete' }
    Start-Sleep -Milliseconds 750

    $controllerPin = Join-Path $workerData 'agent.controller-pin.json'
    $oneTimeInvite = Join-Path $workerData 'remote-invite.json'
    if (-not (Test-Path -LiteralPath $controllerPin -PathType Leaf)) {
        throw 'worker did not convert its consumed invitation into a durable controller pin'
    }
    if (Test-Path -LiteralPath $oneTimeInvite) {
        throw 'worker retained the consumed one-time enrollment secret after pinning the controller'
    }

    $env:RAMPAGE_DATA_DIR = $workerData
    $env:RAMPAGE_DIAGNOSTIC_EXIT_AFTER_MS = '30000'
    $restartDesktop = Start-Process -FilePath $desktopExe -PassThru
    $env:RAMPAGE_DIAGNOSTIC_EXIT_AFTER_MS = $oldDiagnosticExit
    $reconnected = $false
    for ($attempt = 0; $attempt -lt 300; $attempt++) {
        if ($restartDesktop.HasExited) {
            throw "pinned worker desktop exited before reconnecting (exit=$($restartDesktop.ExitCode))"
        }
        $offers = @(Invoke-RestMethod "$controllerUrl/v1/offers" -Headers $headers)
        $nodeOffer = $offers | Where-Object {
            $_ -and "$($_.node_id)" -eq "$($node.node_id)" -and "$($_.offer_id)" -ne "$firstOfferId"
        } | Select-Object -First 1
        if ($nodeOffer -and $nodeOffer.mesh_endpoint.signature) {
            $reconnected = $true
            break
        }
        Start-Sleep -Milliseconds 100
    }
    if (-not $reconnected) { throw 'pinned worker did not publish a fresh signed offer after restart' }
    # Allow the bounded 30-second diagnostic timer a cleanup margin for WebView and sidecars on
    # slower Windows machines; the process is still required to terminate autonomously.
    if (-not $restartDesktop.WaitForExit(45000)) {
        Stop-ProcessTree -RootProcessId $restartDesktop.Id
        throw 'restarted pinned worker diagnostic exit did not complete'
    }
    Start-Sleep -Milliseconds 750
    $workerProcesses = @(Get-CimInstance Win32_Process | Where-Object {
        $_.CommandLine -and $_.CommandLine.Contains($workerData)
    })
    if ($workerProcesses.Count -gt 0) {
        $ids = ($workerProcesses.ProcessId -join ', ')
        foreach ($process in $workerProcesses) { Stop-Process -Id $process.ProcessId -Force -ErrorAction SilentlyContinue }
        throw "worker desktop leaked sidecar processes after exit: $ids"
    }

    [pscustomobject]@{
        result = 'PASS'
        mode = 'worker'
        transport = 'authenticated_direct_quic'
        nodes = $nodes.Count
        offers = $offers.Count
        sustained_heartbeat_offers = $offerIds.Count
        invite_signature = $true
        artifact_endpoint_signature = $true
        artifact_digest = $put.digest
        artifact_round_trip = $true
        sustained_benchmark = $true
        benchmark_receipts = $benchmark.nodes.Count
        controller_restart_recovery = $true
        consumed_invite_removed = $true
        pinned_restart = $true
        close_to_tray = $true
        clean_explicit_exit = $true
        sidecar_leak = $false
        data_dir = $smokeRoot
    } | ConvertTo-Json
} finally {
    $env:RAMPAGE_BIND = $oldBind
    $env:RAMPAGE_DATA_DIR = $oldData
    $env:RAMPAGE_TOKEN = $oldToken
    $env:RAMPAGE_DIAGNOSTIC_EXIT_AFTER_MS = $oldDiagnosticExit
    if ($desktop -and -not $desktop.HasExited) { Stop-ProcessTree -RootProcessId $desktop.Id }
    if ($restartDesktop -and -not $restartDesktop.HasExited) { Stop-ProcessTree -RootProcessId $restartDesktop.Id }
    if ($controller -and -not $controller.HasExited) { Stop-ProcessTree -RootProcessId $controller.Id }
    $lateWorkerProcesses = @(Get-CimInstance Win32_Process | Where-Object {
        $_.CommandLine -and $_.CommandLine.Contains($workerData)
    })
    foreach ($process in $lateWorkerProcesses) {
        Stop-Process -Id $process.ProcessId -Force -ErrorAction SilentlyContinue
    }
}
