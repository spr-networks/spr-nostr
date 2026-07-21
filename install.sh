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

KRUN_MAC="02:53:50:52:4b:0c"
PLUGIN_INTERFACE="spr-nostr"
curl --fail-with-body --silent --show-error "http://127.0.0.1/device?identity=${KRUN_MAC}" \
  -H "Authorization: Bearer ${SPR_API_TOKEN}" -H "Content-Type: application/json" \
  -X PUT --data-raw "{\"MAC\":\"${KRUN_MAC}\",\"Name\":\"spr-nostr\",\"Policies\":[\"wan\",\"dns\"],\"Groups\":[\"nostr\"]}" >/dev/null
if ! sudo nft get element inet filter dhcp_access "{ \"${PLUGIN_INTERFACE}\" . ${KRUN_MAC} }" >/dev/null 2>&1; then
  sudo nft add element inet filter dhcp_access "{ \"${PLUGIN_INTERFACE}\" . ${KRUN_MAC} : accept }"
fi

docker compose -f docker-compose-kvm.yml build
docker compose -f docker-compose-kvm.yml up -d

CONTAINER_IP=
for _ in $(seq 1 30); do
  CONTAINER_IP="$(jq -r --arg mac "$KRUN_MAC" '.[$mac].RecentIP // empty' "$SUPERDIR/state/public/devices-public.json")"
  [ -n "$CONTAINER_IP" ] && break
  sleep 1
done
[ -n "$CONTAINER_IP" ] || { echo "spr-nostr did not obtain an SPR DHCP lease" >&2; exit 1; }
API=127.0.0.1

# Register the plugin's bridge interface with the SPR firewall:
# the nostr group lets LAN Nostr clients reach the relay on ${CONTAINER_IP}:7777,
# and the wan + dns policies allow the plugin outbound name resolution and
# connectivity.
curl "http://${API}/firewall/custom_interface" \
-H "Authorization: Bearer ${SPR_API_TOKEN}" \
-X 'PUT' \
--data-raw "{\"SrcIP\":\"${CONTAINER_IP}\",\"Interface\":\"${PLUGIN_INTERFACE}\",\"Policies\":[\"wan\",\"dns\"],\"Groups\":[\"nostr\"]}"

docker compose -f docker-compose-kvm.yml restart

echo ""
echo "spr-nostr is up. Open Plugins -> spr-nostr in the SPR UI to copy your"
echo "relay address (ws://${CONTAINER_IP}:7777) into your Nostr client under"
echo "Relays."
