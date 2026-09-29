# SPDX-License-Identifier: Apache-2.0
#
# voidfs-server in a small image. Build from the repository root:
#
#   docker build -t voidfs-server .
#
# It keeps its pool in /data unless VOIDFS_STORE says otherwise, and reads every other setting
# from the environment (see `voidfs-server --help`). deploy/compose/ runs it with Compose.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p voidfs-server \
    && cp target/release/voidfs-server /usr/local/bin/voidfs-server

FROM debian:bookworm-slim
# ca-certificates for HTTPS buckets; curl for health checks.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data voidfs \
    && mkdir /data && chown voidfs /data
COPY --from=build /usr/local/bin/voidfs-server /usr/local/bin/voidfs-server
USER voidfs
ENV VOIDFS_STORE=fs:/data \
    VOIDFS_LISTEN=0.0.0.0:9000
VOLUME /data
EXPOSE 9000
ENTRYPOINT ["voidfs-server"]
