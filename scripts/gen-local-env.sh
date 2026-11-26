#!/bin/sh
# Writes .env with random local secrets and a PostgREST JWT (HS256, role wzq_app).
set -eu
cd "$(dirname "$0")/.."
if [ -f .env ]; then
  echo ".env already exists; delete it first to regenerate" >&2
  exit 1
fi
secret=$(openssl rand -hex 32)
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
header=$(printf '{"alg":"HS256","typ":"JWT"}' | b64url)
payload=$(printf '{"role":"wzq_app"}' | b64url)
sig=$(printf '%s.%s' "$header" "$payload" | openssl dgst -sha256 -hmac "$secret" -binary | b64url)
umask 077
cat > .env <<EOF
WZQ_DB_OWNER_PASSWORD=$(openssl rand -hex 24)
WZQ_DB_AUTH_PASSWORD=$(openssl rand -hex 24)
WZQ_POSTGREST_JWT_SECRET=$secret
WZQ_POSTGREST_TOKEN=$header.$payload.$sig
EOF
echo "wrote .env"
