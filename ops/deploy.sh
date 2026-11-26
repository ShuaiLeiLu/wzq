#!/usr/bin/env bash
# Forced SSH command for the wzq-deploy user. Reads a gzipped `docker save`
# of wzq-server:<revision> on stdin, starts it as the `wzq` container and
# restores the previous image when the health check fails.
set -euo pipefail
umask 022

if [[ ! ${SSH_ORIGINAL_COMMAND:-} =~ ^deploy\ ([0-9a-f]{40})$ ]]; then
  echo 'Expected deploy followed by a full Git revision.' >&2
  exit 2
fi

revision=${BASH_REMATCH[1]}
base=${WZQ_BASE:-/opt/wzq}
port=${WZQ_PORT:-3041}
env_file=${WZQ_ENV_FILE:-/etc/wzq/leaderboard.env}
container=${WZQ_CONTAINER:-wzq}
image="wzq-server:$revision"
keep_images=3

exec 9>"$base/.deploy.lock"
flock -w 60 9

if systemctl is-active --quiet wzq.service 2>/dev/null; then
  echo 'The old wzq.service is still running on the game port. Stop and disable it first.' >&2
  exit 1
fi

upload=$(mktemp "$base/.upload.XXXXXXXX")
trap 'rm -f -- "$upload"' EXIT

limit=$((512 * 1024 * 1024))
head -c $((limit + 1)) > "$upload"
if (($(stat -c %s "$upload") > limit)); then
  echo 'Compressed image is too large.' >&2
  exit 1
fi

# Only accept an archive that holds exactly this revision's image, so an
# upload can never retag other images on the host (such as the database).
python3 - "$upload" "$image" <<'PY'
import json
import sys
import tarfile

path, expected = sys.argv[1:]
with tarfile.open(path, mode="r:gz") as archive:
    member = archive.getmember("manifest.json")
    manifest = json.load(archive.extractfile(member))
tags = [tag for entry in manifest for tag in entry.get("RepoTags") or []]
if tags != [expected]:
    sys.exit(f"Unexpected image tags in upload: {tags}")
PY

docker load --quiet < "$upload" >/dev/null
rm -f -- "$upload"

previous=$(docker inspect --format '{{.Config.Image}}' "$container" 2>/dev/null || true)

healthy() {
  for _ in {1..20}; do
    if [[ $(docker inspect --format '{{.State.Running}}' "$container" 2>/dev/null) == true ]] \
      && curl --silent --fail --max-time 1 "http://127.0.0.1:$port/healthz" >/dev/null; then
      return 0
    fi
    sleep 1
  done
  return 1
}

start() {
  docker rm --force "$container" >/dev/null 2>&1 || true
  local env_args=()
  [[ -r "$env_file" ]] && env_args=(--env-file "$env_file")
  # Host networking keeps the server on loopback beside PostgREST, where
  # Caddy already proxies to it.
  docker run --detach --name "$container" \
    --restart unless-stopped \
    --network host \
    "${env_args[@]}" \
    --env HOST=127.0.0.1 --env "PORT=$port" \
    --mount type=volume,source=wzq-history-outbox,target=/var/lib/wzq/history \
    --read-only --tmpfs /tmp \
    --cap-drop ALL --security-opt no-new-privileges \
    --memory 192m --pids-limit 256 \
    --log-opt max-size=10m --log-opt max-file=3 \
    --label "wzq.revision=${1#wzq-server:}" \
    "$1" >/dev/null
}

prune() {
  docker image ls wzq-server --format '{{.Repository}}:{{.Tag}}' |
    tail -n +$((keep_images + 1)) |
    while read -r old; do
      [[ "$old" == "$image" || "$old" == "$previous" ]] || docker image rm "$old" >/dev/null 2>&1 || true
    done
}

if [[ "$previous" == "$image" ]] && healthy; then
  printf '%s is already running. Health check passed.\n' "$revision"
  exit 0
fi

if start "$image" && healthy; then
  printf 'Deployed %s. Health check passed.\n' "$revision"
  prune
  exit 0
fi

echo "Health check failed for $revision. Recent logs:" >&2
docker logs --tail 30 "$container" >&2 2>&1 || true
if [[ -n "$previous" && "$previous" != "$image" ]]; then
  if start "$previous" && healthy; then
    printf 'Deployment failed. Restored %s.\n' "$previous" >&2
  else
    printf 'Deployment failed. Health check also failed after restoring %s.\n' "$previous" >&2
  fi
else
  docker rm --force "$container" >/dev/null 2>&1 || true
fi
exit 1
