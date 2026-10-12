#!/usr/bin/env bash
# Backs up a Millennium Postgres database with pg_dump, keeping the newest copies.
#
#   DATABASE_URL=postgres://user:pass@host:5433/millennium scripts/backup-postgres.sh [dir] [keep]
#
# dir defaults to saves/backups, keep to 48. Run it from cron or a systemd timer (see
# docs/deploy.md). The dump is safe to take while the server runs (pg_dump reads one consistent
# snapshot). PG_DUMP overrides the command, e.g. to run the server's own version:
#   PG_DUMP="docker run --rm --network host postgres:17 pg_dump"
set -euo pipefail

: "${DATABASE_URL:?set DATABASE_URL to the database to back up}"
dir=${1:-saves/backups}
keep=${2:-48}
PG_DUMP=${PG_DUMP:-pg_dump}

mkdir -p "$dir"
file="$dir/millennium-$(date -u +%Y%m%d-%H%M%S).dump"
# Write to a temporary name first, so a failed dump never looks like a good backup.
$PG_DUMP --format=custom --no-owner "$DATABASE_URL" > "$file.partial"
mv "$file.partial" "$file"

# Delete all but the newest $keep (names sort by time).
ls -1 "$dir"/millennium-*.dump | sort | awk -v keep="$keep" '{ all[NR] = $0 } END { for (i = 1; i <= NR - keep; i++) print all[i] }' |
  while read -r old; do rm -f -- "$old"; done

echo "$file"
