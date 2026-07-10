#!/usr/bin/env bash
# update-pins.sh — re-resolve every pin, rewrite reproducible.env, and sync the
# matching `ARG <KEY>=` defaults / `# syntax=` line in the Dockerfile(s). Read-only
# lookups (registry manifest inspect, static.rust-lang.org). Review with
# `git diff`.
# Tip: `docker login` first to avoid Docker Hub's unauthenticated pull-rate-limit (429).
set -euo pipefail
cd "$(dirname "$0")"

# ---- tracked refs (edit to bump, then re-run) ----
UBUNTU_TAG=ubuntu:24.04
ALPINE_TAG=alpine:latest
NODE_TAG=node:18
DOCKERFILE_TAG=docker/dockerfile:1
BUILDKIT_TAG=moby/buildkit:buildx-stable-1
CONTAINER_TEMPLATE_TAG=ghcr.io/spr-networks/container_template:latest
# nostr-relay-builder is pinned in relay/Cargo.toml + relay/Cargo.lock; this is
# just the recorded copy. Bump it with `cargo update -p nostr-relay-builder` and
# edit relay/Cargo.toml, then re-run.
NOSTR_RELAY_BUILDER_VERSION="${NOSTR_RELAY_BUILDER_VERSION:-$(grep -E '^NOSTR_RELAY_BUILDER_VERSION=' reproducible.env | cut -d= -f2)}"

mdigest() { docker buildx imagetools inspect "$1" --format '{{.Manifest.Digest}}'; }

echo "Resolving pins..." >&2
UBUNTU_REF="${UBUNTU_TAG}@$(mdigest "$UBUNTU_TAG")"
ALPINE_REF="${ALPINE_TAG%%:*}@$(mdigest "$ALPINE_TAG")"
NODE_REF="${NODE_TAG}@$(mdigest "$NODE_TAG")"
DOCKERFILE_SYNTAX="${DOCKERFILE_TAG}@$(mdigest "$DOCKERFILE_TAG")"
BUILDKIT_REF="${BUILDKIT_TAG}@$(mdigest "$BUILDKIT_TAG")"
CONTAINER_TEMPLATE_REF="${CONTAINER_TEMPLATE_TAG%:*}@$(mdigest "$CONTAINER_TEMPLATE_TAG")"
UBUNTU_SNAPSHOT="${UBUNTU_SNAPSHOT:-$(grep -E '^UBUNTU_SNAPSHOT=' reproducible.env | cut -d= -f2)}"
code=$(curl -fsS -o /dev/null -w '%{http_code}' "https://snapshot.ubuntu.com/ubuntu/${UBUNTU_SNAPSHOT}/dists/noble/InRelease" || true)
[ "$code" = "200" ] || { echo "snapshot ${UBUNTU_SNAPSHOT} not valid (HTTP $code)" >&2; exit 1; }

# Latest stable Rust toolchain version (channel manifest on static.rust-lang.org).
echo "Resolving Rust toolchain version..." >&2
RUST_VERSION=$(curl -fsSL "https://static.rust-lang.org/dist/channel-rust-stable.toml" \
  | grep -m1 -A2 '^\[pkg.rust\]' | grep -m1 'version' | sed 's/.*"\([0-9][0-9.]*\).*/\1/')
[ -n "$RUST_VERSION" ] || { echo "could not resolve RUST_VERSION" >&2; exit 1; }

# rustup installer version + per-arch sha256 (published next to each binary).
echo "Resolving rustup installer + checksums..." >&2
RUSTUP_VERSION=$(curl -fsSL "https://static.rust-lang.org/rustup/release-stable.toml" \
  | grep -m1 "^version" | sed "s/.*'\(.*\)'.*/\1/")
sha_rustup() {
  curl -fsSL "https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/$1/rustup-init.sha256" | awk '{print $1}'
}
RUSTUP_SHA256_AMD64=$(sha_rustup "x86_64-unknown-linux-gnu")
RUSTUP_SHA256_ARM64=$(sha_rustup "aarch64-unknown-linux-gnu")

echo "Writing reproducible.env" >&2
cat > reproducible.env <<EOF
# Pinned build inputs for build_docker_compose.sh and CI. Regenerate with ./update-pins.sh.
UBUNTU_REF=${UBUNTU_REF}
ALPINE_REF=${ALPINE_REF}
NODE_REF=${NODE_REF}
DOCKERFILE_SYNTAX=${DOCKERFILE_SYNTAX}
BUILDKIT_REF=${BUILDKIT_REF}
CONTAINER_TEMPLATE_REF=${CONTAINER_TEMPLATE_REF}
UBUNTU_SNAPSHOT=${UBUNTU_SNAPSHOT}
# Rust toolchain: installed by rustup (pinned installer version + sha256 per
# arch) then \`rustup default \${RUST_VERSION}\`. The toolchain itself is verified
# by rustup against its signed channel manifest.
RUST_VERSION=${RUST_VERSION}
RUSTUP_VERSION=${RUSTUP_VERSION}
RUSTUP_SHA256_AMD64=${RUSTUP_SHA256_AMD64}
RUSTUP_SHA256_ARM64=${RUSTUP_SHA256_ARM64}
# Relay engine version (pinned exactly in relay/Cargo.toml + relay/Cargo.lock).
NOSTR_RELAY_BUILDER_VERSION=${NOSTR_RELAY_BUILDER_VERSION}
EOF

echo "Syncing Dockerfile ARG defaults + # syntax= lines" >&2
DOCKERFILES=()
while IFS= read -r f; do DOCKERFILES+=("$f"); done < <(find . -path ./node_modules -prune -o -type f -name 'Dockerfile*' -print)
replace_line() {  # <file> <sed-pattern> <new-line>  (sed: no @/$ interpolation)
  local f="$1" pat="$2" new="$3" tmp; tmp=$(mktemp)
  sed "s|${pat}|${new}|" "$f" > "$tmp" && mv "$tmp" "$f"
}
while IFS='=' read -r k v; do
  case "$k" in ''|\#*) continue;; esac
  for f in "${DOCKERFILES[@]}"; do
    if [ "$k" = "DOCKERFILE_SYNTAX" ]; then
      replace_line "$f" '^# syntax=.*' "# syntax=${v}"
    else
      replace_line "$f" "^ARG ${k}=.*" "ARG ${k}=${v}"
    fi
  done
done < reproducible.env

echo "Done. Review with: git diff" >&2
