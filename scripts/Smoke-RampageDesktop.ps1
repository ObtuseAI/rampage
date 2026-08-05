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
$neutralRoot = Join-Path $root ('output\desktop-neutral-smoke-' + [guid]::NewGuid().ToString('N'))
$smokeRoot = Join-Path $root ('output\desktop-smoke-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $neutralRoot | Out-Null
New-Item -ItemType Directory -Path $smokeRoot | Out-Null

$oldData = $env:RAMPAGE_DATA_DIR
$oldDiagnosticExit = $env:RAMPAGE_DIAGNOSTIC_EXIT_AFTER_MS
$neutralStdout = Join-Path $neutralRoot 'desktop.stdout.log'
$neutralStderr = Join-Path $neutralRoot 'desktop.stderr.log'
$neutralDesktop = $null
try {
    $env:RAMPAGE_DATA_DIR = $neutralRoot
    $env:RAMPAGE_DIAGNOSTIC_EXIT_AFTER_MS = '5000'
    $neutralDesktop = Start-Process -FilePath $resolvedExecutable -PassThru `
        -RedirectStandardOutput $neutralStdout -RedirectStandardError $neutralStderr
} finally {
    $env:RAMPAGE_DATA_DIR = $oldData
    $env:RAMPAGE_DIAGNOSTIC_EXIT_AFTER_MS = $oldDiagnosticExit
}
try {
    $neutralMarker = Join-Path $neutralRoot 'setup-required-v1.ready'
    for ($attempt = 0; $attempt -lt 100 -and -not (Test-Path -LiteralPath $neutralMarker); $attempt++) {
        if ($neutralDesktop.HasExited) { break }
        Start-Sleep -Milliseconds 100
    }
    if (-not (Test-Path -LiteralPath $neutralMarker -PathType Leaf)) {
        $neutralError = Get-Content -Raw $neutralStderr -ErrorAction SilentlyContinue
        throw "empty runtime did not enter neutral setup: $neutralError"
    }
    foreach ($forbidden in @('owner-fabric-v1.ready', 'controller.token', 'agent.controller-pin.json')) {
        if (Test-Path -LiteralPath (Join-Path $neutralRoot $forbidden)) {
            throw "neutral first run created forbidden fabric authority: $forbidden"
        }
    }
    if (-not $neutralDesktop.WaitForExit(12000)) {
        throw 'neutral first-run diagnostic exit did not complete'
    }
    if ($neutralDesktop.ExitCode -ne 0) {
        $neutralError = Get-Content -Raw $neutralStderr -ErrorAction SilentlyContinue
        throw "neutral first run exited with code $($neutralDesktop.ExitCode): $neutralError"
    }
} finally {
    if ($neutralDesktop -and -not $neutralDesktop.HasExited) {
        Stop-Process -Id $neutralDesktop.Id -Force -ErrorAction SilentlyContinue
    }
}

# The local-fabric half of this smoke is an explicitly configured owner, not an implicit first
# run. Neutral onboarding above owns the role decision; this marker pair exercises the packaged
# controller, agent, intelligence service, tray lifecycle, and signed local offer after that
# decision has been made.
[IO.File]::WriteAllText(
    (Join-Path $smokeRoot 'owner-fabric-v1.ready'),
    "rampage.owner-fabric.v1`n",
    [Text.UTF8Encoding]::new($false)
)
[IO.File]::WriteAllText(
    (Join-Path $smokeRoot 'owner-confirmed-v1.ready'),
    "rampage.owner-confirmed.v1`n",
    [Text.UTF8Encoding]::new($false)
)

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
$oldControllerBind = $env:RAMPAGE_BIND
$oldIntelligencePort = $env:RAMPAGE_INTELLIGENCE_PORT
$oldMeshPort = $env:RAMPAGE_MESH_PORT
$env:RAMPAGE_DATA_DIR = $smokeRoot
$env:RAMPAGE_DIAGNOSTIC_EXIT_AFTER_MS = '90000'
$env:RAMPAGE_BIND = "127.0.0.1:$controllerPort"
$env:RAMPAGE_INTELLIGENCE_PORT = $intelligencePort.ToString()
$env:RAMPAGE_MESH_PORT = $meshPort.ToString()
$desktopStdout = Join-Path $smokeRoot 'desktop.stdout.log'
$desktopStderr = Join-Path $smokeRoot 'desktop.stderr.log'
$desktop = Start-Process -FilePath $resolvedExecutable -PassThru `
    -RedirectStandardOutput $desktopStdout -RedirectStandardError $desktopStderr
$env:RAMPAGE_DATA_DIR = $oldData
$env:RAMPAGE_DIAGNOSTIC_EXIT_AFTER_MS = $oldDiagnosticExit
$env:RAMPAGE_BIND = $oldControllerBind
$env:RAMPAGE_INTELLIGENCE_PORT = $oldIntelligencePort
$env:RAMPAGE_MESH_PORT = $oldMeshPort

function Stop-ProcessTree([int]$RootProcessId) {
    $children = @(Get-CimInstance Win32_Process | Where-Object ParentProcessId -eq $RootProcessId)
    foreach ($child in $children) { Stop-ProcessTree -RootProcessId $child.ProcessId }
    Stop-Process -Id $RootProcessId -Force -ErrorAction SilentlyContinue
}

try {
    $health = $null
    # A cold PyInstaller one-file extraction can exceed 15 seconds on otherwise supported Windows
    # machines. Keep the gate bounded while allowing the packaged intelligence service to unpack.
    for ($attempt = 0; $attempt -lt 600; $attempt++) {
        if ($desktop.HasExited) {
            $desktopError = Get-Content -Raw $desktopStderr -ErrorAction SilentlyContinue
            throw "desktop exited before its local fabric became ready (exit=$($desktop.ExitCode)): $desktopError"
        }
        try {
            $health = Invoke-RestMethod "$controllerBase/health"
            $intelligence = Invoke-RestMethod "http://127.0.0.1:$intelligencePort/health"
            $token = (Get-Content -Raw (Join-Path $smokeRoot 'controller.token')).Trim()
            $headers = @{ 'x-rampage-token' = $token }
            $nodes = Invoke-RestMethod "$controllerBase/v1/nodes" -Headers $headers
            $offers = Invoke-RestMethod "$controllerBase/v1/offers" -Headers $headers
            $ownerOffer = @($offers | Where-Object { $_.mesh_endpoint.signature } | Select-Object -First 1)
            if ($nodes.Count -ge 1 -and $offers.Count -ge 1 -and $ownerOffer.Count -eq 1 -and
                $intelligence.status -eq 'ready') { break }
        } catch {
            Start-Sleep -Milliseconds 100
        }
    }
    if (-not $health -or $nodes.Count -lt 1 -or $offers.Count -lt 1 -or $ownerOffer.Count -ne 1 -or
        $intelligence.authority -ne 'proposal_only' -or
        $intelligence.capability -ne 'deterministic_only') {
        throw 'desktop did not autonomously start and enroll its local fabric'
    }
    $null = $desktop.CloseMainWindow()
    Start-Sleep -Milliseconds 750
    if ($desktop.HasExited) { throw 'closing the desktop window exited instead of keeping the fabric in the tray' }
    if (-not $desktop.WaitForExit(92000)) { throw 'desktop diagnostic exit did not complete' }
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
        neutral_first_run = $true
        controller = $health.status
        intelligence = $intelligence.status
        intelligence_authority = $intelligence.authority
        capability = $intelligence.capability
        mesh_mode = $health.mesh_mode
        mesh_endpoint_id = $health.mesh_endpoint_id
        nodes = $nodes.Count
        offers = $offers.Count
        owner_mesh_endpoint = $true
        close_to_tray = $true
        clean_explicit_exit = $true
        data_dir = $smokeRoot
    } | ConvertTo-Json
} finally {
    if ($desktop -and -not $desktop.HasExited) {
        $desktopId = $desktop.Id
        $null = $desktop.CloseMainWindow()
        if (-not $desktop.WaitForExit(5000)) { Stop-ProcessTree -RootProcessId $desktopId }
    }
    # Setup failures can terminate the shell before its Exit handler drains already-started
    # sidecars. Remove only processes whose executable is inside this exact candidate directory.
    $candidateDirectory = [IO.Path]::GetFullPath((Split-Path -Parent $resolvedExecutable))
    $candidateProcesses = @(Get-CimInstance Win32_Process | Where-Object {
        $_.Name -like 'rampage*.exe' -and $_.ExecutablePath -and
        [IO.Path]::GetFullPath($_.ExecutablePath).StartsWith(
            $candidateDirectory + [IO.Path]::DirectorySeparatorChar,
            [StringComparison]::OrdinalIgnoreCase
        )
    })
    foreach ($process in $candidateProcesses) {
        Stop-ProcessTree -RootProcessId $process.ProcessId
    }
}
