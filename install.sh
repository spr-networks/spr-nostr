#!/bin/bash
# Command line install alternative to the UI (+ New Plugin)
echo "Please enter your SPR path (/home/spr/super/)"
read -r SUPERDIR

if [ -z "$SUPERDIR" ]; then
    SUPERDIR="/home/spr/super/"
fi

export SUPERDIR

echo "Please enter your SPR API token:"
read -r SPR_API_TOKEN

if [ -z "$SPR_API_TOKEN" ]; then
  echo "need api token, generate one on the auth keys page"
  exit 1
fi

mkdir -p "$SUPERDIR/configs/plugins/spr-nostr"

# Token used by SPR to authorize the plugin (InstallTokenPath)
printf '%s' "$SPR_API_TOKEN" > "$SUPERDIR/configs/plugins/spr-nostr/api-token"
chmod 600 "$SUPERDIR/configs/plugins/spr-nostr/api-token"

docker compose build
docker compose up -d

CONTAINER_IP=$(docker inspect --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "spr-nostr")
API=127.0.0.1

# Register the plugin's bridge interface with the SPR firewall:
# the nostr group + lan policy let LAN Nostr clients reach the relay on
# ${CONTAINER_IP}:7777. The relay is standalone (no federation/outbound), so
# it is NOT granted the wan policy.
curl "http://${API}/firewall/custom_interface" \
-H "Authorization: Bearer ${SPR_API_TOKEN}" \
-X 'PUT' \
--data-raw "{\"SrcIP\":\"${CONTAINER_IP}\",\"Interface\":\"spr-nostr\",\"Policies\":[\"lan\"],\"Groups\":[\"nostr\"]}"

docker compose restart

echo ""
echo "spr-nostr is up. Open Plugins -> spr-nostr in the SPR UI to copy your"
echo "relay address (ws://${CONTAINER_IP}:7777) into your Nostr client under"
echo "Relays."
