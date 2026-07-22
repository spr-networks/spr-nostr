# syntax=docker/dockerfile:1@sha256:87999aa3d42bdc6bea60565083ee17e86d1f3339802f543c0d03998580f9cb89
ARG ALPINE_REF=alpine@sha256:28bd5fe8b56d1bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b
ARG UBUNTU_REF=ubuntu:24.04@sha256:4fbb8e6a8395de5a7550b33509421a2bafbc0aab6c06ba2cef9ebffbc7092d90
ARG NODE_REF=node:18@sha256:c6ae79e38498325db67193d391e6ec1d224d96c693a8a4d943498556716d3783
ARG CONTAINER_TEMPLATE_REF=ghcr.io/spr-networks/container_template@sha256:869ada7b121e9a0c552674042d32e801da3c4d04145638d9e722918c6377e65f
ARG SPR_KRUN_PLUGIN_REF=ghcr.io/spr-networks/spr-krun-plugin:latest
ARG SOURCE_DATE_EPOCH

FROM ${ALPINE_REF} AS cacerts

# ---- Rust builder: the single plugin binary (relay + API + UI server) ----
FROM ${UBUNTU_REF} AS builder
ENV DEBIAN_FRONTEND=noninteractive
ARG UBUNTU_SNAPSHOT=20260601T000000Z
# Rust toolchain pinned by version; rustup-init pinned by version + sha256.
ARG RUST_VERSION=1.97.0
ARG RUSTUP_VERSION=1.29.0
ARG RUSTUP_SHA256_AMD64=4acc9acc76d5079515b46346a485974457b5a79893cfb01112423c89aeb5aa10
ARG RUSTUP_SHA256_ARM64=9732d6c5e2a098d3521fca8145d826ae0aaa067ef2385ead08e6feac88fa5792
ARG TARGETARCH
ARG SOURCE_DATE_EPOCH
COPY --from=cacerts /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
RUN set -eux; \
    printf 'Types: deb\nURIs: https://snapshot.ubuntu.com/ubuntu/%s\nSuites: noble noble-updates noble-security\nComponents: main restricted universe multiverse\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n' "${UBUNTU_SNAPSHOT}" > /etc/apt/sources.list.d/ubuntu.sources; \
    printf 'APT::Install-Recommends "false";\nAcquire::Check-Valid-Until "false";\n' > /etc/apt/apt.conf.d/99reproducible
# build-essential provides the C toolchain nostr-lmdb / secp256k1-sys compile
# against (LMDB + libsecp256k1 are built from vendored C sources by the `cc`
# crate). curl fetches the pinned rustup installer.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl build-essential pkg-config && rm -rf /var/lib/apt/lists/* /var/log/* /var/cache/ldconfig/aux-cache
# Install rustup (pinned installer, verified by sha256) and the pinned toolchain.
RUN set -eux; \
    case "${TARGETARCH}" in \
      amd64) RUST_HOST=x86_64-unknown-linux-gnu; RUSTUP_SHA256="${RUSTUP_SHA256_AMD64}";; \
      arm64) RUST_HOST=aarch64-unknown-linux-gnu; RUSTUP_SHA256="${RUSTUP_SHA256_ARM64}";; \
      *) echo "unsupported TARGETARCH=${TARGETARCH}" >&2; exit 1;; \
    esac; \
    curl -fsSL -o rustup-init "https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/${RUST_HOST}/rustup-init"; \
    echo "${RUSTUP_SHA256}  rustup-init" | sha256sum -c -; \
    chmod +x rustup-init; \
    ./rustup-init -y --no-modify-path --profile minimal --default-toolchain "${RUST_VERSION}" --default-host "${RUST_HOST}"; \
    rm rustup-init
ENV PATH="/root/.cargo/bin:${PATH}"
WORKDIR /relay
COPY relay/ /relay/
# tmpfs the registry + target (like the Go stage's tmpfs GOPATH); copy the
# stripped binary out before the mounts are torn down. --locked pins every
# transitive crate to the committed Cargo.lock.
RUN --mount=type=tmpfs,target=/root/.cargo/registry \
    --mount=type=tmpfs,target=/relay/target \
    cargo build --release --locked && cp /relay/target/release/spr-nostr /spr-nostr

# ---- Node builder: the React iframe UI ----
FROM ${NODE_REF} AS builder-ui
WORKDIR /app
COPY frontend ./
RUN --mount=type=tmpfs,target=/root/.cache \
    --mount=type=tmpfs,target=/app/node_modules \
    yarn install --frozen-lockfile --network-timeout 86400000 && yarn run bundle

# ---- Runtime ----
FROM ${SPR_KRUN_PLUGIN_REF}
# The binary is glibc-dynamic (LMDB + secp256k1 are statically linked into it),
# so the container_template base already provides everything it needs.
COPY scripts /scripts/
COPY --from=builder /spr-nostr /spr-nostr
COPY --from=builder-ui /app/build/ /ui/

CMD ["/scripts/startup.sh"]
