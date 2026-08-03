param(
    [string]$Model = 'llama3.2:latest',
    [string]$OllamaUrl = 'http://127.0.0.1:11434',
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $SkipBuild) {
    & cargo build --manifest-path (Join-Path $root 'Cargo.toml') `
        -p rampage-controller -p rampage-agent
    if ($LASTEXITCODE -ne 0) {
        throw "could not build current Rampage controller and agent binaries"
    }
}
$runRoot = Join-Path $root ('output\ollama-e2e-' + [guid]::NewGuid().ToString('N'))
$controllerData = Join-Path $runRoot 'controller'
$agentData = Join-Path $runRoot 'agent'
New-Item -ItemType Directory -Force -Path $controllerData, $agentData | Out-Null
$controllerExe = Join-Path $root 'target\debug\rampage-controller.exe'
$agentExe = Join-Path $root 'target\debug\rampage-agent.exe'
$inviteFile = Join-Path $runRoot 'invite.json'

function Get-FreeTcpPort {
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    try {
        $listener.Start()
        return ([Net.IPEndPoint]$listener.LocalEndpoint).Port
    } finally {
        $listener.Stop()
    }
}

$controllerPort = Get-FreeTcpPort
$controllerBase = "http://127.0.0.1:$controllerPort"

$oldData = $env:RAMPAGE_DATA_DIR
$oldBind = $env:RAMPAGE_BIND
$oldToken = $env:RAMPAGE_TOKEN
$oldOllamaUrl = $env:RAMPAGE_OLLAMA_URL
$env:RAMPAGE_DATA_DIR = $controllerData
$env:RAMPAGE_BIND = "127.0.0.1:$controllerPort"
$controller = Start-Process -FilePath $controllerExe -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $controllerData 'controller.stdout.log') `
    -RedirectStandardError (Join-Path $controllerData 'controller.stderr.log')
$env:RAMPAGE_DATA_DIR = $oldData
$env:RAMPAGE_BIND = $oldBind
$agent = $null

try {
    $health = $null
    for ($attempt = 0; $attempt -lt 150; $attempt++) {
        if ($controller.HasExited) {
            $controllerError = Get-Content -Raw (Join-Path $controllerData 'controller.stderr.log') `
                -ErrorAction SilentlyContinue
            throw "controller exited before readiness (exit=$($controller.ExitCode)): $controllerError"
        }
        try {
            $health = Invoke-RestMethod "$controllerBase/health"
            break
        } catch {
            Start-Sleep -Milliseconds 100
        }
    }
    if (-not $health) { throw 'controller did not become ready' }
    $env:RAMPAGE_TOKEN = (Get-Content -Raw (Join-Path $controllerData 'controller.token')).Trim()
    $headers = @{ 'x-rampage-token' = $env:RAMPAGE_TOKEN }
    $tags = Invoke-RestMethod "$OllamaUrl/api/tags"
    if (-not (($tags.models | ForEach-Object { $_.model }) -contains $Model) -and
        -not (($tags.models | ForEach-Object { $_.name }) -contains $Model)) {
        throw "Ollama model is not installed: $Model"
    }
    $invite = Invoke-RestMethod "$controllerBase/v1/enrollment/invites" `
        -Method Post -ContentType 'application/json' -Headers $headers -Body '{}'
    $invite | ConvertTo-Json -Depth 20 | Set-Content -Encoding utf8 $inviteFile
    $agentArgs = @(
        '--invite-file', $inviteFile,
        '--key-file', (Join-Path $agentData 'agent.key'),
        '--display-name', 'Ollama-E2E-Worker',
        '--device-kind', 'desktop',
        '--serve'
    )
    $env:RAMPAGE_DATA_DIR = $agentData
    $env:RAMPAGE_OLLAMA_URL = $OllamaUrl
    $agent = Start-Process -FilePath $agentExe -ArgumentList $agentArgs -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $runRoot 'agent.stdout.log') `
        -RedirectStandardError (Join-Path $runRoot 'agent.stderr.log')
    $env:RAMPAGE_DATA_DIR = $oldData
    $env:RAMPAGE_OLLAMA_URL = $oldOllamaUrl
    $offer = $null
    for ($attempt = 0; $attempt -lt 200; $attempt++) {
        $offers = @(Invoke-RestMethod "$controllerBase/v1/offers" -Headers $headers)
        if ($agent.HasExited) {
            throw "worker exited early with code $($agent.ExitCode): $(Get-Content -Raw (Join-Path $runRoot 'agent.stderr.log'))"
        }
        $offer = $offers | Where-Object {
            @($_.adapters) -contains 'rampage.ollama.v1' -and
            @(($_.model_runtimes | ForEach-Object { $_.installed_models }) |
                ForEach-Object { $_.model_id }) -contains $Model
        } | Select-Object -First 1
        if ($offer) { break }
        Start-Sleep -Milliseconds 100
    }
    if (-not $offer) {
        throw "worker did not advertise the Ollama adapter: $($offers | ConvertTo-Json -Depth 8 -Compress)"
    }
    $unauthorized = Invoke-WebRequest "$controllerBase/v1/models" -SkipHttpErrorCheck
    if ($unauthorized.StatusCode -ne 401) { throw 'OpenAI gateway accepted a tokenless request' }
    $openAiHeaders = @{ Authorization = "Bearer $($env:RAMPAGE_TOKEN)" }
    $models = Invoke-RestMethod "$controllerBase/v1/models" -Headers $openAiHeaders
    if (-not (@($models.data.id) -contains $Model)) {
        throw "OpenAI gateway did not expose the consistent installed model: $($models | ConvertTo-Json -Depth 8 -Compress)"
    }
    $openRouterModels = Invoke-RestMethod "$controllerBase/api/v1/models" -Headers $openAiHeaders
    if (-not (@($openRouterModels.data.id) -contains $Model)) {
        throw 'OpenRouter-compatible model alias did not expose the installed model'
    }
    $capabilities = Invoke-RestMethod "$controllerBase/v1/capabilities" -Headers $openAiHeaders
    if ($capabilities.schema -ne 'rampage.gateway-capabilities.v1' -or
        -not (@($capabilities.protocols.id) -contains 'anthropic.messages')) {
        throw 'gateway capability discovery is missing the Anthropic protocol'
    }
    $workloads = Invoke-RestMethod "$controllerBase/v1/workload-capabilities" -Headers $headers
    $ollamaCapability = @($workloads.nodes.capabilities) | Where-Object {
        $_.adapter -eq 'rampage.ollama.v1' -and @($_.operations) -contains 'chat'
    } | Select-Object -First 1
    if ($workloads.schema -ne 'rampage.workload-capability-inventory.v1' -or
        -not $ollamaCapability -or $workloads.candidate_authority -ne $false) {
        throw 'signed workload capability discovery did not expose the exact Ollama chat operation'
    }
    $selfScan = Invoke-RestMethod "$controllerBase/v1/diagnostics/self-scan" -Headers $headers
    if ($selfScan.schema -ne 'rampage.fabric-diagnostic-report.v1' -or
        $selfScan.autonomy.per_change_approval_required -ne $false -or
        $selfScan.evidence_digest -notmatch '^sha256:[0-9a-f]{64}$') {
        throw 'autonomously thresholded self-scan did not return stable evidence'
    }
    $relayAccess = Invoke-RestMethod "$controllerBase/v1/mesh/relay-access" -Headers $headers
    if ($relayAccess.schema -ne 'rampage.relay-access-manifest.v1' -or
        $relayAccess.fabric_id -notmatch '^sha256:[0-9a-f]{64}$' -or
        $relayAccess.generation -lt 1 -or
        @($relayAccess.allowed_endpoint_ids).Count -lt 2 -or
        [string]::IsNullOrWhiteSpace($relayAccess.signature)) {
        throw 'controller did not export a bounded Governor-signed owner-relay allowlist'
    }
    $gateNames = @(
        'g0_schema_policy_static',
        'g1_deterministic_replay',
        'g2_quality_reliability_cost',
        'g3_sealed_holdout',
        'g4_adversarial_security',
        'g5_independent_replication',
        'g6_shadow',
        'g7_canary_rollback'
    )
    $proposalId = [guid]::NewGuid().ToString()
    $promotionCandidate = @{
        schema = 'rampage.promotion-candidate.v1'
        proposal_id = $proposalId
        project_id = [guid]::NewGuid().ToString()
        base_revision = 'e2e-fixture'
        candidate_digest = 'sha256:' + ('b' * 64)
        changed_paths = @('routing/e2e.toml')
        risk = 'r0_configuration'
        gates = @($gateNames | ForEach-Object {
            @{
                name = $_
                passed = $true
                evidence_digest = 'sha256:' + ('c' * 64)
                independent = $_ -eq 'g5_independent_replication'
            }
        })
        requested_at = [DateTimeOffset]::UtcNow.ToString('o')
        expires_at = [DateTimeOffset]::UtcNow.AddMinutes(5).ToString('o')
    } | ConvertTo-Json -Depth 10
    $canary = Invoke-RestMethod "$controllerBase/v1/improvements/canary" `
        -Method Post -ContentType 'application/json' -Headers $headers -Body $promotionCandidate
    $canaryRepeat = Invoke-RestMethod "$controllerBase/v1/improvements/canary" `
        -Method Post -ContentType 'application/json' -Headers $headers -Body $promotionCandidate
    if ($canary.schema -ne 'rampage.promotion-canary-lease.v1' -or
        [string]::IsNullOrWhiteSpace($canary.signature) -or
        $canary.canary_id -ne $canaryRepeat.canary_id -or
        $canary.max_traffic_basis_points -gt 1000) {
        throw 'Rust Governor did not return a bounded idempotent signed canary lease'
    }
    $request = @{
        model = $Model
        messages = @(@{ role = 'user'; content = 'Reply with exactly RAMPAGE_OK.' })
        max_completion_tokens = 16
        stream = $false
    } | ConvertTo-Json -Depth 10
    $completion = Invoke-RestMethod "$controllerBase/v1/chat/completions" `
        -Method Post -ContentType 'application/json' -Headers $openAiHeaders -Body $request
    if ($completion.object -ne 'chat.completion' -or
        [string]::IsNullOrWhiteSpace($completion.choices[0].message.content)) {
        throw "OpenAI-compatible completion was malformed: $($completion | ConvertTo-Json -Depth 8 -Compress)"
    }
    $streamRequest = @{
        model = $Model
        messages = @(@{ role = 'user'; content = 'Reply with exactly STREAM_OK.' })
        max_completion_tokens = 16
        stream = $true
    } | ConvertTo-Json -Depth 10
    $stream = Invoke-WebRequest "$controllerBase/v1/chat/completions" `
        -Method Post -ContentType 'application/json' -Headers $openAiHeaders -Body $streamRequest
    if ($stream.StatusCode -ne 200 -or $stream.Content -notmatch 'data: \[DONE\]' -or
        $stream.Content -notmatch 'chat.completion.chunk') {
        throw "OpenAI-compatible streaming response was malformed: $($stream.Content)"
    }
    $openRouterCompletion = Invoke-RestMethod "$controllerBase/api/v1/chat/completions" `
        -Method Post -ContentType 'application/json' -Headers $openAiHeaders -Body $request
    if ($openRouterCompletion.choices[0].message.content -ne 'RAMPAGE_OK') {
        throw 'OpenRouter-compatible path did not return the expected completion'
    }
    $anthropicHeaders = @{
        'x-api-key' = $env:RAMPAGE_TOKEN
        'anthropic-version' = '2023-06-01'
    }
    $anthropicRequest = @{
        model = $Model
        max_tokens = 16
        system = 'Answer exactly as requested.'
        messages = @(@{ role = 'user'; content = 'Reply with exactly RAMPAGE_OK.' })
        stream = $false
    } | ConvertTo-Json -Depth 10
    $anthropic = Invoke-RestMethod "$controllerBase/v1/messages" `
        -Method Post -ContentType 'application/json' -Headers $anthropicHeaders -Body $anthropicRequest
    if ($anthropic.type -ne 'message' -or $anthropic.role -ne 'assistant' -or
        $anthropic.content[0].type -ne 'text' -or $anthropic.content[0].text -ne 'RAMPAGE_OK' -or
        $anthropic.usage.output_tokens -lt 1 -or $anthropic.usage.output_tokens -gt 16) {
        throw "Anthropic-compatible response was malformed: $($anthropic | ConvertTo-Json -Depth 8 -Compress)"
    }
    $anthropicStreamRequest = @{
        model = $Model
        max_tokens = 16
        messages = @(@{
            role = 'user'
            content = @(@{ type = 'text'; text = 'Reply with exactly STREAM_OK.' })
        })
        stream = $true
    } | ConvertTo-Json -Depth 10
    $anthropicStream = Invoke-WebRequest "$controllerBase/v1/messages" `
        -Method Post -ContentType 'application/json' -Headers $anthropicHeaders -Body $anthropicStreamRequest
    if ($anthropicStream.StatusCode -ne 200 -or
        $anthropicStream.Content -notmatch 'event: message_start' -or
        $anthropicStream.Content -notmatch 'event: content_block_delta' -or
        $anthropicStream.Content -notmatch 'event: message_stop') {
        throw "Anthropic-compatible streaming response was malformed: $($anthropicStream.Content)"
    }
    $events = @(Invoke-RestMethod "$controllerBase/v1/events?after=0&limit=1000" -Headers $headers)
    if (-not ($events.event_type -contains 'model.session.lease.issued') -or
        -not ($events.event_type -contains 'model.session.receipted')) {
        throw 'model lease or signed terminal receipt evidence is missing'
    }
    [pscustomobject]@{
        result = 'PASS'
        gateway = 'openai_chat_completions_subset'
        transport = 'authenticated_direct_quic'
        adapter = 'rampage.ollama.v1'
        model = $Model
        node = $offer.node_id
        non_streaming = $true
        streaming = $true
        anthropic_messages = $true
        openrouter_paths = $true
        capability_discovery = $true
        workload_capability_contract = $true
        autonomously_thresholded_self_scan = $true
        signed_owner_relay_access = $true
        signed_autonomous_canary = $true
        signed_receipt_evidence = $true
        tokenless_request_denied = $true
        response = $completion.choices[0].message.content
        artifacts = $runRoot
    } | ConvertTo-Json
} finally {
    $env:RAMPAGE_DATA_DIR = $oldData
    $env:RAMPAGE_BIND = $oldBind
    $env:RAMPAGE_TOKEN = $oldToken
    $env:RAMPAGE_OLLAMA_URL = $oldOllamaUrl
    if ($agent -and -not $agent.HasExited) { Stop-Process -Id $agent.Id -Force }
    if ($controller -and -not $controller.HasExited) { Stop-Process -Id $controller.Id -Force }
}
