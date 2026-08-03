param(
    [string]$Executable = 'target\release\rampage-desktop.exe'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$executablePath = if ([System.IO.Path]::IsPathRooted($Executable)) {
    $Executable
} else {
    Join-Path $root $Executable
}
$resolvedExecutable = (Resolve-Path $executablePath).Path
$smokeRoot = Join-Path $root ('output\desktop-smoke-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $smokeRoot | Out-Null

function Get-FreeTcpPort {
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    try {
        $listener.Start()
        return ([Net.IPEndPoint]$listener.LocalEndpoint).Port
    } finally {
        $listener.Stop()
    }
}

function Get-FreeUdpPort {
    $socket = [Net.Sockets.UdpClient]::new(0)
    try {
        return ([Net.IPEndPoint]$socket.Client.LocalEndPoint).Port
    } finally {
        $socket.Dispose()
    }
}

$controllerPort = Get-FreeTcpPort
$intelligencePort = Get-FreeTcpPort
while ($intelligencePort -eq $controllerPort) { $intelligencePort = Get-FreeTcpPort }
$meshPort = Get-FreeUdpPort
$controllerBase = "http://127.0.0.1:$controllerPort"
$oldData = $env:RAMPAGE_DATA_DIR
$oldDiagnosticExit = $env:RAMPAGE_DIAGNOSTIC_EXIT_AFTER_MS
$oldControllerBind = $env:RAMPAGE_BIND
$oldIntelligencePort = $env:RAMPAGE_INTELLIGENCE_PORT
$oldMeshPort = $env:RAMPAGE_MESH_PORT
$env:RAMPAGE_DATA_DIR = $smokeRoot
$env:RAMPAGE_DIAGNOSTIC_EXIT_AFTER_MS = '30000'
$env:RAMPAGE_BIND = "127.0.0.1:$controllerPort"
$env:RAMPAGE_INTELLIGENCE_PORT = $intelligencePort.ToString()
$env:RAMPAGE_MESH_PORT = $meshPort.ToString()
$desktop = Start-Process -FilePath $resolvedExecutable -PassThru
$env:RAMPAGE_DATA_DIR = $oldData
$env:RAMPAGE_DIAGNOSTIC_EXIT_AFTER_MS = $oldDiagnosticExit
$env:RAMPAGE_BIND = $oldControllerBind
$env:RAMPAGE_INTELLIGENCE_PORT = $oldIntelligencePort
$env:RAMPAGE_MESH_PORT = $oldMeshPort

try {
    $health = $null
    for ($attempt = 0; $attempt -lt 150; $attempt++) {
        try {
            $health = Invoke-RestMethod "$controllerBase/health"
            $intelligence = Invoke-RestMethod "http://127.0.0.1:$intelligencePort/health"
            $token = (Get-Content -Raw (Join-Path $smokeRoot 'controller.token')).Trim()
            $headers = @{ 'x-rampage-token' = $token }
            $nodes = Invoke-RestMethod "$controllerBase/v1/nodes" -Headers $headers
            $offers = Invoke-RestMethod "$controllerBase/v1/offers" -Headers $headers
            if ($nodes.Count -ge 1 -and $offers.Count -ge 1 -and $intelligence.status -eq 'ready') { break }
        } catch {
            Start-Sleep -Milliseconds 100
        }
    }
    if (-not $health -or $nodes.Count -lt 1 -or $offers.Count -lt 1 -or
        $intelligence.authority -ne 'proposal_only' -or
        $intelligence.capability -ne 'deterministic_only') {
        throw 'desktop did not autonomously start and enroll its local fabric'
    }
    $null = $desktop.CloseMainWindow()
    Start-Sleep -Milliseconds 750
    if ($desktop.HasExited) { throw 'closing the desktop window exited instead of keeping the fabric in the tray' }
    if (-not $desktop.WaitForExit(32000)) { throw 'desktop diagnostic exit did not complete' }
    Start-Sleep -Milliseconds 500
    $leaked = @(Get-Process | Where-Object {
        $_.Path -and $_.Path.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase) -and
        $_.ProcessName -match '^rampage'
    })
    if ($leaked.Count -gt 0) {
        $names = ($leaked | ForEach-Object { "$($_.ProcessName):$($_.Id)" }) -join ', '
        foreach ($process in $leaked) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
        throw "desktop leaked Rampage sidecars after explicit tray-style exit: $names"
    }
    [pscustomobject]@{
        result = 'PASS'
        controller = $health.status
        intelligence = $intelligence.status
        intelligence_authority = $intelligence.authority
        capability = $intelligence.capability
        mesh_mode = $health.mesh_mode
        mesh_endpoint_id = $health.mesh_endpoint_id
        nodes = $nodes.Count
        offers = $offers.Count
        close_to_tray = $true
        clean_explicit_exit = $true
        data_dir = $smokeRoot
    } | ConvertTo-Json
} finally {
    if ($desktop -and -not $desktop.HasExited) {
        $null = $desktop.CloseMainWindow()
        if (-not $desktop.WaitForExit(5000)) { Stop-Process -Id $desktop.Id -Force }
    }
}
