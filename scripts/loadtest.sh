#!/usr/bin/env bash
# Load test: N bots wander around Downtown (all in one zone, the worst case) and chat now and then,
# while the server's own metrics are sampled.
#
#   scripts/loadtest.sh [BOTS] [SECONDS]          (default 50 bots for 120 s)
#   DATABASE_URL=postgres://... scripts/loadtest.sh 100 300
#
# Without DATABASE_URL the server uses an in-memory database. Writes loadtest-<time>.csv in the
# current directory (one row
# per sample) and prints a summary: players, frame time, database latency, bandwidth (total and
# per player), and the server's CPU and memory.
#
# Needs `cargo build --release -p server` and `cargo build --release -p shared --examples`
# (separately). Debug builds work too: set PROFILE=debug.
set -euo pipefail

bots=${1:-50}
secs=${2:-120}
profile=${PROFILE:-release}
here=$PWD
cd "$(dirname "$0")/.."
server=target/$profile/millennium-server
smoke=target/$profile/examples/smoke
for bin in "$server" "$smoke"; do
  [[ -x $bin ]] || { echo "missing $bin: cargo build --$profile -p server, then -p shared --examples" >&2; exit 1; }
done

work=$(mktemp -d)
metrics=127.0.0.1:9188
pid=
cleanup() {
  pkill -P $$ -f examples/smoke 2>/dev/null || true
  if [[ -n $pid ]]; then kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null || true; fi
  rm -rf "$work"
}
trap cleanup EXIT

cat > "$work/server.toml" <<EOF
[network]
tls_dir = "$work/tls"
max_clients = $((bots + 16))
[database]
url = "${DATABASE_URL:-}"
backup_interval_minutes = 0.0
[chat]
# Bots chat in bursts; don't mute them for it.
burst = 20.0
per_second = 5.0
[metrics]
bind = "$metrics"
log_every_secs = 30
EOF

"$server" --config "$work/server.toml" > "$work/server.log" 2>&1 &
pid=$!
for _ in $(seq 1 60); do
  curl -sf "http://$metrics/health" > /dev/null && break
  sleep 1
done
curl -sf "http://$metrics/health" > /dev/null || { cat "$work/server.log"; echo "server didn't start" >&2; exit 1; }

# Value of a metric from /metrics (0 if missing).
metric() { awk -v name="$1" '$1 == name { print $2; found = 1 } END { if (!found) print 0 }' "$work/metrics.txt"; }
sample() {
  curl -sf "http://$metrics/metrics" > "$work/metrics.txt" || return 0
  local cpu rss
  read -r cpu rss < <(ps -o %cpu=,rss= -p "$pid")
  echo "$(date +%s),$(metric millennium_players_online),$(metric millennium_frame_seconds_avg),$(metric millennium_frame_seconds_max),$(metric millennium_db_command_seconds_avg),$(metric millennium_db_command_seconds_max),$(metric millennium_network_sent_bytes_total),$(metric millennium_network_received_bytes_total),$cpu,$rss" >> "$csv"
}

csv="$here/loadtest-$(date +%Y%m%d-%H%M%S).csv"
echo "time,players,frame_avg_s,frame_max_s,db_avg_s,db_max_s,sent_bytes,recv_bytes,cpu_percent,rss_kb" > "$csv"

echo "== $bots bots for ${secs}s ($profile build); samples in $csv"
# Log in in waves of 10: account creation hashes a password (argon2), which is meant to be slow.
for i in $(seq 1 "$bots"); do
  # Lower priority: on one machine, the bots must not steal the server's CPU (that's not what
  # we're measuring).
  nice -n 10 "$smoke" --known "$work/known.txt" --interval 0.5 --name "Bot$i" --character "Bot$i" \
    --step "!wander $secs" > "$work/bot$i.log" 2>&1 &
  (( i % 10 == 0 )) && sleep 2
done

start=$(date +%s)
while (( $(date +%s) - start < secs + 10 )); do
  sample
  printf "\r   %3ss: %s players online" "$(( $(date +%s) - start ))" "$(metric millennium_players_online)"
  sleep 10
done
echo

# Summary, from the samples taken while (nearly) everyone was on.
awk -F, -v bots="$bots" 'NR > 1 {
    n++; players[n] = $2; fa[n] = $3; fm[n] = $4; da[n] = $5; dm[n] = $6; tx[n] = $7; rx[n] = $8; cpu[n] = $9; rss[n] = $10; t[n] = $1
  }
  END {
    peak = 0; for (i = 1; i <= n; i++) if (players[i] > peak) peak = players[i]
    for (i = 1; i <= n; i++) if (players[i] >= peak * 0.9) { if (!first) first = i; last = i }
    for (i = first; i <= last; i++) { k++; sfa += fa[i]; scpu += cpu[i]; if (fm[i] > mfm) mfm = fm[i]; sda += da[i]; if (dm[i] > mdm) mdm = dm[i]; if (rss[i] > mrss) mrss = rss[i] }
    dt = t[last] - t[first]; if (dt < 1) dt = 1
    sent = (tx[last] - tx[first]) / dt; recvd = (rx[last] - rx[first]) / dt
    printf "players: %d of %d bots online at the peak\n", peak, bots
    printf "frame:   %.2f ms average, %.2f ms worst (a frame budget at 30 Hz is 33 ms)\n", sfa / k * 1000, mfm * 1000
    printf "db:      %.2f ms average, %.2f ms worst per command\n", sda / k * 1000, mdm * 1000
    printf "network: %.1f KB/s out, %.1f KB/s in (%.2f KB/s out per player)\n", sent / 1024, recvd / 1024, sent / 1024 / (peak > 0 ? peak : 1)
    printf "server:  %.0f%% CPU (of one core), %.0f MB memory at most\n", scpu / k, mrss / 1024
  }' "$csv"

failed=$( (grep -l "AUTH   failed\|panicked" "$work"/bot*.log 2>/dev/null || true) | wc -l | tr -d " ")
echo "bots that failed to log in or crashed: $failed"
(grep -a "status:" "$work/server.log" || true) | tail -2 | sed 's/.*status: /server log: /'
# A server built with `--features profile` logs slow frames and what took the time.
if grep -aq "slow frame" "$work/server.log"; then
  grep -a "slow frame" "$work/server.log" | sed 's/.*slow frame: //' > "${csv%.csv}-slow.txt"
  echo "slow frames: $(wc -l < "${csv%.csv}-slow.txt" | tr -d ' ') (details in ${csv%.csv}-slow.txt)"
fi
