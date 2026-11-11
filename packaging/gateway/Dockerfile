# fga-server: the GitHub API gateway for one Forgejo instance (server mode).
# Build from the repository root:  docker build -f packaging/gateway/Dockerfile -t fga-server .
FROM rust:1.99.1-slim-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
# The usage report's allowed values, which the gateway embeds.
COPY contract/telemetry.json ./contract/telemetry.json
# Only a release build reports usage to the project's collector (docs/telemetry.md).
RUN GFJ_TELEMETRY_RELEASE=1 cargo build --release --locked -p fga-server

FROM debian:trixie-slim
# CA certificates for an HTTPS upstream Forgejo.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/fga-server /usr/local/bin/fga-server
# Where the usage report keeps its first week and last day sent (docs/telemetry.md).
RUN mkdir -p /var/lib/fga-server && chown 65534:65534 /var/lib/fga-server
ENV FGA_STATE_DIR=/var/lib/fga-server
USER 65534:65534
# Set FGA_FORGEJO_URL (how the gateway reaches Forgejo) and FGA_PUBLIC_URL (the forge's own address).
ENV FGA_LISTEN=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/fga-server"]
