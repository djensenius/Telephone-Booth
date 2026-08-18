#!/bin/sh

set -eu

TARGET="${1:-root@telephone-router.barking-solfege.ts.net}"
SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
FILES_DIR="${SCRIPT_DIR}/files"
STAGE="/tmp/telephone-booth-router-telemetry-install.$$"

tar -C "$FILES_DIR" -cf - . | ssh "$TARGET" "mkdir -p '$STAGE' && tar -C '$STAGE' -xf -"

ssh "$TARGET" "STAGE='$STAGE' sh -s" <<'REMOTE'
set -eu

mkdir -p \
  /usr/lib/lua/prometheus-collectors \
  /usr/bin \
  /etc/init.d \
  /etc/config \
  /etc/telephone-booth-router-telemetry

cp "$STAGE/usr/lib/lua/telephone_booth_router_telemetry.lua" \
  /usr/lib/lua/telephone_booth_router_telemetry.lua
cp "$STAGE/usr/lib/lua/prometheus-collectors/glinet_power.lua" \
  /usr/lib/lua/prometheus-collectors/glinet_power.lua
cp "$STAGE/usr/bin/telephone-booth-router-telemetry-snapshot" \
  /usr/bin/telephone-booth-router-telemetry-snapshot
cp "$STAGE/usr/bin/telephone-booth-router-telemetry-push" \
  /usr/bin/telephone-booth-router-telemetry-push
cp "$STAGE/etc/init.d/telephone-booth-router-telemetry" \
  /etc/init.d/telephone-booth-router-telemetry

if [ ! -e /etc/config/telephone-booth-router-telemetry ]; then
  cp "$STAGE/etc/config/telephone-booth-router-telemetry" \
    /etc/config/telephone-booth-router-telemetry
fi

cp "$STAGE/etc/telephone-booth-router-telemetry/operator-auth-header.example" \
  /etc/telephone-booth-router-telemetry/operator-auth-header.example

chmod 755 \
  /usr/bin/telephone-booth-router-telemetry-snapshot \
  /usr/bin/telephone-booth-router-telemetry-push \
  /etc/init.d/telephone-booth-router-telemetry
chmod 644 \
  /usr/lib/lua/telephone_booth_router_telemetry.lua \
  /usr/lib/lua/prometheus-collectors/glinet_power.lua \
  /etc/telephone-booth-router-telemetry/operator-auth-header.example
[ ! -e /etc/telephone-booth-router-telemetry/operator-auth-header ] || \
  chmod 600 /etc/telephone-booth-router-telemetry/operator-auth-header

touch /etc/sysupgrade.conf
for path in \
  /usr/lib/lua/telephone_booth_router_telemetry.lua \
  /usr/lib/lua/prometheus-collectors/glinet_power.lua \
  /usr/bin/telephone-booth-router-telemetry-snapshot \
  /usr/bin/telephone-booth-router-telemetry-push \
  /etc/init.d/telephone-booth-router-telemetry \
  /etc/rc.d/S95telephone-booth-router-telemetry \
  /etc/config/telephone-booth-router-telemetry \
  /etc/telephone-booth-router-telemetry/operator-auth-header
do
  grep -qxF "$path" /etc/sysupgrade.conf || echo "$path" >>/etc/sysupgrade.conf
done

/etc/init.d/prometheus-node-exporter-lua restart
/etc/init.d/telephone-booth-router-telemetry restart
rm -rf "$STAGE"
REMOTE

printf '%s\n' "Installed router telemetry files on ${TARGET}."
printf '%s\n' "The Prometheus collector is active; the Operator pusher remains disabled."
