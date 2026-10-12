# Running a Millennium server

How to run a dedicated server that stays up: with Docker, or as a systemd service. (To play with
a few friends from your own computer, **Host Game** in the client is simpler; see the README.)

## What you need

- **A Linux machine**, such as a small VPS. 1 CPU and 1 GB of RAM are plenty for a few dozen
  players.
- **Port 5888 open for TCP and UDP.** The login service uses TCP and the game uses UDP, on the
  same number.
- **A database:**
  - **SQLite** (a single file, nothing else to run) is fine for one server.
  - **PostgreSQL** is better for many players, for running the world on several servers (cluster
    mode), or if you already run Postgres.
- **A signing key** for connect tokens (64 hex characters). Make one with
  `millennium-server --generate-key` (or `openssl rand -hex 32`) and keep it secret. Without it,
  the server uses an all-zero development key and warns at startup: anyone could forge tokens.

Players connect with the client's **Server** field: `your.host.name:5888`.

## Option A: Docker

The image contains the server only (the client isn't needed on a server).

```sh
docker build -t millennium-server .
```

### SQLite, in a volume

```sh
docker run -d --name millennium --restart unless-stopped \
  -p 5888:5888/tcp -p 5888:5888/udp \
  -v millennium:/data \
  -e MILLENNIUM_PRIVATE_KEY=<your key> \
  millennium-server
```

The volume holds everything:
- `/data/world.db`: the world.
- `/data/backups/`: hourly backups.
- `/data/tls/`: the login certificate. Keep it: if it changes, players get an "identity has
  changed" warning.

Map the port to **the same number** on the host: clients are sent to port 5888 of the address
they logged in on.

### With Postgres (docker compose)

`docker-compose.yml` has Postgres and, behind the `server` profile, the game server:

```sh
echo "MILLENNIUM_PRIVATE_KEY=$(openssl rand -hex 32)" > .env
docker compose --profile server up -d --build
docker compose logs -f server
```

Change the Postgres password in `docker-compose.yml` (in both places) before you expose anything.
The database port (5433 on the host) doesn't need to be reachable from outside.

### Configuration in the image

The image's config is `config/docker.toml`. It uses SQLite in `/data` and the defaults from
`config/server.toml` for everything else. Override with environment variables:

| Variable | What it sets |
|---|---|
| `DATABASE_URL` | `postgres://user:pass@host:5432/db`, or `sqlite:///data/world.db` |
| `MILLENNIUM_PRIVATE_KEY` | The token signing key |
| `RUST_LOG` | Log levels, e.g. `info` or `info,server=debug` |

For anything else (admins, chat limits, the length of a day), mount your own file over
`/etc/millennium/server.toml`.

## Option B: systemd

### 1. Build and install

Build on the server, or on a machine with the same Linux distribution:

```sh
git clone https://github.com/rsantacroce/millennium.git && cd millennium
cargo build --release -p server
sudo install -m 755 target/release/millennium-server /usr/local/bin/
sudo mkdir -p /opt/millennium && sudo cp -r scripts /opt/millennium/
```

Content (items, quests, maps) and database migrations are compiled into the binary, so nothing
else needs copying.

### 2. A user, a home and a config

```sh
sudo useradd --system --home-dir /var/lib/millennium --create-home millennium
sudo mkdir -p /etc/millennium
sudo cp config/server.toml /etc/millennium/server.toml
```

Edit `/etc/millennium/server.toml`:
- **`[database] url`:** `sqlite://world.db` (relative to `/var/lib/millennium`), or your Postgres
  URL (better kept in the env file below).
- **`[chat] admins`:** the account names allowed to `/mute` and use the `/give...` commands.
- **`[world] day_minutes`:** the length of a game day (default 40).
- **`[network] max_clients`.**

Put the secrets in `/etc/millennium/env`, readable only by root:

```sh
sudo tee /etc/millennium/env >/dev/null <<EOF
MILLENNIUM_PRIVATE_KEY=$(millennium-server --generate-key)
# DATABASE_URL=postgres://millennium:secret@localhost:5432/millennium
EOF
sudo chmod 600 /etc/millennium/env
```

### 3. Start it

```sh
sudo cp deploy/millennium-server.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now millennium-server
journalctl -u millennium-server -f
```

The unit stops the server with SIGINT, the clean shutdown that saves everyone online. It
restarts the server if it crashes, and only lets it write to `/var/lib/millennium`.

### 4. Open the firewall

```sh
sudo ufw allow 5888/tcp && sudo ufw allow 5888/udp     # or your cloud provider's firewall
```

## Backups

### SQLite: automatic

The server backs up its own save file:
- **When:** at every start, then every hour.
- **Where:** `saves/backups/` (`/data/backups` in Docker).
- **How many:** the newest 48.
- **Settings:** `backup_dir`, `backup_interval_minutes` and `backup_keep` under `[database]`.

Copies are made with SQLite's `VACUUM INTO` between database commands, so they're consistent
even while people play. Copy the backups folder off the machine now and then (rsync,
restic...).

**To restore:**
1. Stop the server.
2. Copy a backup over the save file.
3. Delete the `-wal` and `-shm` files next to the save file.
4. Start the server again.

```sh
sudo systemctl stop millennium-server
cd /var/lib/millennium
cp saves/backups/world-20261012-120000-01.db world.db && rm -f world.db-wal world.db-shm
sudo systemctl start millennium-server
```

### Postgres: a timer

`scripts/backup-postgres.sh` takes a `pg_dump` (safe while the server runs) and keeps the newest
copies. Run it hourly with the timer:

```sh
sudo cp deploy/millennium-backup.service deploy/millennium-backup.timer /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now millennium-backup.timer
```

It needs `pg_dump` at least as new as the Postgres server. If your distribution's is older, use
the server's own, e.g. `PG_DUMP="docker run --rm --network host postgres:17 pg_dump"` in
`/etc/millennium/env`.

**To restore** (into a new database, or over the old one with the server stopped):

```sh
DATABASE_URL=postgres://millennium:secret@localhost:5432/millennium \
  scripts/restore-postgres.sh /var/lib/millennium/backups/millennium-20261012-120000.dump
```

Both paths are tested in CI: `scripts/e2e.sh` plays the tutorial, kills the server with `kill
-9`, restarts it, then restores a backup into a fresh database. Each time the player's progress
must be intact.

## Upgrading

1. **Take a backup:** the SQLite one at startup is automatic; for Postgres, run the backup
   service once with `systemctl start millennium-backup`.
2. **Build and install the new binary** (or `docker compose build`).
3. **Restart:** `systemctl restart millennium-server` (or `docker compose up -d`). Database
   migrations run by themselves at startup.

Players need a client from the same version: an older client gets "protocol doesn't match".

## Checking on it

- **Logs:** `journalctl -u millennium-server` or `docker logs millennium`. Logins, knockouts,
  backups, zone transfers and errors all show up there.
- **"failed to start database":** the database URL is wrong or Postgres isn't up yet. systemd
  retries every 5 s.
- **"could not listen on UDP port 5888 / TCP port 5888":** something else is using the port.
- **Players get "Logged in, but the game server didn't answer":** UDP 5888 is blocked
  somewhere. TCP is getting through, but UDP isn't.

## Bigger worlds

The world can be split over several server processes (each running some of the zones), linked by
a world hub. See *Running the world on several servers* in the README and the example configs in
`config/cluster/`. Every zone server shares the database, the signing key (set
`MILLENNIUM_PRIVATE_KEY` the same everywhere) and `cluster.secret` (or
`MILLENNIUM_CLUSTER_SECRET`).
