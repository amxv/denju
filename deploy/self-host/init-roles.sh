#!/bin/sh
set -eu

: "${DENJU_DB_APP_PASSWORD:?DENJU_DB_APP_PASSWORD is required}"
: "${DENJU_DB_WORKER_PASSWORD:?DENJU_DB_WORKER_PASSWORD is required}"

psql --set ON_ERROR_STOP=1 \
  --set app_password="$DENJU_DB_APP_PASSWORD" \
  --set worker_password="$DENJU_DB_WORKER_PASSWORD" \
  --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" <<'SQL'
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'denju_app') THEN
    CREATE ROLE denju_app LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'denju_worker') THEN
    CREATE ROLE denju_worker LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
  END IF;
END
$$;
ALTER ROLE denju_app PASSWORD :'app_password';
ALTER ROLE denju_worker PASSWORD :'worker_password';
SQL
