# The Millennium dedicated server.
#
#   docker build -t millennium-server .
#   docker run -p 5888:5888/tcp -p 5888:5888/udp -v millennium:/data \
#     -e MILLENNIUM_PRIVATE_KEY=$(openssl rand -hex 32) millennium-server
#
# By default the world is a SQLite file in /data (with hourly backups in /data/backups). Set
# DATABASE_URL=postgres://... to use Postgres instead. See docs/deploy.md.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
# Cache the registry and the build between image builds (BuildKit).
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p server \
    && cp target/release/millennium-server /usr/local/bin/

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /data millennium
COPY --from=build /usr/local/bin/millennium-server /usr/local/bin/
COPY config/docker.toml /etc/millennium/server.toml
USER millennium
WORKDIR /data
VOLUME /data
# Login (TCP) and game (UDP) share the port. Map it to the same port on the host: clients are
# sent to the address they logged in on, at this port.
EXPOSE 5888/tcp 5888/udp
# Ctrl-C is the clean shutdown (everyone online is saved first); Docker's default is SIGTERM.
STOPSIGNAL SIGINT
ENTRYPOINT ["millennium-server", "--config", "/etc/millennium/server.toml"]
