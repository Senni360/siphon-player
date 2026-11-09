#!/usr/bin/env bash
# Nightly upkeep for the container-based Encore edge deployment.
#
# 1. Updates yt-dlp inside the container and puts the previous binary back if
#    the new one does not answer --version.
# 2. Trims only the disposable edge cache when the host's Docker filesystem has
#    less than 3 GiB free, then restarts the container so its cache index
#    matches what is left. Uploads and sync documents are never touched.
# 3. Restarts encore-edge when /health fails twice in a row, with a limit so a
#    broken image cannot restart forever.
#
# Nothing here restarts Caddy or any other container, and no URL, query string,
# token or file name is logged.
set -euo pipefail

CONTAINER="${ENCORE_EDGE_CONTAINER:-encore-edge}"
CADDY_CONTAINER="${ENCORE_EDGE_CADDY_CONTAINER:-}"
HEALTH_PORT="${ENCORE_EDGE_PORT:-8787}"
STATE_DIR="${ENCORE_EDGE_STATE_DIR:-/var/lib/encore-edge-upkeep}"

MIN_FREE_BYTES=$((3 * 1024 * 1024 * 1024))
TARGET_FREE_BYTES=$((4 * 1024 * 1024 * 1024))
MAX_RESTARTS=3
RESTART_WINDOW_SECONDS=$((24 * 60 * 60))

log() {
  printf '%s upkeep: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*"
}

fail() {
  printf '%s upkeep: error: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" >&2
}

command -v docker >/dev/null 2>&1 || {
  fail "docker is not installed"
  exit 1
}

container_running() {
  [ "$(docker inspect -f '{{.State.Running}}' "$CONTAINER" 2>/dev/null || true)" = "true" ]
}

caddy_container() {
  if [ -n "$CADDY_CONTAINER" ]; then
    printf '%s' "$CADDY_CONTAINER"
    return 0
  fi
  docker ps --format '{{.Names}}\t{{.Image}}' 2>/dev/null \
    | awk -F '\t' 'tolower($2) ~ /caddy/ { print $1; exit }'
}

# /health through the Caddy container proves the path the phone actually uses.
# If no Caddy container can be found, ask the edge container itself instead.
health() {
  local caddy body
  caddy="$(caddy_container)"
  if [ -n "$caddy" ]; then
    body="$(docker exec "$caddy" wget -q -T 5 -O - "http://${CONTAINER}:${HEALTH_PORT}/health" 2>/dev/null || true)"
    if [ -z "$body" ]; then
      body="$(docker exec "$caddy" curl -fsS --max-time 5 "http://${CONTAINER}:${HEALTH_PORT}/health" 2>/dev/null || true)"
    fi
    case "$body" in
      *'"ok":true'*) return 0 ;;
      *) return 1 ;;
    esac
  fi
  docker exec "$CONTAINER" node -e \
    "fetch('http://127.0.0.1:${HEALTH_PORT}/health').then(r => r.json()).then(j => process.exit(j.ok ? 0 : 1)).catch(() => process.exit(1))" \
    >/dev/null 2>&1
}

update_ytdlp() {
  if ! container_running; then
    log "yt-dlp: ${CONTAINER} is not running; skipped"
    return 0
  fi
  if ! docker exec -u root "$CONTAINER" test -x /usr/local/bin/yt-dlp >/dev/null 2>&1; then
    log "yt-dlp: no binary at /usr/local/bin/yt-dlp; skipped"
    return 0
  fi
  if ! docker exec -u root "$CONTAINER" cp -p /usr/local/bin/yt-dlp /usr/local/bin/yt-dlp.previous >/dev/null 2>&1; then
    log "yt-dlp: could not save the previous binary; skipped"
    return 0
  fi

  if docker exec -u root "$CONTAINER" yt-dlp -U >/dev/null 2>&1 \
    && docker exec -u root "$CONTAINER" yt-dlp --version >/dev/null 2>&1; then
    docker exec -u root "$CONTAINER" rm -f /usr/local/bin/yt-dlp.previous >/dev/null 2>&1 || true
    log "yt-dlp: updated"
    return 0
  fi

  if docker exec -u root "$CONTAINER" cp -p /usr/local/bin/yt-dlp.previous /usr/local/bin/yt-dlp >/dev/null 2>&1 \
    && docker exec -u root "$CONTAINER" yt-dlp --version >/dev/null 2>&1; then
    log "yt-dlp: update failed; restored the previous binary"
    return 0
  fi

  fail "yt-dlp: update failed and the previous binary did not verify"
  return 1
}

docker_root() {
  docker info --format '{{.DockerRootDir}}' 2>/dev/null || printf '/'
}

free_bytes() {
  df -P -B1 "$(docker_root)" 2>/dev/null | awk 'NR == 2 { print $4 }'
}

