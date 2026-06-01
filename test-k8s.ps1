# test-k8s.ps1
# Full integration test for the Droneflow K8s deployment.
# Uses kubectl port-forward to bypass Ingress path-prefix issues.
#
# Usage:
#   .\test-k8s.ps1
#   .\test-k8s.ps1 -Verbose

param(
    [string]$Namespace = "droneflow",
    [int]$OrderPort  = 9001,
    [int]$DronePort  = 9002
)

$ErrorActionPreference = "Continue"

# -- Helpers ------------------------------------------------------------------

function Write-Section($title) {
    Write-Host "`n$("="*50)" -ForegroundColor Cyan
    Write-Host "  $title" -ForegroundColor Cyan
    Write-Host "$("="*50)" -ForegroundColor Cyan
}

function Invoke-Test {
    param(
        [string]$Name,
        [string]$Url,
        [string]$Method = "GET",
        [hashtable]$Body = $null
    )
    Write-Host "`n[TEST] $Name" -ForegroundColor Yellow
    Write-Host "       $Method $Url"
    try {
        $params = @{
            Uri            = $Url
            Method         = $Method
            UseBasicParsing = $true
            TimeoutSec     = 10
        }
        if ($Body) {
            $params.Body        = ($Body | ConvertTo-Json -Depth 5)
            $params.ContentType = "application/json"
        }
        $resp = Invoke-WebRequest @params
        Write-Host "  [OK] Status $($resp.StatusCode)" -ForegroundColor Green
        # Try to pretty-print JSON; fall back to raw text (e.g. /health returns plain text)
        try {
            $parsed = $resp.Content | ConvertFrom-Json
            Write-Host "  $($parsed | ConvertTo-Json -Depth 5)"
            return $parsed
        } catch {
            Write-Host "  $($resp.Content)"
            return $resp.Content
        }
    } catch {
        $code = $_.Exception.Response.StatusCode.value__
        Write-Host "  [FAIL] $($_.Exception.Message)" -ForegroundColor Red
        if ($code) { Write-Host "  HTTP $code" -ForegroundColor Red }
        return $null
    }
}

# -- Port-forwards ------------------------------------------------------------

Write-Section "Starting port-forwards"

$pfOrder = Start-Process kubectl `
    -ArgumentList "port-forward svc/order-service $($OrderPort):8080 -n $Namespace" `
    -PassThru -WindowStyle Hidden
Write-Host "  order-service  -> localhost:$OrderPort  (PID $($pfOrder.Id))"

$pfDrone = Start-Process kubectl `
    -ArgumentList "port-forward svc/drone-service $($DronePort):8082 -n $Namespace" `
    -PassThru -WindowStyle Hidden
Write-Host "  drone-service  -> localhost:$DronePort  (PID $($pfDrone.Id))"

Write-Host "  Waiting for tunnels to stabilise..."
Start-Sleep -Seconds 3

# -- Tests --------------------------------------------------------------------

try {

    # 1. Health checks
    Write-Section "Health Checks"
    Invoke-Test "Order Service /health"  "http://localhost:$OrderPort/health"
    Invoke-Test "Drone Service /health"  "http://localhost:$DronePort/health"

    # 2. Create an order
    Write-Section "Create Order"
    $orderBody = @{
        customer_id               = "test-customer-01"
        from_address              = "Via Torino 10, Milano"
        to_address                = "Via Roma 1, Roma"
        package_weight            = 1.5
        max_delivery_time_minutes = 120          # required field
        # requested_delivery_time = "2026-06-02T10:00:00Z"   # optional ISO-8601
    }
    $created = Invoke-Test "POST / (create order)" "http://localhost:$OrderPort/" "POST" $orderBody

    if (-not $created) {
        Write-Host "`n[SKIP] Order creation failed - skipping downstream tests." -ForegroundColor Red
        return
    }

    $orderId = $created.order_id
    $sagaId  = $created.saga_id
    Write-Host "`n  order_id = $orderId"
    Write-Host "  saga_id  = $sagaId"

    # Give the saga/Kafka pipeline time to run through all steps
    # (OrderValidation -> DeliveryScheduling -> DroneAssignment -> Completed ~= 12s)
    Write-Host "`n  Waiting 15s for saga to complete..." -ForegroundColor DarkGray
    Start-Sleep -Seconds 15

    # 3. Saga status
    Write-Section "Saga and Drone Status"
    Invoke-Test "GET /{orderId}/saga-status" `
        "http://localhost:$OrderPort/$orderId/saga-status"

    # 4. Drone order status
    Invoke-Test "GET /order/{orderId}/status (drone-svc)" `
        "http://localhost:$DronePort/order/$orderId/status"

    # 5. Order events (event-sourcing log)
    Invoke-Test "GET /order/{orderId}/events (drone-svc)" `
        "http://localhost:$DronePort/order/$orderId/events"

} finally {

    # -- Cleanup --------------------------------------------------------------
    Write-Section "Cleanup"
    Stop-Process -Id $pfOrder.Id -ErrorAction SilentlyContinue
    Stop-Process -Id $pfDrone.Id -ErrorAction SilentlyContinue
    Write-Host "  Port-forwards closed."
}

Write-Section "Done"
