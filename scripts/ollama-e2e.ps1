param(
    [string]$Model = 'llama3.2:latest',
    [string]$OllamaUrl = 'http://127.0.0.1:11434'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$runRoot = Join-Path $root ('output\ollama-e2e-' + [guid]::NewGuid().ToString('N'))
$controllerData = Join-Path $runRoot 'controller'
$agentData = Join-Path $runRoot 'agent'
New-Item -ItemType Directory -Force -Path $controllerData, $agentData | Out-Null
$controllerExe = Join-Path $root 'target\debug\rampage-controller.exe'
$agentExe = Join-Path $root 'target\debug\rampage-agent.exe'
$inviteFile = Join-Path $runRoot 'invite.json'

$oldData = $env:RAMPAGE_DATA_DIR
$oldToken = $env:RAMPAGE_TOKEN
$oldOllamaUrl = $env:RAMPAGE_OLLAMA_URL
$env:RAMPAGE_DATA_DIR = $controllerData
$controller = Start-Process -FilePath $controllerExe -PassThru -WindowStyle Hidden
$env:RAMPAGE_DATA_DIR = $oldData
$agent = $null

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
    $env:RAMPAGE_TOKEN = (Get-Content -Raw (Join-Path $controllerData 'controller.token')).Trim()
    $headers = @{ 'x-rampage-token' = $env:RAMPAGE_TOKEN }
    $tags = Invoke-RestMethod "$OllamaUrl/api/tags"
    if (-not (($tags.models | ForEach-Object { $_.model }) -contains $Model) -and
        -not (($tags.models | ForEach-Object { $_.name }) -contains $Model)) {
        throw "Ollama model is not installed: $Model"
    }
    $invite = Invoke-RestMethod 'http://127.0.0.1:47831/v1/enrollment/invites' `
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
        $offers = @(Invoke-RestMethod 'http://127.0.0.1:47831/v1/offers' -Headers $headers)
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
    $unauthorized = Invoke-WebRequest 'http://127.0.0.1:47831/v1/models' -SkipHttpErrorCheck
    if ($unauthorized.StatusCode -ne 401) { throw 'OpenAI gateway accepted a tokenless request' }
    $openAiHeaders = @{ Authorization = "Bearer $($env:RAMPAGE_TOKEN)" }
    $models = Invoke-RestMethod 'http://127.0.0.1:47831/v1/models' -Headers $openAiHeaders
    if (-not (@($models.data.id) -contains $Model)) {
        throw "OpenAI gateway did not expose the consistent installed model: $($models | ConvertTo-Json -Depth 8 -Compress)"
    }
    $request = @{
        model = $Model
        messages = @(@{ role = 'user'; content = 'Reply with exactly RAMPAGE_OK.' })
        max_completion_tokens = 16
        stream = $false
    } | ConvertTo-Json -Depth 10
    $completion = Invoke-RestMethod 'http://127.0.0.1:47831/v1/chat/completions' `
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
    $stream = Invoke-WebRequest 'http://127.0.0.1:47831/v1/chat/completions' `
        -Method Post -ContentType 'application/json' -Headers $openAiHeaders -Body $streamRequest
    if ($stream.StatusCode -ne 200 -or $stream.Content -notmatch 'data: \[DONE\]' -or
        $stream.Content -notmatch 'chat.completion.chunk') {
        throw "OpenAI-compatible streaming response was malformed: $($stream.Content)"
    }
    $events = @(Invoke-RestMethod 'http://127.0.0.1:47831/v1/events?after=0&limit=1000' -Headers $headers)
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
        signed_receipt_evidence = $true
        tokenless_request_denied = $true
        response = $completion.choices[0].message.content
        artifacts = $runRoot
    } | ConvertTo-Json
} finally {
    $env:RAMPAGE_DATA_DIR = $oldData
    $env:RAMPAGE_TOKEN = $oldToken
    $env:RAMPAGE_OLLAMA_URL = $oldOllamaUrl
    if ($agent -and -not $agent.HasExited) { Stop-Process -Id $agent.Id -Force }
    if ($controller -and -not $controller.HasExited) { Stop-Process -Id $controller.Id -Force }
}
