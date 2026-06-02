# Changelog — Kubernetes & Service Levels iteration

This iteration completed the two remaining assignment items — **Kubernetes deployment** and **SLOs/SLIs with measurement** — and added supporting tooling and documentation. Deep-dive docs live under `documentation/code_documentation/` (`k8s.qd`, `slo.qd`, `agent.qd`); this file is the consolidated summary.

---

## 1. Kubernetes deployment

A full deployment of the system on **k3d** (k3s in Docker), replacing `docker-compose` as the orchestration target while keeping the same application contract (env vars, ports, Kafka topics).

### Manifests added (`k8s/`)

| File | Resources | Notes |
|---|---|---|
| `namespace.yml` | Namespace `droneflow` | isolates all resources |
| `configmap.yml` | ConfigMap `droneflow-config` | `KAFKA_BROKERS`, Kafka UI config |
| `secret.yml` | Secret `droneflow-secret` | Mongo creds + `MONGODB_URI` |
| `mongodb.yml` | headless Service + StatefulSet | persistent via `volumeClaimTemplates` |
| `kafka.yml` | headless + ClusterIP Service + StatefulSet | KRaft mode, single broker |
| `kafka-ui.yml` | Deployment + Service | monitoring UI |
| `order-service.yml` | Deployment + Service | port 8080, Mongo + Kafka |
| `delivery-service.yml` | Deployment + Service | port 8081, Kafka only |
| `drone-service.yml` | Deployment + Service | port 8082, Mongo + Kafka |
| `ingress.yml` | Ingress (Traefik) | routes `/order`, `/drone` |
| `prometheus.yml` | ConfigMap + Deployment + Service | metrics scraping (see §2) |

### Design decisions
- **StatefulSet** for MongoDB & Kafka (stable storage + DNS); **Deployment** for the stateless services and UIs.
- **Headless Services** (`clusterIP: None`) for the StatefulSets; short Service DNS names (`mongodb`, `kafka`) for client access.
- **ConfigMap vs Secret** split: non-sensitive config (Kafka broker) in the ConfigMap, credentials/URI in the Secret. Both injected with `envFrom`.
- **Ingress** routes only the two services with HTTP APIs; `delivery-service` is event-driven only and is intentionally excluded.

### Integration bugs fixed (compose → K8s)
| Symptom | Cause | Fix |
|---|---|---|
| `POST /` timed out | app reads `MONGODB_URI`, ConfigMap had `MONGO_URI` → fallback to `localhost` → Mongo server-selection hang | renamed key to `MONGODB_URI` |
| Mongo auth refused | URI had no credentials / `authSource` | `mongodb://root:root@mongodb:27017/?authSource=admin` in the Secret |
| Auth still failing after password change | StatefulSet only initialises the root user on an **empty** volume | `kubectl delete pvc -l app=mongodb` to force re-init |
| Kafka wouldn't start | comma-separated env values had spaces; `CLUSTER_ID` wrong length | no spaces; 22-char base64url `CLUSTER_ID` |
| `ErrImagePull` | local images not in cluster | `k3d image import ...`, `imagePullPolicy: IfNotPresent` |
| Docker build failed on `common` crate | build context was the service subdir | build from repo root: `docker build -f .\order\Dockerfile .` |

Full detail + deployment sequence: **`documentation/code_documentation/k8s.qd`**.

---

## 2. SLOs / SLIs and measurement

Four SLI/SLO pairs defined and instrumented via **Prometheus**. The two required pairs are #1 and #2 (covering both availability and latency archetypes); #3 and #4 are extensions.

| # | SLO | SLI metric |
|---|---|---|
| 1 | ≥ 95% sagas complete successfully | `order_saga_{completed,failed,compensated}_total` |
| 2 | ≥ 90% sagas complete within 15s | `order_saga_duration_seconds` (Histogram) |
| 3 | ≥ 99% HTTP responses non-5xx | `http_requests_total{status}` (CounterVec) |
| 4 | ≥ 90% drone assignments succeed | `drone_assignment_{assigned,refused}_total` |

