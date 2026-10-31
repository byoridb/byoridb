# Pinned multi-platform base images; refresh digests deliberately with runtime smoke.
FROM rust:1.90-slim-bookworm@sha256:64232e656c058f4468e8d024e990acff04f0fd5a5c0a88a574dc37773d7325c9 AS builder
RUN apt-get update && apt-get install -y --no-install-recommends \
    protobuf-compiler build-essential pkg-config libssl-dev git \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /usr/src/byoridb
COPY . .
ARG CARGO_BUILD_JOBS=2
# No best-effort/dummy build: all shipped executables use the committed lockfile.
RUN cargo build --locked --release -j ${CARGO_BUILD_JOBS} \
    -p byoridb --bin byoridb-server --bin byoridb-backup \
    -p byoridb-bulkloader --bin byoridb-bulkloader \
    -p byoridb-client --bin byoridb-cli

FROM debian:bookworm-slim@sha256:99c0b25c46d0214d5a2091092e7908907dcaecd6a962cde0ac700acee222b6f3
RUN apt-get update && apt-get install -y --no-install-recommends libssl3 ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 byoridb \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin byoridb \
    && mkdir -p /app/data && chown 10001:10001 /app/data && chmod 700 /app/data
WORKDIR /app
COPY --from=builder /usr/src/byoridb/target/release/byoridb-server /usr/local/bin/byoridb-server
COPY --from=builder /usr/src/byoridb/target/release/byoridb-backup /usr/local/bin/byoridb-backup
COPY --from=builder /usr/src/byoridb/target/release/byoridb-bulkloader /usr/local/bin/byoridb-bulkloader
COPY --from=builder /usr/src/byoridb/target/release/byoridb-cli /usr/local/bin/byoridb-cli
USER 10001:10001
EXPOSE 9669 19669
STOPSIGNAL SIGTERM
CMD ["byoridb-server"]
