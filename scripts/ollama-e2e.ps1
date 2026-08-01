param(
    [string]$Model = 'llama3.2:latest'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$runRoot = Join-Path $root ('output\ollama-e2e-' + [guid]::NewGuid().ToString('N'))
$controllerData = Join-Path $runRoot 'controller'
$agentData = Join-Path $runRoot 'agent'
New-Item -ItemType Directory -Force -Path $controllerData, $agentData | Out-Null
$controllerExe = Join-Path $root 'target\debug\rampage-controller.exe'
$agentExe = Join-Path $root 'target\debug\rampage-agent.exe'
$cliExe = Join-Path $root 'target\debug\rampage.exe'

$oldData = $env:RAMPAGE_DATA_DIR
$oldToken = $env:RAMPAGE_TOKEN
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
    $tags = Invoke-RestMethod 'http://127.0.0.1:11434/api/tags'
    if (-not ($tags.models.name -contains $Model)) { throw "Ollama model is not installed: $Model" }
    $invite = Invoke-RestMethod 'http://127.0.0.1:47831/v1/enrollment/invites' `
        -Method Post -ContentType 'application/json' -Headers $headers -Body '{}'
    $agentArgs = @(
        '--controller', 'http://127.0.0.1:47831',
        '--key-file', (Join-Path $agentData 'agent.key'),
        '--enrollment-code', $invite.enrollment_code,
        '--display-name', 'Ollama-E2E-Worker',
        '--device-kind', 'desktop',
        '--serve'
    )
    $env:RAMPAGE_DATA_DIR = $agentData
    $agent = Start-Process -FilePath $agentExe -ArgumentList $agentArgs -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $runRoot 'agent.stdout.log') `
        -RedirectStandardError (Join-Path $runRoot 'agent.stderr.log')
    $env:RAMPAGE_DATA_DIR = $oldData
    $offer = $null
    for ($attempt = 0; $attempt -lt 200; $attempt++) {
        $offers = @(Invoke-RestMethod 'http://127.0.0.1:47831/v1/offers' -Headers $headers)
        if ($agent.HasExited) {
            throw "worker exited early with code $($agent.ExitCode): $(Get-Content -Raw (Join-Path $runRoot 'agent.stderr.log'))"
        }
        $offer = $offers | Where-Object { @($_.adapters) -contains 'rampage.ollama.v1' } | Select-Object -First 1
        if ($offer) { break }
        Start-Sleep -Milliseconds 100
    }
    if (-not $offer) {
        throw "worker did not advertise the Ollama adapter: $($offers | ConvertTo-Json -Depth 8 -Compress)"
    }
    $receiptJson = & $cliExe generate $Model 'Reply with exactly RAMPAGE_OK.' `
        --max-tokens 16 --timeout-seconds 120
    if ($LASTEXITCODE -ne 0) { throw 'Rampage generation command failed' }
    $receipt = $receiptJson | ConvertFrom-Json
    if ($receipt.state -ne 'succeeded' -or -not $receipt.result) {
        throw "Ollama job did not return a successful signed result: $receiptJson"
    }
    [pscustomobject]@{
        result = 'PASS'
        adapter = 'rampage.ollama.v1'
        model = $Model
        node = $receipt.node_id
        receipt = $receipt.receipt_id
        response = $receipt.result
        stdout_digest = $receipt.stdout_digest
        artifacts = $runRoot
    } | ConvertTo-Json
} finally {
    $env:RAMPAGE_DATA_DIR = $oldData
    $env:RAMPAGE_TOKEN = $oldToken
    if ($agent -and -not $agent.HasExited) { Stop-Process -Id $agent.Id -Force }
    if ($controller -and -not $controller.HasExited) { Stop-Process -Id $controller.Id -Force }
}
