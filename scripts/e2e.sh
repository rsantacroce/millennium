#!/usr/bin/env bash
# End to end: a real server on a real database, a scripted player, a crash, a restart.
#
#   DATABASE_URL=postgres://user:pass@localhost:5432/db scripts/e2e.sh
#   DATABASE_URL=sqlite:///tmp/e2e.db scripts/e2e.sh
#
# 1. Starts the server on DATABASE_URL (migrations run on an empty database).
# 2. A brand-new player plays the tutorial (Welcome to the City) with the smoke tool.
# 3. The server is killed with SIGKILL: no shutdown, no final save.
# 4. A new server starts on the same database; the player logs back in, and everything the
#    tutorial gave them must still be there.
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
EOF

start_server() {
  "$server" --config "$work/server.toml" >> "$work/server.log" 2>&1 &
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

echo "== 3. kill -9 the server"
kill -9 "$pid"
wait "$pid" 2>/dev/null || true
pid=

echo "== 4. restart and log back in"
start_server
play --step "!wait 1" --step "!state" > "$work/after.txt"
after=$(grep -a "STATE" "$work/after.txt" | tail -1)
echo "$after"
for expect in 'welcome=done' '\$34' 'First Aid Kit x1'; do
  grep -q "$expect" <<<"$after" || { cat "$work/after.txt"; echo "lost after the crash: $expect" >&2; exit 1; }
done

echo "== passed: the tutorial's results survived kill -9 on $(cut -d: -f1 <<<"$DATABASE_URL")"
