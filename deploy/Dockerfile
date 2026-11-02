FROM rust:1.88-bookworm AS build
WORKDIR /src
COPY vendor/branches /vendor/branches
COPY apps/core/Cargo.toml apps/core/Cargo.lock* ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo fetch
COPY apps/core/src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates docker.io python3 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/jarvis-core /usr/local/bin/jarvis-core
ENV JARVIS_DATA=/data
ENV JARVIS_VAULT=/data/vault
ENV JARVIS_DOCKER=1
EXPOSE 8080
CMD ["jarvis-core"]
