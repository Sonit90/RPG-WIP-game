#!/bin/bash
# Runs once on first volume init (mounted into /docker-entrypoint-initdb.d/).
# Creates the `auth` schema and sets it as the DB-level default search_path,
# so every subsequent session — including sqlx creating `_sqlx_migrations` —
# operates inside `auth` without qualifying every identifier.
set -e

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" <<-EOSQL
    CREATE SCHEMA IF NOT EXISTS auth;
    ALTER DATABASE "$POSTGRES_DB" SET search_path TO auth;
EOSQL
