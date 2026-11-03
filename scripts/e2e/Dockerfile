# Test runner only; packaging the service is outside G1.
FROM rust:1.97.1
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl git jq openssl python3 \
    && rm -rf /var/lib/apt/lists/*
ARG GH_VERSION=2.103.3
RUN arch="$(dpkg --print-architecture)" \
    && curl -fsSL "https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_${arch}.tar.gz" -o /tmp/gh.tar.gz \
    && curl -fsSL "https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_checksums.txt" -o /tmp/checksums \
    && digest="$(awk -v name="gh_${GH_VERSION}_linux_${arch}.tar.gz" '$2 == name {print $1}' /tmp/checksums)" \
    && test -n "$digest" && echo "$digest  /tmp/gh.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/gh.tar.gz -C /tmp \
    && cp "/tmp/gh_${GH_VERSION}_linux_${arch}/bin/gh" /usr/local/bin/gh \
    && rm -rf /tmp/gh* /tmp/checksums
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
# The shim embeds its gh grammar and usage captures at compile time.
COPY contract ./contract
RUN --mount=type=cache,id=fga-cargo-registry,target=/usr/local/cargo/registry --mount=type=cache,id=fga-g2a-target,target=/src/target \
    cargo build -p fga-server --locked && cp target/debug/fga-server /usr/local/bin/fga-server \
    && cargo build -p gh-forgejo-shim --locked && cp target/debug/gh-forgejo-shim /usr/local/bin/gh-forgejo-shim
COPY scripts/e2e/client.sh /test/client.sh
COPY scripts/e2e/assert.py /test/assert.py
COPY fixtures/gh-pr-view.graphql /test/gh-pr-view.graphql

COPY scripts/e2e/g2.sh scripts/e2e/seed.py scripts/e2e/read-contract.py scripts/e2e/merge_retry.py /test/
COPY scripts/oracle /test/oracle-scripts
COPY fixtures/oracle /test/oracle

COPY scripts/e2e/write-seed.py /test/write-seed.py
