# syntax=docker/dockerfile:1

FROM rust:1.88-bookworm AS build
WORKDIR /src
ENV CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse \
    CARGO_TERM_COLOR=never \
    CARGO_PROFILE_RELEASE_LTO=false \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
    CARGO_PROFILE_RELEASE_OPT_LEVEL=2

COPY vendor/branches /vendor/branches
COPY apps/core/Cargo.toml apps/core/Cargo.lock ./
RUN mkdir src && printf 'fn main() {}\n' > src/main.rs
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo build --release --locked

COPY apps/core/src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    rm -rf target/release/jarvis-core \
        target/release/deps/jarvis_core-* \
        target/release/.fingerprint/jarvis-core-* \
        target/release/incremental/jarvis_core-* \
    && cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates docker.io python3 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/jarvis-core /usr/local/bin/jarvis-core
ENV JARVIS_DATA=/data
ENV JARVIS_VAULT=/data/vault
ENV JARVIS_DOCKER=1
EXPOSE 8080
CMD ["jarvis-core"]
