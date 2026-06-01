# health-check.ps1
# Quick check that all pods are Running and both HTTP services respond.

param([string]$Namespace = "droneflow")

Write-Host "`n=== Pod Status ===" -ForegroundColor Cyan
kubectl get pods -n $Namespace

Write-Host "`n=== Service Status ===" -ForegroundColor Cyan
kubectl get services -n $Namespace

Write-Host "`n=== Ingress ===" -ForegroundColor Cyan
kubectl get ingress -n $Namespace

# Quick health pings via port-forward
Write-Host "`n=== Health Pings ===" -ForegroundColor Cyan

foreach ($svc in @(
    @{ Name="order-service"; LocalPort=9001; SvcPort=8080; Path="/health" },
    @{ Name="drone-service";  LocalPort=9002; SvcPort=8082; Path="/health" }
)) {
    $pf = Start-Process kubectl `
        -ArgumentList "port-forward svc/$($svc.Name) $($svc.LocalPort):$($svc.SvcPort) -n $Namespace" `
        -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 2

    try {
        $r = Invoke-WebRequest -Uri "http://localhost:$($svc.LocalPort)$($svc.Path)" `
            -UseBasicParsing -TimeoutSec 5
        Write-Host "  $($svc.Name): HTTP $($r.StatusCode) - OK" -ForegroundColor Green
    } catch {
        Write-Host "  $($svc.Name): UNREACHABLE - $($_.Exception.Message)" -ForegroundColor Red
    } finally {
        Stop-Process -Id $pf.Id -ErrorAction SilentlyContinue
    }
}
