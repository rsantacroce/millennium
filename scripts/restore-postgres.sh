#!/usr/bin/env bash
# Restores a backup made by backup-postgres.sh into a database, creating it if it's missing and
# replacing whatever is in it. Stop the game server first if it uses that database.
#
#   DATABASE_URL=postgres://user:pass@host:5433/millennium scripts/restore-postgres.sh saves/backups/millennium-20261012-120000.dump
#
# PG_RESTORE and PSQL override the commands (like PG_DUMP in backup-postgres.sh), e.g.
#   PG_RESTORE="docker run -i --rm --network host postgres:17 pg_restore"
#   PSQL="docker run -i --rm --network host postgres:17 psql"
set -euo pipefail

: "${DATABASE_URL:?set DATABASE_URL to the database to restore into}"
dump=${1:?usage: restore-postgres.sh BACKUP_FILE}
[[ -f $dump ]] || { echo "no such file: $dump" >&2; exit 1; }
PG_RESTORE=${PG_RESTORE:-pg_restore}
PSQL=${PSQL:-psql}

# Connect to the server's `postgres` database to create the target if needed.
base=${DATABASE_URL%/*}
db=${DATABASE_URL##*/}
db=${db%%\?*}
if ! $PSQL "$base/postgres" -tAc "SELECT 1 FROM pg_database WHERE datname = '$db'" | grep -q 1; then
  $PSQL "$base/postgres" -qc "CREATE DATABASE \"$db\""
fi

$PG_RESTORE --clean --if-exists --no-owner --dbname "$DATABASE_URL" < "$dump"
echo "restored $dump into $db"
