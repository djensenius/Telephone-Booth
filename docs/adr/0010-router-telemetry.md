# ADR 0010 — GL.iNet router telemetry

**Status:** accepted.

## Context

The GL.iNet E5800 powers equipment inside the installation. Its battery and
charger state are operationally important: losing external power while the
internal load continues to run can drain the router and disconnect the booth.

The router exposes standard Linux power-supply and thermal interfaces, plus
fast-charge and charging-state fields through its `mcu` ubus object. Its
existing OpenWrt Lua exporter does not include collectors for those sources.

Prometheus runs on a Tailscale-connected server and already scrapes the router
every 15 seconds. The Operator backend is outside the tailnet, so it cannot
query that Prometheus endpoint directly.

## Decision

- Install a custom `prometheus-node-exporter-lua` collector on the router.
  It exports battery, charger, and all valid thermal-zone readings.
- Read standard values from `/sys/class/power_supply` and
  `/sys/class/thermal`. Use `ubus call mcu status` only for fields unavailable
  through sysfs, including fast-charge and charging status.
- Filter unreadable zones and known sentinel readings before publication.
- Keep historical samples in Prometheus. The router's existing scrape job
  picks up new metrics automatically.
- Run a separate OpenWrt `procd` service that pushes the latest structured
  snapshot to the Operator every 30 seconds over HTTPS.
- Authenticate that push with a telemetry-scoped Operator token stored in a
  mode-0600 curl header file.
- Let the Operator query historical data through Grafana's authenticated
  datasource proxy. Do not expose Prometheus port 9090 publicly.
- Keep the installation and deployment assets under `packaging/openwrt/`.

## Consequences

**Good:**

- Historical graphs begin as soon as the collector is installed, independent
  of Operator availability.
- The Operator and its clients receive a stable, structured current snapshot
  without joining the tailnet.
- Prometheus remains private, and browser/mobile clients never receive
  Grafana or Prometheus credentials.
- Thermal-zone cardinality is bounded by the router's fixed hardware sensors.

**Trade-offs:**

- The router runs a second small service for the Operator push.
- Grafana becomes the authenticated bridge for historical Operator queries.
- Custom files may need reinstalling after a router firmware upgrade if
  OpenWrt does not restore the paths listed in `/etc/sysupgrade.conf`.
- The latest Operator state and Prometheus history use separate delivery
  paths, so their timestamps can differ by one scrape or push interval.
