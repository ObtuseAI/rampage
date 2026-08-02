param(
    [string]$ControllerExecutable = 'target\debug\rampage-controller.exe',
    [string]$AgentExecutable = 'target\debug\rampage-agent.exe'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$runRoot = Join-Path $root ('output\mesh-e2e-' + [guid]::NewGuid().ToString('N'))
$controllerData = Join-Path $runRoot 'controller'
$agentData = Join-Path $runRoot 'agent'
$agentTwoData = Join-Path $runRoot 'agent-two'
New-Item -ItemType Directory -Force -Path $controllerData, $agentData, $agentTwoData | Out-Null
$controllerPath = if ([System.IO.Path]::IsPathRooted($ControllerExecutable)) {
    $ControllerExecutable
} else {
    Join-Path $root $ControllerExecutable
}
$agentPath = if ([System.IO.Path]::IsPathRooted($AgentExecutable)) {
    $AgentExecutable
} else {
    Join-Path $root $AgentExecutable
}
$controllerExe = (Resolve-Path $controllerPath).Path
$agentExe = (Resolve-Path $agentPath).Path
$inviteFile = Join-Path $runRoot 'invite.json'
$inviteTwoFile = Join-Path $runRoot 'invite-two.json'
$agentKey = Join-Path $agentData 'agent.key'
$agentTwoKey = Join-Path $agentTwoData 'agent.key'

$oldData = $env:RAMPAGE_DATA_DIR
$oldProtectedStorage = $env:RAMPAGE_ALLOW_PROTECTED_STORAGE
$agent = $null
$agentTwo = $null
$env:RAMPAGE_DATA_DIR = $controllerData
$controller = Start-Process -FilePath $controllerExe -PassThru -WindowStyle Hidden
$env:RAMPAGE_DATA_DIR = $oldData

try {
    $health = $null
    for ($attempt = 0; $attempt -lt 150; $attempt++) {
        try {
            $health = Invoke-RestMethod 'http://127.0.0.1:47831/health'
            break
        } catch {
            Start-Sleep -Milliseconds 100
        }
    }
    if (-not $health) { throw 'controller did not become ready' }
    $token = (Get-Content -Raw (Join-Path $controllerData 'controller.token')).Trim()
    $headers = @{ 'x-rampage-token' = $token }
    $invite = Invoke-RestMethod 'http://127.0.0.1:47831/v1/enrollment/invites' `
        -Method Post -ContentType 'application/json' -Headers $headers -Body '{}'
    if (-not $invite.controller_mesh.signature) { throw 'invite lacks signed mesh endpoint' }
    if ($invite.controller_mesh.direct_addresses.Count -lt 1 -and
        $invite.controller_mesh.relay_urls.Count -lt 1) {
        throw 'invite has no dialable mesh address'
    }
    $invite | ConvertTo-Json -Depth 20 | Set-Content -Encoding utf8 $inviteFile

    $agentArguments = @(
        '--invite-file', $inviteFile,
        '--key-file', $agentKey,
        '--display-name', 'Remote-QUIC-E2E',
        '--device-kind', 'desktop',
        '--serve'
    )
    $env:RAMPAGE_DATA_DIR = $agentData
    $env:RAMPAGE_ALLOW_PROTECTED_STORAGE = 'true'
    $agent = Start-Process -FilePath $agentExe -ArgumentList $agentArguments `
        -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $agentData 'agent.stdout.log') `
        -RedirectStandardError (Join-Path $agentData 'agent.stderr.log')
    $env:RAMPAGE_DATA_DIR = $oldData
    $node = $null
    $nodeOffer = $null
    for ($attempt = 0; $attempt -lt 150; $attempt++) {
        $nodes = @(foreach ($item in (Invoke-RestMethod 'http://127.0.0.1:47831/v1/nodes' -Headers $headers)) { $item })
        $offers = @(foreach ($item in (Invoke-RestMethod 'http://127.0.0.1:47831/v1/offers' -Headers $headers)) { $item })
        $node = $nodes | Select-Object -First 1
        $nodeOffer = $offers | Where-Object {
            "$($_.node_id)" -eq "$($node.node_id)"
        } | Select-Object -First 1
        if ($node -and $nodeOffer -and $nodeOffer.mesh_endpoint.signature -and
            $nodeOffer.link_benchmark.samples -ge 3) { break }
        Start-Sleep -Milliseconds 100
    }
    if (-not $node -or -not $nodeOffer) {
        throw "mesh-enrolled worker did not publish an offer (node=$($node | ConvertTo-Json -Compress), offer=$($offers[0] | ConvertTo-Json -Compress))"
    }
    if (-not $nodeOffer.mesh_endpoint.signature) {
        throw 'worker offer omitted its signed artifact endpoint'
    }
    if ($nodeOffer.link_benchmark.transport -ne 'authenticated_quic' -or
        $nodeOffer.link_benchmark.rtt_micros_p50 -le 0 -or
        $nodeOffer.link_benchmark.uplink_bps -le 0 -or
        $nodeOffer.link_benchmark.downlink_bps -le 0) {
        throw 'worker offer omitted a valid authenticated link benchmark'
    }

    $inviteTwo = Invoke-RestMethod 'http://127.0.0.1:47831/v1/enrollment/invites' `
        -Method Post -ContentType 'application/json' -Headers $headers -Body '{}'
    $inviteTwo | ConvertTo-Json -Depth 20 | Set-Content -Encoding utf8 $inviteTwoFile
    $agentTwoArguments = @(
        '--invite-file', $inviteTwoFile,
        '--key-file', $agentTwoKey,
        '--display-name', 'Remote-QUIC-E2E-Two',
        '--device-kind', 'desktop',
        '--serve'
    )
    $env:RAMPAGE_DATA_DIR = $agentTwoData
    $agentTwo = Start-Process -FilePath $agentExe -ArgumentList $agentTwoArguments `
        -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $agentTwoData 'agent.stdout.log') `
        -RedirectStandardError (Join-Path $agentTwoData 'agent.stderr.log')
    $env:RAMPAGE_DATA_DIR = $oldData
    $env:RAMPAGE_ALLOW_PROTECTED_STORAGE = $oldProtectedStorage
    $nodeTwo = $null
    for ($attempt = 0; $attempt -lt 150; $attempt++) {
        $nodes = @(foreach ($item in (Invoke-RestMethod 'http://127.0.0.1:47831/v1/nodes' -Headers $headers)) { $item })
        $offers = @(foreach ($item in (Invoke-RestMethod 'http://127.0.0.1:47831/v1/offers' -Headers $headers)) { $item })
        $nodeTwo = $nodes | Where-Object { "$($_.node_id)" -ne "$($node.node_id)" } |
            Select-Object -First 1
        $nodeTwoOffer = $offers | Where-Object {
            "$($_.node_id)" -eq "$($nodeTwo.node_id)"
        } | Select-Object -First 1
        if ($nodeTwo -and $nodeTwoOffer -and $nodeTwoOffer.mesh_endpoint.signature -and
            $nodeTwoOffer.link_benchmark.samples -ge 3) { break }
        Start-Sleep -Milliseconds 100
    }
    if (-not $nodeTwo -or -not $nodeTwoOffer) {
        throw 'second independent mesh worker did not publish a qualified offer'
    }
    $nodeTwo = @($nodeTwo)[0]
    $nodeTwoOffer = @($nodeTwoOffer)[0]
    if ("$($nodeTwo.node_id)" -eq "$($node.node_id)") {
        throw 'repair worker is not independent from the primary replica worker'
    }

    $artifactText = 'RAMPAGE_STORAGE_PROOF_' + [guid]::NewGuid().ToString('N') +
        ('x' * (4 * 1024 * 1024 + 257))
    $artifactBytes = [System.Text.Encoding]::UTF8.GetBytes($artifactText)
    $putBody = @{
        data_base64 = [Convert]::ToBase64String($artifactBytes)
        media_type = 'text/plain'
        storage_class = 'cache'
    } | ConvertTo-Json
    $stored = Invoke-RestMethod 'http://127.0.0.1:47831/v1/artifacts/put' `
        -Method Post -ContentType 'application/json' -Headers $headers -Body $putBody
    $replicateBody = @{
        digest = $stored.digest
        node_id = $node.node_id
        media_type = 'text/plain'
        storage_class = 'protected'
    } | ConvertTo-Json
    $replica = Invoke-RestMethod 'http://127.0.0.1:47831/v1/artifacts/replicate' `
        -Method Post -ContentType 'application/json' -Headers $headers -Body $replicateBody
    if ($replica.artifact.digest -ne $stored.digest) { throw 'remote replica digest changed' }
    if (-not $replica.transfer_session_id -or $replica.resumed_chunks -ne 0 -or
        $replica.chunk_count -lt 2 -or
        -not $replica.replica_receipt.signature -or
        $replica.replica_receipt.challenge_nonce.Length -lt 1) {
        throw 'replica omitted restart-safe session or signed challenge evidence'
    }
    $repair = Invoke-RestMethod 'http://127.0.0.1:47831/v1/artifacts/repair' `
        -Method Post -ContentType 'application/json' -Headers $headers -Body '{}'
    if ($repair.per_change_approval_required -or $repair.authority_expansion -ne 'denied' -or
        $repair.fresh_replica_receipts -lt 2) {
        throw 'autonomous protected-storage reconciliation violated its authority envelope'
    }
    $diagnostics = Invoke-RestMethod 'http://127.0.0.1:47831/v1/diagnostics/self-scan' `
        -Headers $headers
    if ($diagnostics.metrics.under_replicated_protected_artifacts -ne 0) {
        throw 'autonomous repair did not establish two fresh independent replica receipts'
    }
    $remoteChunk = Get-ChildItem (Join-Path $agentData 'cas\objects') -Recurse -Filter '*.chunk' |
        Select-Object -First 1
    if (-not $remoteChunk) { throw 'worker did not persist the replica in its donated store' }
    $ciphertext = [System.IO.File]::ReadAllBytes($remoteChunk.FullName)
    if ([Convert]::ToBase64String($ciphertext) -eq [Convert]::ToBase64String($artifactBytes)) {
        throw 'worker artifact was stored as plaintext'
    }
    $repairedChunks = @(Get-ChildItem (Join-Path $agentTwoData 'cas\objects') -Recurse -Filter '*.chunk')
    if ($repairedChunks.Count -lt 2) {
        throw 'autonomous repair did not persist the complete multi-chunk second replica'
    }
    $retrieveBody = @{
        digest = $stored.digest
        node_id = $node.node_id
    } | ConvertTo-Json
    $retrieved = Invoke-RestMethod 'http://127.0.0.1:47831/v1/artifacts/retrieve' `
        -Method Post -ContentType 'application/json' -Headers $headers -Body $retrieveBody
    if (-not $retrieved.transfer_session_id) {
        throw 'retrieval omitted its restart-safe transfer session'
    }
    $retrievedText = [System.Text.Encoding]::UTF8.GetString(
        [Convert]::FromBase64String($retrieved.data_base64)
    )
    if ($retrievedText -ne $artifactText) { throw 'remote artifact round trip changed content' }

    $jobArtifactText = 'RAMPAGE_JOB_INPUT_' + [guid]::NewGuid().ToString('N')
    $jobArtifactBytes = [System.Text.Encoding]::UTF8.GetBytes($jobArtifactText)
    $jobPutBody = @{
        data_base64 = [Convert]::ToBase64String($jobArtifactBytes)
        media_type = 'text/plain'
        storage_class = 'cache'
    } | ConvertTo-Json
    $jobStored = Invoke-RestMethod 'http://127.0.0.1:47831/v1/artifacts/put' `
        -Method Post -ContentType 'application/json' -Headers $headers -Body $jobPutBody
    $jobId = [guid]::NewGuid().ToString()
    $job = @{
        schema = 'rampage.job-spec.v1'
        job_id = $jobId
        project_id = [guid]::NewGuid().ToString()
        submitted_by = [guid]::NewGuid().ToString()
        adapter = 'rampage.artifact-hash.v1'
        operation = 'hash_artifact'
        arguments = @{}
        inputs = @($jobStored)
        requests = @(
            @{
                class = 'cpu_compute'; minimum = 1; preferred = 1
                unit = 'logical_core'; required_labels = @{}
            },
            @{
                class = 'storage_cache'; minimum = $jobStored.size_bytes
                preferred = $jobStored.size_bytes; unit = 'byte'; required_labels = @{}
            }
        )
        trust = 'native_trusted'
        restart_tolerant = $true
        network_allowlist = @()
        deadline = (Get-Date).ToUniversalTime().AddMinutes(5).ToString('o')
        idempotency_key = [guid]::NewGuid().ToString()
    } | ConvertTo-Json -Depth 20
    $plan = Invoke-RestMethod 'http://127.0.0.1:47831/v1/jobs/plan' `
        -Method Post -ContentType 'application/json' -Headers $headers -Body $job
    $nodeScore = @($plan.scores) | Where-Object { "$($_.node_id)" -eq "$($node.node_id)" } |
        Select-Object -First 1
    if (-not $nodeScore -or $nodeScore.topology_confidence -ne 'measured' -or
        $nodeScore.link_downlink_bps -le 0 -or $nodeScore.estimated_transfer_millis -le 0) {
        throw "placement plan did not use signed topology evidence: $($nodeScore | ConvertTo-Json -Compress)"
    }
    $lease = Invoke-RestMethod 'http://127.0.0.1:47831/v1/jobs' `
        -Method Post -ContentType 'application/json' -Headers $headers -Body $job
    if ("$($lease.node_id)" -ne "$($node.node_id)" -and
        "$($lease.node_id)" -ne "$($nodeTwo.node_id)") {
        throw 'mesh job placed outside the two enrolled workers'
    }

    $receipted = $null
    for ($attempt = 0; $attempt -lt 200; $attempt++) {
        $events = @(Invoke-RestMethod 'http://127.0.0.1:47831/v1/events?after=0&limit=1000' -Headers $headers)
        $receipted = $events | Where-Object {
            $_.event_type -eq 'job.receipted' -and $_.subject_id -eq $jobId
        }
        if ($receipted) { break }
        Start-Sleep -Milliseconds 100
    }
    if (-not $receipted) { throw 'remote mesh receipt was not recorded' }
    $outputArtifact = @($receipted.payload.outputs) | Select-Object -First 1
    if (-not $outputArtifact.digest) { throw 'artifact worker produced no retrievable output' }
    $outputRetrieveBody = @{
        digest = $outputArtifact.digest
        node_id = $lease.node_id
    } | ConvertTo-Json
    $outputRetrieved = Invoke-RestMethod 'http://127.0.0.1:47831/v1/artifacts/retrieve' `
        -Method Post -ContentType 'application/json' -Headers $headers -Body $outputRetrieveBody
    $outputReport = [System.Text.Encoding]::UTF8.GetString(
        [Convert]::FromBase64String($outputRetrieved.data_base64)
    ) | ConvertFrom-Json
    if ($outputReport.input_digest -ne $jobStored.digest -or
        $outputReport.observed_digest -ne $jobStored.digest) {
        throw 'worker output did not prove the staged input digest'
    }
    # job.receipted is durably appended immediately before artifact.output.recorded. Poll for the
    # complete evidence set instead of reusing the receipt snapshot and racing the next append.
    $artifactEvidence = $false
    for ($attempt = 0; $attempt -lt 100; $attempt++) {
        $events = @(Invoke-RestMethod 'http://127.0.0.1:47831/v1/events?after=0&limit=1000' -Headers $headers)
        # Fresh signed PUT/repair receipts already prove possession; the budgeted reconciler must
        # not immediately reread the same multi-MiB objects merely to add a duplicate event.
        $artifactEvidence = ($events.event_type -contains 'artifact.replicated') -and
            ($events.event_type -contains 'artifact.repaired') -and
            ($events.event_type -contains 'artifact.retrieved') -and
            ($events.event_type -contains 'artifact.input.staged') -and
            ($events.event_type -contains 'artifact.output.recorded')
        if ($artifactEvidence) { break }
        Start-Sleep -Milliseconds 50
    }
    if (-not $artifactEvidence) {
        throw 'artifact transfer evidence was not recorded'
    }
    [pscustomobject]@{
        result = 'PASS'
        transport = 'authenticated_direct_quic'
        controller_endpoint = $invite.controller_mesh.endpoint_id
        direct_addresses = $invite.controller_mesh.direct_addresses.Count
        rtt_micros_p50 = $nodeOffer.link_benchmark.rtt_micros_p50
        uplink_bps = $nodeOffer.link_benchmark.uplink_bps
        downlink_bps = $nodeOffer.link_benchmark.downlink_bps
        estimated_staging_millis = $nodeScore.estimated_transfer_millis
        topology_confidence = $nodeScore.topology_confidence
        node = $node.node_id
        repair_node = $nodeTwo.node_id
        lease = $lease.lease_id
        receipted = $true
        artifact_digest = $stored.digest
        artifact_round_trip = $true
        encrypted_at_rest = $true
        resumable_transfer_session = $replica.transfer_session_id
        transfer_chunks = $replica.chunk_count
        signed_replica_receipt = $true
        independent_replicas = 2
        autonomous_repair = $true
        staged_job_input = $jobStored.digest
        retrievable_job_output = $outputArtifact.digest
        artifacts = $runRoot
    } | ConvertTo-Json
} finally {
    $env:RAMPAGE_DATA_DIR = $oldData
    $env:RAMPAGE_ALLOW_PROTECTED_STORAGE = $oldProtectedStorage
    if ($controller -and -not $controller.HasExited) {
        Stop-Process -Id $controller.Id -Force
    }
    if ($agent -and -not $agent.HasExited) {
        Stop-Process -Id $agent.Id -Force
    }
    if ($agentTwo -and -not $agentTwo.HasExited) {
        Stop-Process -Id $agentTwo.Id -Force
    }
}
