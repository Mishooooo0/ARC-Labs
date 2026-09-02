# ARC-LABS in a container: the headless shell, served to a browser.
#
# There is no desktop shell here and no webview — a container has no display, so
# `arc-labs doctor` reports the desktop shell as unavailable rather than looking
# for a webkit that will never exist. The image is the server, the UI bundle and
# nothing else.
#
# The vault is a bind mount. Constraint 1 says files are the source of truth, so
# the image deliberately holds no vault data: delete the container and the notes
# are exactly where they were.

# ── 1. UI bundle ─────────────────────────────────────────────────────────────
FROM node:24-bookworm-slim AS ui
WORKDIR /build/ui
COPY ui/package.json ui/package-lock.json ./
RUN npm ci
COPY ui/ ./
RUN npm run build

# ── 2. Rust binary ───────────────────────────────────────────────────────────
FROM rust:1-bookworm AS server
WORKDIR /build

# Manifests first so the dependency layer survives a source-only change.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/arc-labs-core/Cargo.toml       crates/arc-labs-core/
COPY crates/arc-labs-api/Cargo.toml        crates/arc-labs-api/
COPY crates/arc-labs-bootstrap/Cargo.toml  crates/arc-labs-bootstrap/
COPY crates/arc-labs-server/Cargo.toml     crates/arc-labs-server/
COPY crates/arc-labs-cli/Cargo.toml        crates/arc-labs-cli/
COPY xtask/Cargo.toml                      xtask/
# arc-labs-app is a workspace member but needs Tauri's system libraries, which
# a headless image has no use for. Dropping it from the members list here keeps
# the image small and the build honest about what it actually ships.
RUN sed -i '/arc-labs-app/d' Cargo.toml \
 && mkdir -p crates/arc-labs-core/src crates/arc-labs-api/src crates/arc-labs-bootstrap/src \
             crates/arc-labs-server/src crates/arc-labs-cli/src xtask/src \
 && echo "" > crates/arc-labs-core/src/lib.rs \
 && echo "" > crates/arc-labs-api/src/lib.rs \
 && echo "" > crates/arc-labs-bootstrap/src/lib.rs \
 && echo "" > crates/arc-labs-server/src/lib.rs \
 && echo "fn main() {}" > crates/arc-labs-cli/src/main.rs \
 && echo "fn main() {}" > xtask/src/main.rs \
 && cargo build --release -p arc-labs-cli 2>/dev/null || true

COPY crates/ crates/
COPY xtask/ xtask/
RUN sed -i '/arc-labs-app/d' Cargo.toml \
 && rm -rf crates/arc-labs-app \
 && touch crates/*/src/lib.rs crates/arc-labs-cli/src/main.rs \
 && cargo build --release -p arc-labs-cli

# ── 3. Runtime ───────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

# ca-certificates only so a configured remote Ollama endpoint can be reached over
# TLS in Phase 5. Nothing in the image initiates a connection on its own.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*

# Never root. A container that can write anywhere as root is a container that can
# rewrite the vault it was handed.
RUN useradd --system --create-home --uid 10001 arc
USER arc
WORKDIR /home/arc

COPY --from=server --chown=arc:arc /build/target/release/arc-labs /usr/local/bin/arc-labs
COPY --from=ui     --chown=arc:arc /build/ui/dist ./ui

ENV ARC_LABS_VAULT=/vault \
    ARC_LABS_UI_DIR=/home/arc/ui \
    ARC_LABS_IN_CONTAINER=1 \
    ARC_LABS_CONFIG=/home/arc/.config/arc-labs/config.toml

VOLUME ["/vault"]
EXPOSE 7777

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD curl -fsS http://127.0.0.1:7777/api/status || exit 1

# 0.0.0.0 is required for the port to be reachable from outside the container,
# and the server generates and prints a token because the bind is not loopback.
# Read the token from the container logs.
ENTRYPOINT ["arc-labs"]
CMD ["serve", "--host", "0.0.0.0", "--port", "7777"]
