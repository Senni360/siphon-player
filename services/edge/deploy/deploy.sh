#!/usr/bin/env bash
# Build and deploy the Encore edge image on the production host.
#
# The previous image is kept so a failed /health check can put it back. Only
# the encore-edge container is started or recreated; Caddy and every other
# container are left alone. No URL, query string or token is logged.
set -euo pipefail

CONTAINER="${ENCORE_EDGE_CONTAINER:-encore-edge}"
SERVICE="${ENCORE_EDGE_SERVICE:-encore-edge}"
CADDY_CONTAINER="${ENCORE_EDGE_CADDY_CONTAINER:-}"
COMPOSE_FILE="${ENCORE_EDGE_COMPOSE_FILE:-/opt/encore-edge/deploy/docker-compose.yml}"
PROJECT_DIR="${ENCORE_EDGE_PROJECT_DIR:-/opt/encore-edge}"
HEALTH_PORT="${ENCORE_EDGE_PORT:-8787}"
HEALTH_ATTEMPTS="${ENCORE_EDGE_HEALTH_ATTEMPTS:-20}"
HEALTH_DELAY="${ENCORE_EDGE_HEALTH_DELAY:-3}"

log() {
  printf '%s deploy: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*"
}

fail() {
  printf '%s deploy: error: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" >&2
}

compose() {
  docker compose -f "$COMPOSE_FILE" "$@"
}

caddy_container() {
  if [ -n "$CADDY_CONTAINER" ]; then
    printf '%s' "$CADDY_CONTAINER"
    return 0
  fi
  docker ps --format '{{.Names}}\t{{.Image}}' 2>/dev/null \
    | awk -F '\t' 'tolower($2) ~ /caddy/ { print $1; exit }'
}

# The health check runs from the Caddy container, because that is the path the
# phone uses. The official Caddy image has wget; curl is a fallback.
probe() {
  local body
  body="$(docker exec "$CADDY_CONTAINER" wget -q -T 5 -O - "http://${CONTAINER}:${HEALTH_PORT}/health" 2>/dev/null || true)"
  if [ -z "$body" ]; then
    body="$(docker exec "$CADDY_CONTAINER" curl -fsS --max-time 5 "http://${CONTAINER}:${HEALTH_PORT}/health" 2>/dev/null || true)"
  fi
  case "$body" in
    *'"ok":true'*) return 0 ;;
    *) return 1 ;;
  esac
}

wait_for_health() {
  local attempt=1
  while [ "$attempt" -le "$HEALTH_ATTEMPTS" ]; do
    if probe; then
      return 0
    fi
    sleep "$HEALTH_DELAY"
    attempt=$((attempt + 1))
  done
  return 1
}

command -v docker >/dev/null 2>&1 || {
  fail "docker is not installed"
  exit 1
}
[ -f "$COMPOSE_FILE" ] || {
  fail "compose file not found: ${COMPOSE_FILE}"
  exit 1
}
[ -d "$PROJECT_DIR" ] || {
  fail "project directory not found: ${PROJECT_DIR}"
  exit 1
}

CADDY_CONTAINER="$(caddy_container)"
if [ -z "$CADDY_CONTAINER" ]; then
  fail "no Caddy container found; set ENCORE_EDGE_CADDY_CONTAINER"
  exit 1
fi

previous_image="$(docker inspect -f '{{.Image}}' "$CONTAINER" 2>/dev/null || true)"
# The name compose will use matters for rollback, not the name the old
# container happened to be created with.
image_ref="$(compose config --images 2>/dev/null | head -n 1 || true)"
if [ -z "$image_ref" ]; then
  image_ref="$(docker inspect -f '{{.Config.Image}}' "$CONTAINER" 2>/dev/null || true)"
fi

rollback() {
  if [ -z "$previous_image" ]; then
    fail "rollback: there is no previous image; stopping ${CONTAINER}"
    docker stop "$CONTAINER" >/dev/null 2>&1 || true
    return 1
  fi
  if [ -z "$image_ref" ]; then
    fail "rollback: could not determine the image name"
    return 1
  fi
  log "rollback: restoring the previous image"
  docker tag "$previous_image" "$image_ref"
  if ! compose up -d --no-deps --force-recreate "$SERVICE"; then
    fail "rollback: could not start the previous image"
    return 1
  fi
  if wait_for_health; then
    log "rollback: the previous image is healthy"
    return 0
  fi
  fail "rollback: the previous image is not healthy"
  return 1
}

log "building ${SERVICE} from ${PROJECT_DIR}"
if ! compose build "$SERVICE"; then
  fail "build failed; the running container was not changed"
  exit 1
fi

log "starting ${SERVICE}"
if ! compose up -d --no-deps "$SERVICE"; then
  fail "start failed"
  rollback || true
  exit 1
fi

if wait_for_health; then
  log "health: ok"
  exit 0
fi

fail "health: failed after ${HEALTH_ATTEMPTS} attempts"
if rollback; then
  exit 1
fi
exit 1
