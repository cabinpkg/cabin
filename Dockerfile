# syntax=docker/dockerfile:1

ARG rust_image=rust:1-slim-bookworm
ARG runtime_image=debian:bookworm-slim

FROM ${rust_image} AS dev

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential ca-certificates git pkg-config \
      clang lld ninja-build \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /work

FROM dev AS builder

WORKDIR /app

COPY . .

RUN cargo build --workspace --release \
 && install -Dm755 target/release/cabin /usr/local/bin/cabin

FROM ${runtime_image} AS runtime

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential ca-certificates git pkg-config \
      clang lld ninja-build \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/bin/cabin /usr/local/bin/cabin

WORKDIR /work

CMD ["cabin"]
