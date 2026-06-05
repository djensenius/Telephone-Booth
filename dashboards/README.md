# Grafana dashboards

JSON dashboards for the Telephone Booth observability stack.
See [`docs/observability.md`](../docs/observability.md) for the full
data flow and the metric catalog.

## Layout

| File                          | Title                          | Focus                                                                 |
| ----------------------------- | ------------------------------ | --------------------------------------------------------------------- |
| `booth-overview.json`         | Booth — Overview               | CPU temp, load, memory, uptime, network throughput, calls/day.        |
| `booth-call-activity.json`    | Booth — Call activity          | Calls per outcome, dialed digit histogram, recording + upload timing. |
| `booth-audio.json`            | Booth — Audio & operator HTTP  | Input/output dBFS, operator request rate, p95 latency, dropped events.|
| `booth-combined.json`         | Telephone Booth (tabbed)       | All of the above combined into one dashboard with three tabs (Grafana 12+, schema v2). |

The three single-focus dashboards use the classic dashboard schema
(`schemaVersion: 39`) and import into any modern Grafana.
`booth-combined.json` is the same panels reorganised into one dashboard
with a tab per section, using Grafana's newer dashboard schema
(`dashboard.grafana.app/v2`). Use it if you prefer one dashboard
with tabs; keep the three classic files if you run an older Grafana or
provision dashboards individually.

All of them use a `$booth` template variable populated from the
`booth_id` label, so they work out of the box for single- and
multi-booth deployments.

## Datasource

The dashboards expect a Prometheus-compatible datasource named
`VictoriaMetrics` with uid `VictoriaMetrics`. Adjust either the
datasource block in your Grafana provisioning or edit each file to
match your existing uid.

A minimal provisioning datasource looks like:

```yaml
apiVersion: 1
datasources:
  - name: VictoriaMetrics
    uid: VictoriaMetrics
    type: prometheus
    access: proxy
    url: http://victoriametrics:8428
    isDefault: true
    editable: true
```

## Importing

### Via the Grafana UI

1. Settings → Dashboards → Import.
2. Upload the JSON file (or paste its contents).
3. Pick the `VictoriaMetrics` datasource when prompted.

### Via the Grafana HTTP API

```sh
for board in dashboards/*.json; do
  curl -s \
    -H "Authorization: Bearer $GRAFANA_API_TOKEN" \
    -H "Content-Type: application/json" \
    -d "{\"dashboard\": $(cat "$board"), \"overwrite\": true}" \
    "$GRAFANA_URL/api/dashboards/db"
done
```

### Via Grafana file provisioning

Copy the JSON files into Grafana's `provisioning/dashboards/booth/`
directory and add a provider entry:

```yaml
apiVersion: 1
providers:
  - name: booth
    folder: Telephone Booth
    type: file
    options:
      path: /var/lib/grafana/provisioning/dashboards/booth
```

### The combined tabbed dashboard (`booth-combined.json`)

`booth-combined.json` uses Grafana's newer dashboard schema (v2,
`dashboard.grafana.app/v2`) so the three sections render as tabs.
The file is the bare dashboard **spec** (the v2 "JSON model"), which is
what the UI import expects. A few things to know:

- **Requires Grafana 12+** with the new dashboard layouts. Older
  Grafana versions don't understand `TabsLayout` and will reject it.
- **Import via the UI:** Dashboards → New → Import → paste the file.
- **File provisioning** wants the Kubernetes-style envelope, not the
  bare spec. Wrap it first:

  ```sh
  jq '{apiVersion:"dashboard.grafana.app/v2", kind:"Dashboard", \
       metadata:{name:"booth-combined"}, spec:.}' booth-combined.json
  ```

- **`annotations` errors on import:** the classic schema's
  `"annotations": { "list": [] }` is *invalid* in schema v2. In v2,
  annotations are a list of `AnnotationQuery` objects (already set
  correctly here). If you hit `annotations … invalid`, you're pasting
  classic JSON into the v2 path — use `booth-combined.json` as-is rather
  than copying fields from the classic files.
- **Datasource:** unlike the classic files (which hard-code the
  `VictoriaMetrics` uid), the combined dashboard exposes a **Datasource**
  dropdown (a `DatasourceVariable` for `prometheus`). Pick your
  Prometheus/VictoriaMetrics datasource there and the `$booth` selector
  and all panels follow it — no uid editing required.

## Editing

The dashboards are stored as JSON-as-code so they're reproducible
across Grafana instances. To update one:

1. Edit it in Grafana.
2. Export the JSON model (Share → Export → "Save to file").
3. Replace the corresponding `dashboards/*.json` in this repo.
4. Open a PR. CI doesn't run anything against the dashboards beyond the
   markdownlint pass on this README, so review focuses on the visual
   diff in the JSON.
