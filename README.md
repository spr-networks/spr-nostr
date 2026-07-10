# spr-nostr

Run a self-hosted [Nostr](https://nostr.com) relay as an
[SPR](https://github.com/spr-networks/super) plugin. Your Nostr clients get a
private relay on your own router — events are stored locally in an embedded
LMDB database instead of relying solely on public relays.

Unlike the other SPR plugins, **spr-nostr has no Go backend**. A single Rust
binary does everything: it runs the relay in-process via
[`nostr-relay-builder`](https://github.com/rust-nostr/nostr)'s `LocalRelay`
(with [`nostr-lmdb`](https://github.com/rust-nostr/nostr) persistence) **and**
serves the SPR plugin's REST API + iframe UI over the plugin unix socket using
`axum` on a tokio `UnixListener`. "Restart" does not fork a process — it shuts
the relay down and rebuilds it in-process from the new config.

## Features

- **In-process Nostr relay** — `nostr-relay-builder` `0.44.1` `LocalRelay` +
  `nostr-lmdb` persistence, bound to the container IP on the plugin's own
  docker bridge (`spr-nostr`), default port `7777`. Events survive container
  rebuilds (the LMDB store lives under `/state/plugins/spr-nostr/db`).
- **Copyable relay address** — `ws://<container-ip>:<port>`, ready to paste
  into your Nostr client's Relays list.
- **Access mode** — read / write / both, enforced with real
  `nostr-relay-builder` `WritePolicy` / `QueryPolicy` plugins.
- **Optional NIP-42 authentication** — require clients to authenticate before
  reading or writing.
- **NIP-11 relay information** — the plugin serves a NIP-11 Relay Information
  Document (name, description, owner pubkey, contact, supported NIPs, software,
  version) on the relay port itself, so clients can discover the relay's
  metadata.
- **Topology** — contributes the relay as a service node to SPR's topology
  view (`HasTopology` + `GET /topology`).
- **No host ports.** The relay listens on the container IP on the `spr-nostr`
  bridge; the management API is only reachable over the SPR plugin unix socket.
- **Reproducible container build** — digest-pinned base images, Rust toolchain
  pinned by version (rustup installer pinned by sha256), Ubuntu packages from a
  dated `snapshot.ubuntu.com` snapshot, `cargo build --release --locked` with a
  committed `Cargo.lock`, `SOURCE_DATE_EPOCH` honored.

## How it integrates with SPR

SPR proxies `/plugins/spr-nostr/…` to the plugin's unix socket at
`/state/plugins/spr-nostr/socket` and embeds the UI (served from the same
socket) as an iframe under **Plugins → spr-nostr**. The relay itself is only
exposed on the `spr-nostr` docker bridge; SPR policies and the `nostr` device
group decide who can reach it.

## Exposing the relay to clients

The relay binds `CONTAINER_IP:7777` (ws) on the `spr-nostr` bridge. Two ways to
let Nostr clients reach it:

1. **LAN (default).** The plugin interface carries the `lan` policy and the
   `nostr` group, so clients on your network can reach `CONTAINER_IP:7777`.
   Copy the address from the UI and add it in your Nostr app under Relays.
2. **Internet exposure (optional, documented only).** To serve roaming devices,
   add an SPR port forward: in the SPR UI, **Firewall → Port Forwarding**,
   forward a WAN TCP port to `CONTAINER_IP:7777`, and use your public IP / DDNS
   name in the relay URL. If you expose the relay publicly, turn on **Require
   client authentication (NIP-42)** in the UI so strangers can't read or write
   to it. Nostr `ws://` is unencrypted — put it behind a TLS-terminating
   reverse proxy (`wss://`) if you expose it to the internet.

The relay is standalone (no federation / outbound connections), so the plugin
is **not** granted the `wan` policy.

## Install (UI)

In the SPR UI: **Plugins → + New Plugin** and enter this repository's GitHub
URL (e.g. `https://github.com/USER/spr-nostr`). SPR clones the repo, builds the
container and starts the plugin. The `plugin.json` `NetworkCapabilities`
register the `spr-nostr` interface with the `lan` policy and the `nostr` group
automatically.

## Install (CLI)

```sh
git clone https://github.com/USER/spr-nostr
cd spr-nostr
./install.sh    # prompts for SUPERDIR and an SPR API token
```

`install.sh` writes the API token, builds and starts the container, and
registers the container IP with SPR's firewall
(`PUT /firewall/custom_interface`, policy `lan`, group `nostr`).

## API

All endpoints are served over the plugin unix socket and reachable (with SPR
auth) at `/plugins/spr-nostr/<path>`.

| Method | Path | Description |
| --- | --- | --- |
| GET | `/status` | Relay state, copyable ws:// address, host, port, mode, auth, version, engine, uptime, DB size |
| GET | `/address` | The copyable relay address: `{Address, Host, Port}` |
| GET | `/config` | Plugin configuration `{Port, Mode, RequireAuth, RelayName, RelayDescription, RelayPubkey, RelayContact}` |
| PUT | `/config` | Validate + save config, then rebuild the relay in-process |
| POST | `/restart` | Rebuild the relay in-process from the current config |
| GET | `/topology` | Topology contribution (root anchor + relay service node) |

`PUT /config` body:

```json
{
  "Port": 7777,
  "Mode": "both",
  "RequireAuth": false,
  "RelayName": "Home relay",
  "RelayDescription": "My personal Nostr relay",
  "RelayPubkey": "",
  "RelayContact": "mailto:me@example.com"
}
```

## Configuration reference

`/configs/plugins/spr-nostr/config.json` (managed via the UI / API, mode 0600):

| Field | Default | Meaning |
| --- | --- | --- |
| `Port` | `7777` | TCP port the relay listens on (container IP, `spr-nostr` bridge) |
| `Mode` | `"both"` | `read` (reject EVENT — read-only), `write` (reject REQ — write-only), or `both`. Enforced with `nostr-relay-builder` `WritePolicy` / `QueryPolicy` plugins |
| `RequireAuth` | `false` | Require NIP-42 client authentication for reading and writing (`RelayBuilder::nip42`) |
| `RelayName` | `""` | NIP-11 relay name (empty = omitted) |
| `RelayDescription` | `""` | NIP-11 description (empty = omitted) |
| `RelayPubkey` | `""` | NIP-11 owner public key, 64-char hex (empty = omitted) |
| `RelayContact` | `""` | NIP-11 owner contact, e.g. `mailto:` or URL (empty = omitted) |

### NIP-11 relay information

`nostr-relay-builder` `0.44.1` does not serve NIP-11 itself, so the plugin does
it: because `LocalRelay::take_connection` expects an already-upgraded stream (it
performs no WebSocket handshake), the plugin **fronts the relay** — it binds the
listener, and for each connection reads the request head and dispatches. A `GET`
with `Accept: application/nostr+json` (no Upgrade) gets a NIP-11 Relay
Information Document (built from the config, with permissive CORS) on the *same*
relay port; a WebSocket upgrade gets the `101` handshake written by the plugin,
after which the upgraded stream is handed to `take_connection`. This mirrors the
crate's own `examples/hyper.rs`; `LocalRelay::run()` is not used. `/status`
reports `Nip11Supported: true`. The document advertises `supported_nips`
`[1, 9, 11, 45, 77]` (plus `42` when authentication is required) — only NIPs the
relay actually implements.

## Topology

`GET /topology` returns `{"Nodes":[…],"Edges":[…]}` and the plugin sets
`HasTopology: true`, so the relay appears in SPR's router topology view. It
emits a `root` anchor node (`ConnType: "nostr"`, always online) and a
`nostr-relay` service node whose `Online` reflects the live relay state, with a
single `l1` edge from `root` to the relay. This mirrors the spr-tailscale /
spr-simplex topology contract; the SPR host merges the graph at the `root`
anchor.

## Security model

- **No published host ports**; `network_mode: host` is not used. The only
  listeners are the plugin unix socket (0770) and the relay on the container IP
  `:7777` on the dedicated `spr-nostr` bridge, gated by SPR policies/groups
  (`lan` + `nostr`). No `wan` policy — the relay makes no outbound connections.
- **No extra capabilities** (`cap_add` empty), no devices,
  `security_opt: no-new-privileges:true`.
- **Data**: the LMDB event store lives under `/state/plugins/spr-nostr/db`
  (dir 0700). The config file is mode 0600. The relay holds no secret key
  material, so there is nothing to redact on read; if a NIP-42 admin key is
  ever added it must be redacted in `/config`/`/status`.
- **Input validation**: port and mode are validated server-side before the
  relay is rebuilt; there is no shell interpolation anywhere (the relay runs
  in-process, not as a forked command line).

## Reproducible builds

All build inputs are pinned in `reproducible.env`: base images by digest, the
Rust toolchain by version (rustup installer pinned by version + sha256 for
amd64 and arm64, toolchain verified by rustup against its signed channel
manifest), Ubuntu packages from a dated `snapshot.ubuntu.com` snapshot, and the
relay's whole crate graph pinned by the committed `relay/Cargo.lock`
(`cargo build --release --locked`). The binary is stripped
(`strip = "symbols"`) and `SOURCE_DATE_EPOCH` is honored; image timestamps are
normalized by buildx `rewrite-timestamp`.

- `./build_docker_compose.sh` — reproducible local build (buildx +
  `rewrite-timestamp`, pins injected as build args)
- `./update-pins.sh` — re-resolve every pin (image digests, latest stable Rust
  toolchain version, latest rustup installer + checksums) and sync the
  Dockerfile ARG defaults

## Upstream

- [rust-nostr/nostr](https://github.com/rust-nostr/nostr) — the
  `nostr-relay-builder` and `nostr-lmdb` crates, MIT license. This plugin uses
  the unmodified published crates.
- Wishlist context: [spr-networks/super#341](https://github.com/spr-networks/super/issues/341)

## License

MIT — see [LICENSE](LICENSE).
