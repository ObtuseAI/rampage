$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$probe = [System.Net.Sockets.TcpListener]::new(
    [System.Net.IPAddress]::Loopback,
    0
)
$probe.Start()
$ollamaPort = ([System.Net.IPEndPoint]$probe.LocalEndpoint).Port
$probe.Stop()
$fakeOllama = Join-Path $PSScriptRoot 'fixtures\fake_ollama.py'
$stdout = Join-Path $root 'output\fake-ollama.stdout.log'
$stderr = Join-Path $root 'output\fake-ollama.stderr.log'
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $stdout) | Out-Null
$server = Start-Process -FilePath 'python' -ArgumentList @($fakeOllama, '--port', $ollamaPort) `
    -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr

try {
    $ollamaUrl = "http://127.0.0.1:$ollamaPort"
    $ready = $false
    for ($attempt = 0; $attempt -lt 100; $attempt++) {
        try {
            Invoke-RestMethod "$ollamaUrl/api/tags" | Out-Null
            $ready = $true
            break
        } catch {
            if ($server.HasExited) {
                throw "fake Ollama exited early: $(Get-Content -Raw $stderr)"
            }
            Start-Sleep -Milliseconds 100
        }
    }
    if (-not $ready) { throw 'fake Ollama did not become ready' }
    & (Join-Path $PSScriptRoot 'ollama-e2e.ps1') `
        -Model 'rampage-test:latest' -OllamaUrl $ollamaUrl
    if (-not $?) { throw 'deterministic model gateway E2E failed' }
} finally {
    if ($server -and -not $server.HasExited) {
        Stop-Process -Id $server.Id -Force
    }
}