### Code changes

**Order Service**
- `order/Cargo.toml` — added `prometheus = "0.14.0"`.
- `order/src/orchestrator.rs` — `SagaMetrics` now holds `prometheus::IntCounter`s + an `order_saga_duration_seconds` Histogram (buckets `[1, 2.5, 5, 10, 15, 30]`), registered in a `Registry`; `complete_saga()` observes the saga duration.
- `order/src/api.rs` — `GET /metrics` handler (gather + `TextEncoder`).
- `order/src/http_metrics.rs` *(new)* — `HttpMetrics` (`http_requests_total{status}`) + `track_metrics` Axum middleware counting every response by status code.
- `order/src/main.rs` — creates the `Registry`, merges the `/metrics` route, applies the metrics middleware layer.

**Drone Service**
- `drone/Cargo.toml` — added `prometheus = "0.14.0"`.
- `drone/src/metrics.rs` *(new)* — `DroneMetrics` (`assigned` / `refused` counters).
- `drone/src/fleet.rs` — `DroneFleet` holds `Arc<DroneMetrics>`; `dispatch_order` increments based on the agent's `deliberate()` decision (no-drone-available counts as a refusal).
- `drone/src/api.rs` + `main.rs` — `GET /metrics` endpoint + `Registry`, wired via `.merge()`.

### Prometheus (static config)
- ConfigMap mounted as a **file** at `/etc/prometheus/prometheus.yml` (volume mount, not env var).
- Scrapes `order-service:8080` and `drone-service:8082` every 15s.

### PromQL gotchas captured (in `slo.qd`)
- **Vector matching** — dividing series with mismatched labels needs `/ ignoring(le)`.
- **`le` is a string** — float buckets serialise as `"15.0"`, not `"15"`.
- **`histogram_quantile(rate(...[5m]))` → NaN** when there's no recent traffic.
- **Scrape timing** — after a pod restart / config reload, queries return `NaN`/empty until the next scrape cycle (~15s).

Full detail: **`documentation/code_documentation/slo.qd`**.

---

## 3. Test & ops scripts (repo root)

| Script | Purpose |
|---|---|
| `health-check.ps1` | Pod/Service/Ingress status + `/health` pings via port-forward |
| `test-k8s.ps1` | End-to-end flow: create order → verify saga progression → drone status |
| `generate-slo-report.ps1` | Queries Prometheus for every SLI, compares vs SLO targets, computes error-budget usage, writes `reports/slo-report-*.md` |

> PowerShell 5.1 note: both scripts are ASCII-only — Unicode characters (em-dash, box-drawing) break the parser.

---

## 4. Documentation

| File | Change |
|---|---|
| `documentation/code_documentation/k8s.qd` | *new* — full K8s deployment doc |
| `documentation/code_documentation/slo.qd` | *new* — SLO/SLI doc (4 SLIs, PromQL, Prometheus) |
| `documentation/code_documentation/agent.qd` | *new* — BDI agentic drone redesign |
| `documentation/code_documentation/_nav.qd` | added links to the three new pages |
| `documentation/code_documentation/drone.qd` | fixed a stray-text typo on the `.docname` line |
| `README.md` | checked off **SLOs/SLIs** and **K8s deploy** TODOs |
| `CHANGELOG.md` | *new* — this file |

---

## 5. Status

All four README assignment items are complete:

- [x] Event-driven redesign (Kafka)
- [x] Agent-based drone (BDI) + prototype
- [x] Kubernetes deployment
- [x] SLOs/SLIs + measurement

### Known limitations (worth a line in the report)
- The Prometheus Deployment has **no persistent volume** — metrics reset if its pod restarts. Acceptable for a demo.
- The drone fleet is fixed at **3 agents**; under sustained load it saturates and refuses deliveries (observable via SLI #4 dropping).
- In-memory counters reset on pod restart, so SLI windows restart with each deploy.
