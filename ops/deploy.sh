#!/usr/bin/env bash
set -euo pipefail
umask 022

if [[ ! ${SSH_ORIGINAL_COMMAND:-} =~ ^deploy\ ([0-9a-f]{40})$ ]]; then
  echo 'Expected deploy followed by a full Git revision.' >&2
  exit 2
fi

revision=${BASH_REMATCH[1]}
base=/opt/wzq
target="releases/$revision"
exec 9>"$base/.deploy.lock"
flock -w 60 9

stage=$(mktemp -d "$base/releases/.upload.XXXXXXXX")
temp_link="$base/.current-$revision"
trap 'rm -rf -- "$stage"; rm -f -- "$temp_link"' EXIT

python3 -c '
import io
from pathlib import PurePosixPath
import sys
import tarfile

limit = 32 * 1024 * 1024
payload = sys.stdin.buffer.read(limit + 1)
if len(payload) > limit:
    sys.exit("Compressed release is too large")

with tarfile.open(fileobj=io.BytesIO(payload), mode="r:gz") as archive:
    total_size = 0
    for index, entry in enumerate(archive):
        if index >= 2000:
            sys.exit("Too many release files")
        path = PurePosixPath(entry.name)
        if path.is_absolute() or ".." in path.parts or not (entry.isfile() or entry.isdir()):
            sys.exit("Invalid release archive entry")
        total_size += entry.size
        if total_size > limit:
            sys.exit("Uncompressed release is too large")
        archive.extract(entry, path=sys.argv[1], filter="data")
' "$stage"

test -s "$stage/bin/wzq-server"
test -x "$stage/bin/wzq-server"
chmod 0755 "$stage"
if [[ ! -d "$base/$target" ]]; then
  mv -T "$stage" "$base/$target"
fi

previous=$(readlink "$base/current" || true)
activate() {
  ln -sfn "$1" "$temp_link"
  mv -Tf "$temp_link" "$base/current"
}

healthy() {
  for attempt in {1..20}; do
    if curl --silent --fail --max-time 1 http://127.0.0.1:3041/healthz >/dev/null; then
      return 0
    fi
    sleep 1
  done
  return 1
}

if [[ "$previous" == "$target" ]] && healthy; then
  printf '%s is already running. Health check passed.\n' "$revision"
  exit 0
fi

activate "$target"
if sudo -n /usr/bin/systemctl restart wzq.service && healthy; then
  printf 'Deployed %s. Health check passed.\n' "$revision"
  exit 0
fi

if [[ -n "$previous" && "$previous" != "$target" ]]; then
  activate "$previous"
  if sudo -n /usr/bin/systemctl restart wzq.service && healthy; then
    printf 'Deployment failed. Restored %s.\n' "$previous" >&2
  else
    printf 'Deployment failed. Health check also failed after restoring %s.\n' "$previous" >&2
  fi
else
  printf 'Health check failed for %s.\n' "$revision" >&2
  if [[ -z "$previous" ]]; then
    rm -f -- "$base/current"
    sudo -n /usr/bin/systemctl restart wzq.service || true
  fi
fi
exit 1
