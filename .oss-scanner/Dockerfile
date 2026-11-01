# Anthropic OSS Scanner image (https://github.com/anthropics/oss-scanner): the
# Cabin CLI workspace, the standalone registry Worker workspace, and everything
# their tests and `cargo registry-smoke` need offline. The checkout lives at
# /src; threat_model.md next to this file says what to exercise.
FROM node:24-bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

# worker-build links OpenSSL; the CLI tests need clang-format, clang-tidy, zlib.
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential ca-certificates clang clang-format clang-tidy curl git \
      libssl-dev lld ninja-build pkg-config zlib1g-dev \
    && rm -rf /var/lib/apt/lists/* \
    && curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none

WORKDIR /src

# Ahead of the checkout, so a source change reuses this layer: only a
# rust-toolchain.toml change rebuilds the toolchain and worker-build.
COPY rust-toolchain.toml ./
RUN rustup toolchain install && rustup target add wasm32-unknown-unknown \
    && cargo install -q "worker-build@=0.8.5" --locked

COPY . /src

RUN cargo fetch --locked \
    && cargo test --workspace --all-targets --all-features --locked --no-run

RUN cd registry \
    && cargo fetch --locked \
    && cargo test --locked --no-run \
    && worker-build --release \
    && npm ci --ignore-scripts \
    && node_modules/.bin/wrangler --version

# The scan has no network: resolve from the caches above instead of timing out.
ENV CARGO_NET_OFFLINE=true \
    npm_config_offline=true
