#!/usr/bin/env bash
# End to end: a real server on a real database, a scripted player, a crash, a restart.
#
#   DATABASE_URL=postgres://user:pass@localhost:5432/db scripts/e2e.sh
#   DATABASE_URL=sqlite:///tmp/e2e.db scripts/e2e.sh
#
# 1. Starts the server on DATABASE_URL (migrations run on an empty database).
# 2. A brand-new player plays the tutorial (Welcome to the City) with the smoke tool, then drinks
#    a diner coffee (a 10-minute buff) and buys two of the pawn shop's three walkmen.
# 3. The server is killed with SIGKILL: no shutdown, no final save.
# 4. A new server starts on the same database; the player logs back in, and everything the
#    tutorial gave them must still be there, and so must the buff and the shop's low stock.
# 5. A backup is restored into a new database (SQLite: the copy the server makes at startup;
#    Postgres: backup-postgres.sh and restore-postgres.sh), and the player must find it all there.
#
# For Postgres, PG_DUMP, PG_RESTORE and PSQL can point at the server's own tools (see the scripts).
#
# Needs `cargo build -p server` and `cargo build -p shared --examples` first (separately: building
# both packages in one command only builds the examples).
set -euo pipefail

: "${DATABASE_URL:?set DATABASE_URL to the database to test}"
cd "$(dirname "$0")/.."
server=target/debug/millennium-server
smoke=target/debug/examples/smoke
for bin in "$server" "$smoke"; do
  [[ -x $bin ]] || { echo "missing $bin: build it first" >&2; exit 1; }
done

work=$(mktemp -d)
pid=
cleanup() { if [[ -n $pid ]]; then kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null || true; fi; rm -rf "$work"; }
trap cleanup EXIT

cat > "$work/server.toml" <<EOF
[network]
tls_dir = "$work/tls"
[database]
url = "$DATABASE_URL"
backup_dir = "$work/backups"
[chat]
admins = ["Tester"]
EOF

# start_server [database url]: the URL overrides the config (the server reads DATABASE_URL).
start_server() {
  DATABASE_URL=${1:-$DATABASE_URL} "$server" --config "$work/server.toml" >> "$work/server.log" 2>&1 &
  pid=$!
  # Wait for the login service (TCP 5888).
  for _ in $(seq 1 60); do
    if (exec 3<>/dev/tcp/127.0.0.1/5888) 2>/dev/null; then return 0; fi
    if ! kill -0 "$pid" 2>/dev/null; then cat "$work/server.log"; echo "server died" >&2; exit 1; fi
    sleep 1
  done
  cat "$work/server.log"; echo "server didn't start listening" >&2; exit 1
}

play() {
  "$smoke" --known "$work/known.txt" --interval 0.7 --name Tester --character Tester "$@" 2>&1 | tee -a "$work/smoke.log"
}

echo "== 1. start the server on an empty database"
start_server

echo "== 2. play the tutorial"
play --step "!goto -7.5 -6" --step "!talk pruitt" --step "!choose First" --step "!choose Sure" \
     --step "!goto -14 5.6" --step "!talk kim" --step "!choose Let" --step "!buy soda 1" --step "!closeshop" \
     --step "!goto -7.5 -6" --step "!talk pruitt" --step "!choose Here" --step "!wait 2" --step "!state" > "$work/before.txt"
grep -a "STATE" "$work/before.txt" | tail -1
grep -aq 'welcome=done' "$work/before.txt" || { cat "$work/before.txt"; echo "the tutorial didn't complete" >&2; exit 1; }
# Sal sells walkmen 3 at a time (one more every 90 s). The checks below run well within 180 s.
play --step "/give diner_coffee 1" --step "!wait 1" --step "!use diner_coffee" \
     --step "!goto -31 -5.5" --step "!talk sal" --step "!choose Show" --step "!buy broken_walkman 2" --step "!closeshop" \
     --step "!wait 2" --step "!state" > "$work/shopping.txt"
grep -a "STATE" "$work/shopping.txt" | tail -1
grep -a "STATE" "$work/shopping.txt" | tail -1 | grep -q 'Driving +1' || { cat "$work/shopping.txt"; echo "no coffee buff" >&2; exit 1; }

echo "== 3. kill -9 the server"
kill -9 "$pid"
wait "$pid" 2>/dev/null || true
pid=

echo "== 4. restart and log back in"
start_server
# check_state FILE WHAT: the last STATE line in FILE still has the tutorial's rewards.
check_state() {
  local state
  state=$(grep -a "STATE" "$1" | tail -1)
  echo "$state"
  for expect in 'welcome=done' '\$18' 'First Aid Kit x1' 'Broken Walkman x1", "Broken Walkman x1' 'Driving +1'; do
    grep -q "$expect" <<<"$state" || { cat "$1"; echo "lost $2: $expect" >&2; exit 1; }
  done
  # Sal hasn't restocked to three (a restart used to restock everything).
  local shop
  shop=$(grep -a "SHOP   stock" "$1" | tail -1)
  echo "$shop"
  grep -q '"broken_walkman", [012])' <<<"$shop" || { cat "$1"; echo "the pawn shop restocked $2" >&2; exit 1; }
}
look() {
  play --step "!wait 1" --step "!state" --step "!goto -31 -5.5" --step "!talk sal" --step "!choose Show" --step "!wait 1" --step "!closeshop"
}
look > "$work/after.txt"
check_state "$work/after.txt" "after the crash"

echo "== 5. restore a backup into a new database"
kill "$pid"; wait "$pid" 2>/dev/null || true; pid=
case $DATABASE_URL in
  sqlite:*)
    # The server backs up its save file at every startup; the newest is from step 4.
    latest=$(ls -1 "$work/backups"/*.db | sort | tail -1)
    echo "restoring $latest"
    cp "$latest" "$work/restored.db"
    restored="sqlite://$work/restored.db"
    ;;
  *)
    dump=$(scripts/backup-postgres.sh "$work/backups" 5)
    restored="${DATABASE_URL%/*}/millennium_restored"
    DATABASE_URL=$restored scripts/restore-postgres.sh "$dump"
    ;;
esac
start_server "$restored"
look > "$work/restored.txt"
check_state "$work/restored.txt" "in the restored backup"

echo "== passed on $(cut -d: -f1 <<<"$DATABASE_URL"): the tutorial's results, a buff and shop stock survived kill -9, and a restored backup"
