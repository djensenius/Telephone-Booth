# Grafana dashboards

JSON dashboards for the Telephone Booth observability stack.
See [`docs/observability.md`](../docs/observability.md) for the full
data flow and the metric catalog.

## Layout

| File                          | Title                          | Focus                                                                 |
| ----------------------------- | ------------------------------ | --------------------------------------------------------------------- |
| `booth-overview.json`         | Booth — Overview               | CPU temp, fan command, load, memory, uptime, network, pickups/day. |
| `booth-call-activity.json`    | Booth — Pickup activity        | Selected-range pickup stats plus outcome, digit, recording, and upload-failure diagnostics. |
| `booth-audio.json`            | Booth — Audio & operator HTTP  | Input/output dBFS, operator request rate, p95 latency, dropped events. |
| `booth-thermals.json`         | Booth — Thermals               | Pi, router, modem, fan, outdoor weather, and repeated sensor charts.  |
| `booth-combined.json`         | Telephone Booth (tabbed)       | Overview, pickup activity, and audio in three tabs (Grafana 12+, schema v2). |

The four single-focus dashboards use the classic dashboard schema
(`schemaVersion: 39`) and import into any modern Grafana.
`booth-combined.json` reorganises the Overview, Pickup activity, and Audio
panels into one dashboard with a tab per section, using Grafana's newer
dashboard schema (`dashboard.grafana.app/v2`). Use it if you prefer one
dashboard with tabs; keep the classic files if you run an older Grafana or
provision dashboards individually. Thermals remains a dedicated classic
dashboard.

The overview dashboard labels `booth_calls_started_total` as pickups.
The Pickup activity dashboard and matching combined tab headline show
selected-range totals for pickups, no selection, wrong numbers,
messages left, messages listened to, and instructions heard. The playback
metrics count starts, not completions. The underlying Prometheus series names
and Operator API analytics may still use interaction terminology; Grafana only
relabels the start-time metric for dashboard UI consistency.

The **Pickups** panel is start-time based. The no-selection,
messages-left, and pickup-outcome panels use `booth_calls_total`, which increments
when a pickup ends. They therefore use end time and may not reconcile
with the pickup start cohort at a selected-range boundary. The Operator API
remains the source for cohort-consistent pickup analytics.

All of them use a `$booth` template variable populated from the
`booth_id` label, so they work out of the box for single- and
multi-booth deployments.

### Thermals variables

`booth-thermals.json` adds these selectors:

- `$datasource` selects a Prometheus-compatible datasource. Its checked-in
  default is the live `prometheus` datasource with uid `eet73sk9rhts0f`,
  but the dropdown keeps imports portable to other Grafana instances.
- `$booth` filters Pi temperature series by `booth_id`.
- `$router` selects one or more `instance` labels from the fixed
  `job="TelephoneRouter"` scrape job.
- `$thermal_sensor` selects thermal-zone `name` labels. It is multi-value,
  defaults to **All**, and drives the repeated per-sensor panels in the
  collapsible **Individual router thermal zones** row. With **All** selected,
  Grafana creates one chart for each of the 24 currently exported sensors.

The dashboard defaults to a rolling 24-hour view. Its combined temperature
chart overlays Pi CPU, router battery, the hottest of the router's four modem
zones, outdoor air temperature, and apparent temperature. Separate panels
retain each source's detail, while the weather row adds current condition,
humidity, cloud cover, fetch health, and freshness. Fan PWM is a commanded
duty ratio; the RPM panel remains empty when no tachometer wire is connected.

The router exporter does not publish `booth_id`, so `$booth` and `$router`
intentionally filter their respective Pi and router series independently.
The booth, router, and thermal-sensor selectors include an **All** option and
default to it.

Outdoor values are modeled regional context rather than measurements inside
the enclosure. The dashboard includes the required
[Open-Meteo attribution](https://open-meteo.com/) as a dashboard link.

## Datasource

The Overview, Pickup activity, and Audio classic dashboards expect a
Prometheus-compatible datasource named `VictoriaMetrics` with uid
`VictoriaMetrics`. The Thermals dashboard instead uses its `$datasource`
variable described above; the combined dashboard also has a datasource
dropdown. Adjust the hard-coded classic files or select the appropriate
variable when your datasource uses another uid.

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
2. Upload the JSON file (or paste its contents). Use
   `booth-thermals.json` for the dedicated Thermals dashboard.
3. Pick the `VictoriaMetrics` datasource when prompted by the older classic
   dashboards. For Thermals, verify the **Datasource** dropdown instead.

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

Copy the classic JSON files, including `booth-thermals.json`, into Grafana's
`provisioning/dashboards/booth/` directory and add a provider entry:

```yaml
apiVersion: 1
providers:
  - name: booth
    folder: Telephone Booth
    type: file
    options:
      path: /var/lib/grafana/provisioning/dashboards/booth
```

For production file provisioning, keep the Thermals datasource uid
`eet73sk9rhts0f` available. On another Grafana instance, select and save a
different Prometheus-compatible datasource after importing, or change the
`datasource` variable's default before provisioning.

### The combined tabbed dashboard (`booth-combined.json`)

`booth-combined.json` uses Grafana's newer dashboard schema (v2,
`dashboard.grafana.app/v2`) so the Overview, Pickup activity, and
Audio sections render as tabs.
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
