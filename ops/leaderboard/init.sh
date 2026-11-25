#!/bin/sh
set -eu
psql --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" \
  --set ON_ERROR_STOP=1 --set "authenticator_password=$WZQ_DB_AUTH_PASSWORD" \
  --file /opt/wzq/schema.sql
