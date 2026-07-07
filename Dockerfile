# Build Stage
FROM rust:1.90-slim-bookworm AS builder

# Build dependencies. The KV store is now pure-Rust (redb), so the RocksDB C++
# toolchain (cmake, clang, libclang-dev) is gone. protobuf-compiler is for gRPC
# codegen; build-essential/pkg-config cover native crates (zstd, openssl-sys).
RUN apt-get update && apt-get install -y \
    protobuf-compiler \
    build-essential \
    pkg-config \
    libssl-dev \
    git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/byoridb

# Copy manifests first for caching
COPY Cargo.toml Cargo.lock ./
COPY byoridb-common/Cargo.toml byoridb-common/
COPY byoridb-storage/Cargo.toml byoridb-storage/
COPY byoridb-graph/Cargo.toml byoridb-graph/
COPY byoridb-meta/Cargo.toml byoridb-meta/
COPY byoridb-kvstore/Cargo.toml byoridb-kvstore/
COPY byoridb-codec/Cargo.toml byoridb-codec/
COPY byoridb-parser/Cargo.toml byoridb-parser/
COPY byoridb-executor/Cargo.toml byoridb-executor/
COPY byoridb-client/Cargo.toml byoridb-client/
COPY byoridb-bulkloader/Cargo.toml byoridb-bulkloader/

# Create dummy source files to cache dependencies
RUN mkdir -p src \
    byoridb-common/src \
    byoridb-storage/src \
    byoridb-graph/src \
    byoridb-meta/src \
    byoridb-kvstore/src \
    byoridb-codec/src \
    byoridb-parser/src \
    byoridb-executor/src \
    byoridb-client/src \
    byoridb-bulkloader/src

RUN echo "fn main() {}" > src/main.rs && \
    echo "fn main() {}" > byoridb-client/src/main.rs && \
    echo "fn main() {}" > byoridb-bulkloader/src/main.rs && \
    touch byoridb-bulkloader/src/lib.rs && \
    touch byoridb-common/src/lib.rs && \
    touch byoridb-storage/src/lib.rs && \
    touch byoridb-graph/src/lib.rs && \
    touch byoridb-meta/src/lib.rs && \
    touch byoridb-kvstore/src/lib.rs && \
    touch byoridb-codec/src/lib.rs && \
    touch byoridb-parser/src/lib.rs && \
    touch byoridb-executor/src/lib.rs && \
    touch byoridb-client/src/lib.rs

# Copy proto files (needed for build.rs in byoridb-client/byoridb-graph helpers)
COPY byoridb-graph/proto byoridb-graph/proto

# Best-effort dep cache: pre-download crates.io dependencies with dummy sources.
# This step is expected to fail for projects with build.rs / proc-macros;
# || true is intentional here — the real build below will catch actual errors.
RUN cargo build --release --bin byoridb-server || true

# Now copy actual source
COPY . .

# Touch main.rs to force rebuild of the binary
RUN touch src/main.rs

# Build release (server + offline bulk loader + online CLI). `-p` is required:
# the bins live in different workspace packages, so `--bin X --bin Y` alone
# fails to resolve cross-package binaries ("no bin target ...").
RUN cargo build --release \
    -p byoridb --bin byoridb-server \
    -p byoridb-bulkloader --bin byoridb-bulkloader \
    -p byoridb-client --bin byoridb-cli

# Runtime Stage
FROM debian:bookworm-slim

# Install runtime dependencies (OpenSSL etc)
RUN apt-get update && apt-get install -y \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binaries from builder
COPY --from=builder /usr/src/byoridb/target/release/byoridb-server /usr/local/bin/byoridb-server
# Offline bulk loader — invoked by a Job (server scaled to 0) for large imports.
COPY --from=builder /usr/src/byoridb/target/release/byoridb-bulkloader /usr/local/bin/byoridb-bulkloader
# Online admin/query CLI — used by maintenance Jobs such as text index rebuilds.
COPY --from=builder /usr/src/byoridb/target/release/byoridb-cli /usr/local/bin/byoridb-cli

# Config is loaded via BYORIDB__* env vars (AppConfig file is optional)
# Create data directory
RUN mkdir -p /app/data

# Expose ports
# 9669: Graph Service (gRPC)
# 19669: HTTP Service
EXPOSE 9669 19669

# Config is loaded entirely via AppConfig defaults + optional byoridb.{toml,yaml}
# + BYORIDB__SECTION__KEY env vars (set by deployment, not baked into the image).
# Avoid Dockerfile ENV here: image-level ENV cannot be unset by a K8s ConfigMap,
# which previously caused Vec<String> deserialization failures on Pod startup.

CMD ["byoridb-server"]
