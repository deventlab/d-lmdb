# syntax=docker/dockerfile:1
#
# Multi-stage build for d-lmdb-server, layered via cargo-chef so dependency
# compilation is cached separately from application code changes.

FROM rust:1.89-slim-bookworm AS chef
# protobuf-compiler: d-engine-proto's build.rs shells out to `protoc`.
RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release -p d-lmdb-server

FROM debian:bookworm-slim AS runtime

# setpriv (util-linux) replaces gosu — avoids 68 Go 1.19 stdlib CVEs.
# perl-base + tar purged: not needed at runtime, bring CVEs.
RUN apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends ca-certificates \
    && dpkg --force-remove-essential --force-depends --purge perl-base tar \
    && rm -rf /var/lib/apt/lists/*

# Fixed UID/GID (not just UID) — docker-compose bind-mounts a host directory
# onto /data, and the host side must be chowned to this same 1000:1000 ahead
# of time. See docker-entrypoint.sh for the runtime half of this.
RUN groupadd -g 1000 appuser \
    && useradd -u 1000 -g appuser -m -s /usr/sbin/nologin appuser \
    && mkdir -p /data \
    && chown appuser:appuser /data

COPY --from=builder /app/target/release/d-lmdb-server /usr/local/bin/d-lmdb-server
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

# 9081: Raft inter-node RPC. 8080: HTTP client API. Both configurable via the
# mounted config's [cluster]/[http] listen_address — EXPOSE here is documentation,
# not enforcement.
EXPOSE 9081 8080

# start-period=10s: measured 5 cold starts locally (serve → first successful
# healthcheck), worst case 863ms (first-ever exec, page cache cold — the
# closest local proxy for a freshly started container). 10s leaves ~10x margin
# for slower/loaded hosts without masking a real startup failure for too long.
HEALTHCHECK --interval=5s --timeout=3s --start-period=10s --retries=3 \
    CMD ["d-lmdb-server", "healthcheck"]

ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["d-lmdb-server", "serve"]