# Deletes only cache media files, oldest first, and never a file written in the
# last ten minutes. /data/uploads and /data/sync are outside the path below.
trim_cache() {
  local free need
  free="$(free_bytes)"
  if [ -z "$free" ]; then
    fail "disk: could not read free space"
    return 1
  fi
  if [ "$free" -ge "$MIN_FREE_BYTES" ]; then
    log "disk: ${free} bytes free; no trim needed"
    return 0
  fi
  if ! container_running; then
    fail "disk: ${free} bytes free and ${CONTAINER} is not running; cache not trimmed"
    return 1
  fi

  need=$((TARGET_FREE_BYTES - free))
  log "disk: ${free} bytes free; trimming up to ${need} bytes from the edge cache"
  trimmed="$(docker exec -e ENCORE_TRIM_NEED="$need" -i "$CONTAINER" sh -s <<'CONTAINER_SCRIPT'
set -eu
need="${ENCORE_TRIM_NEED:?}"
dir=/data/cache
[ -d "$dir" ] || { printf '0'; exit 0; }
list="$(mktemp)"
trap 'rm -f "$list"' EXIT
find "$dir" -maxdepth 1 -type f -regextype posix-extended \
  -regex '.*/c[0-9a-f]{56}\.(m4a|mp3|ogg|flac|wav)' -mmin +10 \
  -printf '%T@ %s %p\n' > "$list" 2>/dev/null || true
sort -n -o "$list" "$list" 2>/dev/null || true
freed=0
while read -r _mtime size file; do
  if [ "$freed" -ge "$need" ]; then
    break
  fi
  if rm -f -- "$file"; then
    freed=$((freed + size))
  fi
done < "$list"
printf '%s' "$freed"
CONTAINER_SCRIPT
)"
  case "$trimmed" in
    ''|*[!0-9]*)
      fail "disk: could not trim the edge cache"
      return 1
      ;;
  esac
  log "disk: trimmed ${trimmed} bytes from the edge cache"
  if [ "$trimmed" -gt 0 ]; then
    # The running service still has the deleted keys in its in-memory index.
    # A restart makes it rebuild from what is left, so no play gets a dead URL.
    log "disk: restarting ${CONTAINER} to rebuild the cache index"
    if ! docker restart "$CONTAINER" >/dev/null; then
      fail "disk: could not restart ${CONTAINER} after trimming"
      return 1
    fi
  fi

  free="$(free_bytes)"
  log "disk: ${free} bytes free after trimming the cache"
  [ -n "$free" ] && [ "$free" -ge "$MIN_FREE_BYTES" ]
}

restart_count() {
  local cutoff
  cutoff=$(( $(date +%s) - RESTART_WINDOW_SECONDS ))
  if [ ! -f "$STATE_DIR/restarts" ]; then
    printf '0'
    return 0
  fi
  awk -v cutoff="$cutoff" '$1 ~ /^[0-9]+$/ && $1 >= cutoff' "$STATE_DIR/restarts" | wc -l | tr -d ' '
}

record_restart() {
  local cutoff
  install -d -m 0750 "$STATE_DIR"
  cutoff=$(( $(date +%s) - RESTART_WINDOW_SECONDS ))
  {
    if [ -f "$STATE_DIR/restarts" ]; then
      awk -v cutoff="$cutoff" '$1 ~ /^[0-9]+$/ && $1 >= cutoff' "$STATE_DIR/restarts"
    fi
    date +%s
  } > "$STATE_DIR/restarts.new"
  mv "$STATE_DIR/restarts.new" "$STATE_DIR/restarts"
}

clear_restarts() {
  rm -f "$STATE_DIR/restarts" "$STATE_DIR/restarts.new"
}

# Two probes ten seconds apart. A single lost packet is not a restart.
check_health() {
  if health; then
    return 0
  fi
  sleep 10
  health
}

status=0

update_ytdlp || status=1
trim_cache || status=1

if check_health; then
  clear_restarts
  log "health: ok"
else
  if ! container_running; then
    fail "health: failed twice and ${CONTAINER} is not running; left to its restart policy"
    status=1
  else
    count="$(restart_count)"
    if [ "$count" -ge "$MAX_RESTARTS" ]; then
      fail "health: failed twice; ${count} restarts in the last 24 hours, not restarting again"
      status=1
    else
      log "health: failed twice; restarting ${CONTAINER}"
      if ! docker restart "$CONTAINER" >/dev/null; then
        fail "health: could not restart ${CONTAINER}"
        status=1
      else
        record_restart
        if check_health; then
          log "health: ok after restart"
        else
          fail "health: still failing after restart"
          status=1
        fi
      fi
    fi
  fi
fi

exit "$status"
