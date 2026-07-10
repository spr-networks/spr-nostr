#!/bin/bash
set -a
. /configs/base/config.sh
if [ -f /configs/spr-nostr/config.sh ]; then
    . /configs/spr-nostr/config.sh
fi
set +a

# Relay data (the LMDB event store) lives on the bind mount under the plugin
# state dir so events survive container rebuilds — keep it owner-only.
mkdir -p /state/plugins/spr-nostr/db
chmod 700 /state/plugins/spr-nostr/db

# The single Rust binary IS the backend: it runs the Nostr relay in-process
# (nostr-relay-builder LocalRelay + nostr-lmdb) AND serves the SPR plugin API
# and iframe UI over the unix socket. No separate daemon, no Go shim.
exec /spr-nostr
