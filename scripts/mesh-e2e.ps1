$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$runRoot = Join-Path $root ('output\mesh-e2e-' + [guid]::NewGuid().ToString('N'))
$controllerData = Join-Path $runRoot 'controller'
$agentData = Join-Path $runRoot 'agent'
New-Item -ItemType Directory -Force -Path $controllerData, $agentData | Out-Null
$controllerExe = Join-Path $root 'target\debug\rampage-controller.exe'
$agentExe = Join-Path $root 'target\debug\rampage-agent.exe'
$inviteFile = Join-Path $runRoot 'invite.json'
$agentKey = Join-Path $agentData 'agent.key'

$oldData = $env:RAMPAGE_DATA_DIR
$agent = $null
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
    $agent = Start-Process -FilePath $agentExe -ArgumentList $agentArguments `
        -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $agentData 'agent.stdout.log') `
        -RedirectStandardError (Join-Path $agentData 'agent.stderr.log')
    $env:RAMPAGE_DATA_DIR = $oldData
    $node = $null
    $nodeOffer = $null
    for ($attempt = 0; $attempt -lt 150; $attempt++) {
        $nodes = @(Invoke-RestMethod 'http://127.0.0.1:47831/v1/nodes' -Headers $headers)
        $offers = @(Invoke-RestMethod 'http://127.0.0.1:47831/v1/offers' -Headers $headers)
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

    $artifactText = 'RAMPAGE_STORAGE_PROOF_' + [guid]::NewGuid().ToString('N')
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
        storage_class = 'cache'
    } | ConvertTo-Json
    $replica = Invoke-RestMethod 'http://127.0.0.1:47831/v1/artifacts/replicate' `
        -Method Post -ContentType 'application/json' -Headers $headers -Body $replicateBody
    if ($replica.artifact.digest -ne $stored.digest) { throw 'remote replica digest changed' }
    $remoteChunk = Get-ChildItem (Join-Path $agentData 'cas\objects') -Recurse -Filter '*.chunk' |
        Select-Object -First 1
    if (-not $remoteChunk) { throw 'worker did not persist the replica in its donated store' }
    $ciphertext = [System.IO.File]::ReadAllBytes($remoteChunk.FullName)
    if ([Convert]::ToBase64String($ciphertext) -eq [Convert]::ToBase64String($artifactBytes)) {
        throw 'worker artifact was stored as plaintext'
    }
    $retrieveBody = @{
        digest = $stored.digest
        node_id = $node.node_id
    } | ConvertTo-Json
    $retrieved = Invoke-RestMethod 'http://127.0.0.1:47831/v1/artifacts/retrieve' `
        -Method Post -ContentType 'application/json' -Headers $headers -Body $retrieveBody
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
    if ("$($lease.node_id)" -ne "$($node.node_id)") {
        throw 'mesh job placed on an unexpected worker'
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
        node_id = $node.node_id
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
    if (-not ($events.event_type -contains 'artifact.replicated') -or
        -not ($events.event_type -contains 'artifact.retrieved') -or
        -not ($events.event_type -contains 'artifact.input.staged') -or
        -not ($events.event_type -contains 'artifact.output.recorded')) {
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
        lease = $lease.lease_id
        receipted = $true
        artifact_digest = $stored.digest
        artifact_round_trip = $true
        encrypted_at_rest = $true
        staged_job_input = $jobStored.digest
        retrievable_job_output = $outputArtifact.digest
        artifacts = $runRoot
    } | ConvertTo-Json
} finally {
    $env:RAMPAGE_DATA_DIR = $oldData
    if ($controller -and -not $controller.HasExited) {
        Stop-Process -Id $controller.Id -Force
    }
    if ($agent -and -not $agent.HasExited) {
        Stop-Process -Id $agent.Id -Force
    }
}
