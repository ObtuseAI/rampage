param(
    [ValidatePattern('^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$')]
    [string]$NodeId,
    [string]$ControllerBase = 'http://127.0.0.1:47831',
    [string]$DataDir = (Join-Path $env:APPDATA 'ai.obtuse.rampage\runtime'),
    [ValidateRange(5, 60)]
    [int]$TimeoutSeconds = 15
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$controllerUri = $null
if (-not [Uri]::TryCreate($ControllerBase, [UriKind]::Absolute, [ref]$controllerUri) -or
    $controllerUri.Scheme -ne 'http') {
    throw 'ControllerBase must be an absolute HTTP loopback URL'
}
$controllerAddress = $null
$controllerIsLoopback = $controllerUri.Host -eq 'localhost' -or
    ([Net.IPAddress]::TryParse($controllerUri.Host, [ref]$controllerAddress) -and
        [Net.IPAddress]::IsLoopback($controllerAddress))
if (-not $controllerIsLoopback) {
    throw 'Remote Assist qualification refuses a non-loopback controller API'
}
$controllerRoot = $controllerUri.AbsoluteUri.TrimEnd('/')

$resolvedDataDir = [IO.Path]::GetFullPath($DataDir)
$tokenPath = Join-Path $resolvedDataDir 'controller.token'
$tokenFile = Get-Item -LiteralPath $tokenPath -Force
if ($tokenFile.PSIsContainer -or
    ($tokenFile.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
    $tokenFile.Length -lt 32 -or $tokenFile.Length -gt 4096) {
    throw "Controller token is not a bounded regular file: $tokenPath"
}
$controllerToken = (Get-Content -LiteralPath $tokenPath -Raw).Trim()
if ($controllerToken -notmatch '^[A-Za-z0-9_-]{32,512}$') {
    throw 'Controller token has an invalid shape'
}

function Invoke-RampageRequest {
    param(
        [Parameter(Mandatory)]
        [ValidateSet('GET', 'POST')]
        [string]$Method,
        [Parameter(Mandatory)]
        [string]$Path,
        [object]$Body,
        [int[]]$ExpectedStatus = @(200)
    )

    $request = @{
        Uri = "$controllerRoot$Path"
        Method = $Method
        Headers = @{ 'x-rampage-token' = $controllerToken }
        TimeoutSec = $TimeoutSeconds
        SkipHttpErrorCheck = $true
    }
    if ($PSBoundParameters.ContainsKey('Body')) {
        $request.ContentType = 'application/json'
        $request.Body = $Body | ConvertTo-Json -Depth 8 -Compress
    }
    $response = Invoke-WebRequest @request
    $parsed = if ([string]::IsNullOrWhiteSpace($response.Content)) {
        $null
    } else {
        $response.Content | ConvertFrom-Json
    }
    if ([int]$response.StatusCode -notin $ExpectedStatus) {
        $detail = if ($parsed -and $parsed.error) { $parsed.error } else { $response.Content }
        throw "Rampage $Method $Path returned HTTP $([int]$response.StatusCode): $detail"
    }
    [pscustomobject]@{
        StatusCode = [int]$response.StatusCode
        Body = $parsed
    }
}

$health = (Invoke-RampageRequest -Method GET -Path '/health').Body
if ($health.status -ne 'ready' -or $health.kill_latch -eq $true) {
    throw 'The owner controller is not ready or its STOP latch is active'
}
if ($health.version -ne '0.3.0') {
    throw "Remote Assist qualification requires controller 0.3.0, found $($health.version)"
}

$nodes = @((Invoke-RampageRequest -Method GET -Path '/v1/nodes').Body)
$offers = @((Invoke-RampageRequest -Method GET -Path '/v1/offers').Body)
$now = [DateTimeOffset]::UtcNow
$eligible = @($offers | Where-Object {
    $offer = $_
    $capability = @($offer.workload_capabilities | Where-Object {
        $_.adapter -eq 'rampage.remote-assist.v1' -and
        $_.status -eq 'shipped' -and
        @($_.operations) -contains 'view'
    })
    $offer.node_id -and
    $offer.mesh_endpoint -and
    @($offer.adapters) -contains 'rampage.remote-assist.v1' -and
    $offer.availability.foreground_allowed -eq $true -and
    [DateTimeOffset]::Parse($offer.expires_at) -gt $now -and
    $capability.Count -eq 1
})

if ($NodeId) {
    $eligible = @($eligible | Where-Object { $_.node_id -eq $NodeId })
    if ($eligible.Count -ne 1) {
        $known = @($nodes | Where-Object { $_.node_id -eq $NodeId }).Count -eq 1
        $reason = if ($known) {
            'the enrolled worker has no fresh opted-in Remote Assist offer'
        } else {
            'the node is not enrolled with this owner'
        }
        throw "Cannot qualify worker ${NodeId}: $reason"
    }
} elseif ($eligible.Count -eq 0) {
    throw 'No live paired worker is advertising opted-in Rampage 0.3.0 Remote Assist'
} elseif ($eligible.Count -gt 1) {
    $choices = ($eligible.node_id | Sort-Object) -join ', '
    throw "More than one Remote Assist worker is eligible; rerun with -NodeId. Choices: $choices"
}

$selected = $eligible[0]
$node = @($nodes | Where-Object { $_.node_id -eq $selected.node_id }) | Select-Object -First 1
if (-not $node) {
    throw 'The eligible Remote Assist offer does not belong to an enrolled node'
}

$session = $null
$framePayload = $null
$frameBytes = $null
$closeReceipt = $null
$qualificationError = $null
$closeError = $null
try {
    $opened = Invoke-RampageRequest -Method POST -Path '/v1/remote-assist/sessions' `
        -ExpectedStatus @(201) -Body @{ node_id = $selected.node_id; mode = 'view' }
    $session = $opened.Body.session
    $issuedAt = [DateTimeOffset]::Parse($session.issued_at)
    $expiresAt = [DateTimeOffset]::Parse($session.expires_at)
    $leaseSeconds = ($expiresAt - $issuedAt).TotalSeconds
    if ($session.schema -ne 'rampage.remote-desktop-lease.v1' -or
        $session.node_id -ne $selected.node_id -or
        $session.mode -ne 'view' -or
        $leaseSeconds -le 0 -or $leaseSeconds -gt 30 -or
        $session.max_width -lt 1 -or $session.max_width -gt 4096 -or
        $session.max_height -lt 1 -or $session.max_height -gt 4096 -or
        $session.max_fps -lt 1 -or $session.max_fps -gt 15 -or
        [string]::IsNullOrWhiteSpace($session.signature)) {
        throw 'The controller returned an invalid Remote Assist lease'
    }

    $frameResponse = Invoke-RampageRequest -Method GET `
        -Path "/v1/remote-assist/sessions/$($session.session_id)/frame"
    $framePayload = $frameResponse.Body
    $frame = $framePayload.frame
    $frameBytes = [Convert]::FromBase64String($framePayload.data_base64)
    $computedDigest = 'sha256:' + [Convert]::ToHexString(
        [Security.Cryptography.SHA256]::HashData($frameBytes)
    ).ToLowerInvariant()
    if ($framePayload.schema -ne 'rampage.remote-desktop-frame-payload.v1' -or
        $framePayload.session_id -ne $session.session_id -or
        $frame.sequence -lt 1 -or
        $frame.width -lt 1 -or $frame.width -gt $session.max_width -or
        $frame.height -lt 1 -or $frame.height -gt $session.max_height -or
        $frame.media_type -ne 'image/jpeg' -or
        $frame.payload_size -ne $frameBytes.Length -or
        $frameBytes.Length -lt 4 -or $frameBytes.Length -gt (4 * 1024 * 1024) -or
        $frame.payload_digest -ne $computedDigest -or
        $frameBytes[0] -ne 0xff -or $frameBytes[1] -ne 0xd8 -or
        $frameBytes[$frameBytes.Length - 2] -ne 0xff -or
        $frameBytes[$frameBytes.Length - 1] -ne 0xd9) {
        throw 'The physical Remote Assist frame failed its contract or digest verification'
    }
} catch {
    $qualificationError = $_
} finally {
    if ($session) {
        try {
            $closeReceipt = (Invoke-RampageRequest -Method POST `
                -Path "/v1/remote-assist/sessions/$($session.session_id)/close").Body
        } catch {
            $closeError = $_
        }
    }
}

if ($qualificationError) { throw $qualificationError }
if ($closeError) { throw $closeError }
if (-not $closeReceipt.closed -or $closeReceipt.duplicate) {
    throw 'Remote Assist close did not revoke the active controller session'
}
$postClose = Invoke-RampageRequest -Method GET `
    -Path "/v1/remote-assist/sessions/$($session.session_id)/frame" -ExpectedStatus @(404)

$frame = $framePayload.frame
$leaseDuration = ([DateTimeOffset]::Parse($session.expires_at) -
    [DateTimeOffset]::Parse($session.issued_at)).TotalSeconds
[pscustomobject]@{
    schema = 'rampage.remote-assist-physical-receipt.v1'
    result = 'PASS'
    controller_version = $health.version
    controller_mesh_endpoint_id = $health.mesh_endpoint_id
    node_id = $selected.node_id
    node_name = $node.display_name
    node_platform = $node.platform
    offer_observed_at = $selected.observed_at
    session_id = $session.session_id
    lease_id = $session.lease_id
    lease_mode = $session.mode
    lease_seconds = $leaseDuration
    fencing_epoch = $session.fencing_epoch
    frame_sequence = $frame.sequence
    frame_captured_at = $frame.captured_at
    frame_width = $frame.width
    frame_height = $frame.height
    frame_bytes = $frameBytes.Length
    frame_sha256 = $frame.payload_digest
    jpeg_contract_verified = $true
    session_close_confirmed = $true
    post_close_http_status = $postClose.StatusCode
    elevation_authority = $false
    secure_desktop_authority = $false
    verified_at = [DateTimeOffset]::UtcNow.ToString('o')
} | ConvertTo-Json -Depth 4
