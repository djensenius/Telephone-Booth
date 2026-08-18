# GL.iNet router telemetry

This package adds battery, charger, and thermal-zone telemetry to the
GL.iNet E5800 used by the Telephone Booth installation.

The existing `prometheus-node-exporter-lua` process automatically loads the
collector from `/usr/lib/lua/prometheus-collectors/glinet_power.lua`. The
Prometheus server already scrapes the router every 15 seconds as
`job="TelephoneRouter"`.

## Install

From this repository:

```sh
./packaging/openwrt/install.sh
```

The installer restarts the Lua exporter and adds the installed paths to
`/etc/sysupgrade.conf`, including the service's boot-time enablement symlink.
Re-run it after a firmware update if the firmware does not restore custom
files. It installs only an example authorization header; the live token file
does not exist until it is explicitly provisioned.

## Prometheus metrics

The collector exports:

- Battery presence, charge percentage, temperature, voltage, signed current,
  health, technology, kernel cycle count, MCU charge count, and abnormal state.
- Charger presence, online state, fast-charge state, charging status, USB
  type, manufacturer, model, charge type, and configured voltage/current limits.
- Every readable thermal zone as
  `glinet_thermal_temperature_celsius{name="...",zone="..."}`.

Known invalid thermal sentinels, including `-273°C`, `-40.96°C`, and the
synthetic `zeroc=0°C` zone, are omitted.

Verify the collector directly:

```sh
curl http://telephone-router.barking-solfege.ts.net:9100/metrics |
  grep '^glinet_'
```

Verify Prometheus ingestion:

```sh
curl --get http://server.barking-solfege.ts.net:9090/api/v1/query \
  --data-urlencode 'query=glinet_battery_charge_percent{job="TelephoneRouter"}'
```

## Operator push

The pusher sends the latest structured snapshot to Operator every 30 seconds.
Historical samples remain in Prometheus.

Create a `telemetry`-scoped Operator API token bound to the
`booth-01/router` telemetry source. Then configure the router:

```sh
ssh root@telephone-router.barking-solfege.ts.net

uci set telephone-booth-router-telemetry.main.operator_url=\
'https://operator.example.com/v1/system/components/current'
uci set telephone-booth-router-telemetry.main.enabled='1'
uci commit telephone-booth-router-telemetry

printf 'Authorization: Bearer %s\n' 'tb_replace_me' \
  >/etc/telephone-booth-router-telemetry/operator-auth-header
chmod 600 /etc/telephone-booth-router-telemetry/operator-auth-header

/etc/init.d/telephone-booth-router-telemetry enable
/etc/init.d/telephone-booth-router-telemetry restart
```

The token is read by `curl` from the protected header file, so it does not
appear in the process command line.
