# Grafana dashboards

JSON dashboards for the Telephone Booth observability stack.
See [`docs/observability.md`](../docs/observability.md) for the full
data flow and the metric catalog.

## Layout

| File                          | Title                          | Focus                                                                 |
| ----------------------------- | ------------------------------ | --------------------------------------------------------------------- |
| `booth-overview.json`         | Booth — Overview               | CPU temp, load, memory, uptime, network throughput, calls/sec.        |
| `booth-call-activity.json`    | Booth — Call activity          | Calls per outcome, dialed digit histogram, recording + upload timing. |
| `booth-audio.json`            | Booth — Audio & operator HTTP  | Input/output dBFS, operator request rate, p95 latency, dropped events.|

All three dashboards use a `$booth` template variable populated from the
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

## Editing

The dashboards are stored as JSON-as-code so they're reproducible
across Grafana instances. To update one:

1. Edit it in Grafana.
2. Export the JSON model (Share → Export → "Save to file").
3. Replace the corresponding `dashboards/*.json` in this repo.
4. Open a PR. CI doesn't run anything against the dashboards beyond the
   markdownlint pass on this README, so review focuses on the visual
   diff in the JSON.
