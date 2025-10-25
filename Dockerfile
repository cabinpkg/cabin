ARG base=ubuntu:24.04


FROM $base AS builder

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential ca-certificates git pkg-config \
      libfmt-dev libspdlog-dev libgit2-dev libcurl4-openssl-dev nlohmann-json3-dev libtbb-dev \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY .clang-format .
COPY .clang-tidy .
COPY cabin.toml .
COPY .git .
COPY Makefile .
COPY src ./src/

RUN make BUILD=release install


FROM $base

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential clang ninja-build \
      libfmt-dev libspdlog-dev libgit2-dev libcurl4-openssl-dev nlohmann-json3-dev libtbb-dev \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/bin/cabin /usr/local/bin/cabin

CMD ["cabin"]
