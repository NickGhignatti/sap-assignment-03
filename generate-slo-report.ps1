# generate-slo-report.ps1
# Queries Prometheus for every SLI, compares each against its SLO target,
# computes error-budget consumption, and writes a Markdown report.
#
# Usage:
#   .\generate-slo-report.ps1
#   .\generate-slo-report.ps1 -Namespace droneflow -PromPort 9090
#
# Output: reports\slo-report-<timestamp>.md  (+ reports\slo-report-latest.md)

param(
    [string]$Namespace = "droneflow",
    [int]$PromPort     = 9090,
    [string]$OutDir    = "reports"
)

$ErrorActionPreference = "Stop"

# -- SLO definitions ----------------------------------------------------------
# Each: target ratio + the PromQL that yields the SLI as a 0..1 value.
$slos = @(
    [pscustomobject]@{
        Id     = "SLO-1"
        Name   = "Saga success rate"
        Type   = "Availability"
        Target = 0.95
        Query  = 'order_saga_completed_total / (order_saga_completed_total + order_saga_failed_total + order_saga_compensated_total)'
    },
    [pscustomobject]@{
        Id     = "SLO-2"
        Name   = "Saga completion < 15s"
        Type   = "Latency"
        Target = 0.90
        Query  = 'order_saga_duration_seconds_bucket{le="15.0"} / ignoring(le) order_saga_duration_seconds_count'
    },
    [pscustomobject]@{
        Id     = "SLO-3"
        Name   = "HTTP availability (non-5xx)"
        Type   = "Availability"
        Target = 0.99
        Query  = '1 - (sum(http_requests_total{status=~"5.."}) or vector(0)) / sum(http_requests_total)'
    },
    [pscustomobject]@{
        Id     = "SLO-4"
        Name   = "Drone assignment success"
        Type   = "Availability"
        Target = 0.90
        Query  = 'drone_assignment_assigned_total / (drone_assignment_assigned_total + drone_assignment_refused_total)'
    }
)

# Supporting raw metrics shown as a snapshot.
$rawMetrics = @(
    'order_saga_started_total',
    'order_saga_completed_total',
    'order_saga_failed_total',
    'order_saga_compensated_total',
    'order_saga_duration_seconds_count',
    'order_saga_duration_seconds_sum',
    'drone_assignment_assigned_total',
    'drone_assignment_refused_total'
)

# -- Prometheus query helper --------------------------------------------------
function Invoke-Prom($query) {
    $uri = "http://localhost:$PromPort/api/v1/query?query=$([uri]::EscapeDataString($query))"
    $resp = Invoke-WebRequest -Uri $uri -UseBasicParsing -TimeoutSec 10 | ConvertFrom-Json
    if ($resp.status -ne "success" -or $resp.data.result.Count -eq 0) { return $null }
    # Instant vector: take the first series' value [timestamp, "value"]
    return $resp.data.result[0].value[1]
}

function Format-Pct($ratio) {
    if ($null -eq $ratio) { return "n/a" }
    return ("{0:N2}%" -f ($ratio * 100))
}

# -- Port-forward to Prometheus -----------------------------------------------
Write-Host "Starting port-forward to prometheus:$PromPort ..." -ForegroundColor Cyan
$pf = Start-Process kubectl `
    -ArgumentList "port-forward svc/prometheus $($PromPort):9090 -n $Namespace" `
    -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 4

try {
    # -- Evaluate each SLO ----------------------------------------------------
    $rows = foreach ($slo in $slos) {
        $raw = Invoke-Prom $slo.Query
        $val = $null
        if ($raw -and $raw -ne "NaN" -and $raw -ne "+Inf") {
            $val = [double]$raw
        }

        if ($null -eq $val) {
            $status = "NO DATA"
            $budget = "n/a"
        } else {
            $met = $val -ge $slo.Target
            $status = if ($met) { "MET" } else { "BREACHED" }
            # Error budget consumed = actual failure / allowed failure.
            $allowed  = 1.0 - $slo.Target
            $actual   = 1.0 - $val
            $consumed = if ($allowed -gt 0) { [math]::Min([math]::Max($actual / $allowed, 0), 9.99) } else { 0 }
            $budget   = ("{0:N0}%" -f ($consumed * 100))
        }

        [pscustomobject]@{
            Id       = $slo.Id
            Name     = $slo.Name
            Type     = $slo.Type
            Target   = Format-Pct $slo.Target
            Measured = Format-Pct $val
            Status   = $status
            Budget   = $budget
            Query    = $slo.Query
        }
    }

    # -- Raw metric snapshot --------------------------------------------------
    $raws = foreach ($m in $rawMetrics) {
        $v = Invoke-Prom $m
        [pscustomobject]@{ Metric = $m; Value = if ($null -eq $v) { "n/a" } else { $v } }
    }

    # -- Build the Markdown ---------------------------------------------------
    $now = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
    $sb  = New-Object System.Collections.Generic.List[string]
    $sb.Add("# SLO Report - Shipping on the Air")
    $sb.Add("")
    $sb.Add("_Generated: ${now}_")
    $sb.Add("")
    $sb.Add("Snapshot of every Service Level Indicator measured by Prometheus, compared against its Service Level Objective.")
    $sb.Add("")
    $sb.Add("## SLO compliance")
    $sb.Add("")
    $sb.Add("| SLO | Indicator | Type | Target | Measured | Status | Error budget used |")
    $sb.Add("|-----|-----------|------|--------|----------|--------|-------------------|")
    foreach ($r in $rows) {
        $sb.Add("| $($r.Id) | $($r.Name) | $($r.Type) | $($r.Target) | $($r.Measured) | $($r.Status) | $($r.Budget) |")
    }
    $sb.Add("")
    $sb.Add("> Error budget used = (1 - measured) / (1 - target). Above 100% means the objective is breached.")
    $sb.Add("")
    $sb.Add("## Raw metric snapshot")
    $sb.Add("")
    $sb.Add("| Metric | Value |")
    $sb.Add("|--------|-------|")
    foreach ($r in $raws) { $sb.Add("| ``$($r.Metric)`` | $($r.Value) |") }
    $sb.Add("")
    $sb.Add("## PromQL used")
    $sb.Add("")
    foreach ($r in $rows) {
        $sb.Add("**$($r.Id): $($r.Name)**")
        $sb.Add('```promql')
        $sb.Add($r.Query)
        $sb.Add('```')
        $sb.Add("")
    }

    # -- Write files ----------------------------------------------------------
    if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }
    $stamp     = Get-Date -Format "yyyyMMdd-HHmmss"
    $stampPath = Join-Path $OutDir "slo-report-$stamp.md"
    $latest    = Join-Path $OutDir "slo-report-latest.md"
    $content   = ($sb -join "`r`n")
    $content | Out-File -FilePath $stampPath -Encoding utf8
    $content | Out-File -FilePath $latest    -Encoding utf8

    Write-Host "`nReport written to:" -ForegroundColor Green
    Write-Host "  $stampPath"
    Write-Host "  $latest"
    Write-Host ""
    # Echo the compliance table to the console too.
    $rows | Format-Table Id, Name, Target, Measured, Status, Budget -AutoSize
}
finally {
    Stop-Process -Id $pf.Id -ErrorAction SilentlyContinue
}
