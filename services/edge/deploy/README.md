# Deploying Encore Edge

This is the container deployment for the production host: one small Ubuntu VPS
that already runs Caddy, a password manager (`vaultwarden`) and `sms-relay` in
Docker. Encore edge runs as the container `encore-edge`, built from
`/opt/encore-edge`, with the compose file at
`/opt/encore-edge/deploy/docker-compose.yml` and a `mem_limit` of 420 MB.
Nothing is published on the host; Caddy reaches the container by name over the
existing Docker network. Nothing in this directory contains a secret.

## 1. Host packages

Only Docker and its compose plugin are needed on the host. Node, `ffmpeg` and
`yt-dlp` live in the image. Use Docker's own repository, or Ubuntu's packages:

```sh
sudo apt update
sudo apt install -y docker.io docker-compose-v2
docker compose version
```

Caddy is already running in Docker on this host. The compose file joins the
external network `vaultwarden_default`; adjust that name if Caddy's network is
called something else.

## 2. Application

Copy `services/edge` to `/opt/encore-edge`. Copying over an existing directory
updates the code without deleting `deploy/edge.env`:

```sh
sudo install -d -m 0755 /opt/encore-edge
sudo cp -a services/edge/. /opt/encore-edge/
```

The Dockerfile installs the production Node dependencies, `ffmpeg` and the
standalone `yt-dlp` binary. Keep `node_modules` out of git.

## 3. Environment

The compose file reads `/opt/encore-edge/deploy/edge.env`. Generate two
independent secrets and write them only to that file:

```sh
sudo install -d -m 0755 -o root -g root /opt/encore-edge/deploy
sudo tee /opt/encore-edge/deploy/edge.env >/dev/null <<'EOF'
EDGE_TOKEN_SECRET=REPLACE_WITH_64_HEX_CHARACTERS
EDGE_ADMIN_TOKEN=REPLACE_WITH_ANOTHER_INDEPENDENT_SECRET
EDGE_ALLOWED_ORIGINS=https://encore.senni.dev
EDGE_CACHE_MAX_BYTES=10737418240
EDGE_UPLOAD_USER_MAX_BYTES=5368709120
EDGE_UPLOAD_FILE_MAX_BYTES=536870912
EDGE_SYNC_USER_MAX_BYTES=4194304
EDGE_LOG_LEVEL=info
EOF
sudo chmod 0600 /opt/encore-edge/deploy/edge.env
```

`EDGE_TOKEN_SECRET` must match the secret configured on the Vercel group
service. Both sides mint the same `e1.<payload>.<hmac>` token format with
`signToken`/`mintToken` from `src/tokens.mjs`. A token secret shorter than 32
characters makes the service exit at startup, by design.

The image already sets `EDGE_DATA_DIR=/data`, `EDGE_HOST=0.0.0.0` and
`EDGE_PORT=8787`. Do not set `EDGE_HOST=127.0.0.1` in the container: Caddy has
to reach it over the Docker network. The port is not published to the host.
Change `EDGE_ALLOWED_ORIGINS` to the exact browser origin(s) the phone uses.
Native clients send no `Origin` and are unaffected. No wildcard is accepted.

Do not commit the environment file. No secret is logged by the service.

## 4. Caddy

The site block is in [`Caddyfile`](./Caddyfile). It proxies to
`encore-edge:8787`, so Caddy and the edge container must be on the same Docker
network (the compose file handles the edge side).

If Caddy is already running, append the block to its existing Caddyfile **in
place**. That file is a single-file bind mount: replacing or renaming it leaves
the running container reading the old inode. Do not use an editor or command
that writes a new file and renames it over the old one.

```sh
CADDYFILE=/path/to/Caddyfile        # the host file bind-mounted into Caddy
CADDY_CONTAINER=caddy               # the running Caddy container

sudo tee -a "$CADDYFILE" >/dev/null <<'EOF'
edge.encore.senni.dev {
	request_body {
		max_size 1GB
	}

	reverse_proxy encore-edge:8787 {
		flush_interval -1
		transport http {
			read_timeout 0
			write_timeout 0
		}
	}

	log {
		output discard
	}

	header {
		Strict-Transport-Security "max-age=31536000; includeSubDomains"
		X-Content-Type-Options nosniff
		-Server
	}
}
EOF
```

Validate the whole file in a throwaway container, then ask the running Caddy
to read it again. Never restart Caddy or any other container:

```sh
CADDY_IMAGE=caddy:2                 # the image the running Caddy container uses
docker run --rm \
  -v "$CADDYFILE":/etc/caddy/Caddyfile:ro \
  "$CADDY_IMAGE" caddy validate --config /etc/caddy/Caddyfile

docker exec "$CADDY_CONTAINER" caddy reload --config /etc/caddy/Caddyfile
```

If the container's config path is not `/etc/caddy/Caddyfile`, use its actual
path in both commands. If validation fails, fix the file and reload again; do
not restart anything to recover.

Point the DNS name at the host and allow inbound 80/443 only. Caddy proxies
WebSocket upgrades and long media reads automatically. Keep compression off for
this host; the block also discards access logs because a media URL contains a
token. The health checks in `deploy.sh` and `upkeep.sh` run `wget` (or `curl`)
inside the Caddy container, so Caddy must be able to reach `encore-edge:8787`;
the official Caddy image includes `wget`.

Verify from outside:

```sh
curl -sS https://edge.encore.senni.dev/health
# {"v":1,"ok":true}

curl -sS -H "Authorization: Bearer $EDGE_ADMIN_TOKEN" \
  https://edge.encore.senni.dev/status
```

`/status` is disabled unless `EDGE_ADMIN_TOKEN` is set.

## 5. Deploy

[`deploy.sh`](./deploy.sh) builds the image from `/opt/encore-edge`, starts only
the `encore-edge` service, checks `/health` from the Caddy container, and puts
the previous image back if that check fails. It never restarts Caddy or any
other container.

```sh
cd /opt/encore-edge
sudo ./deploy/deploy.sh
```

The previous image is kept for rollback, so old images accumulate. Remove
unused ones by hand when the host has room:

```sh
docker image prune
```

The script can be pointed elsewhere with `ENCORE_EDGE_COMPOSE_FILE`,
`ENCORE_EDGE_PROJECT_DIR`, `ENCORE_EDGE_CONTAINER`, `ENCORE_EDGE_SERVICE` and
`ENCORE_EDGE_CADDY_CONTAINER`. It refuses to run if it cannot find the Caddy
container, because the health check would prove nothing.

## 6. Nightly upkeep

Install the timer once:

```sh
cd /opt/encore-edge
sudo cp deploy/encore-edge-upkeep.service deploy/encore-edge-upkeep.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now encore-edge-upkeep.timer
systemctl list-timers encore-edge-upkeep.timer
```

It runs at 04:30 with up to 30 minutes of jitter. [`upkeep.sh`](./upkeep.sh):

- updates `yt-dlp` inside the container with `docker exec -u root encore-edge
  yt-dlp -U`, and copies the previous binary back if the new one fails
  `yt-dlp --version`;
- checks the filesystem that holds Docker's data. Below 3 GiB free it deletes
  only cache media files under `/data/cache`, oldest first, until there is
  about 4 GiB free. It never touches `/data/uploads` or `/data/sync`, never a
  file written in the last ten minutes, and no other container. When it deleted
  anything it restarts `encore-edge` so the cache index matches the disk; a
  trimmed song is fetched again the next time it is played;
- probes `/health` twice, ten seconds apart. If both fail it restarts
  `encore-edge`, but at most three times in 24 hours. The count lives in
  `/var/lib/encore-edge-upkeep`.

Run it by hand to check the output:

```sh
sudo /opt/encore-edge/deploy/upkeep.sh
sudo journalctl -u encore-edge-upkeep.service --since today
```

Log lines name the container, byte counts and the step that ran. They never
contain a request URL, query string, token or file name.

## 7. Storage and backups

The compose file keeps state in the named volume `encore-edge-data` (Docker
prefixes it with the project name, usually `deploy_encore-edge-data`):

```
/data/sync/<user-hash>/playlists.json
/data/sync/<user-hash>/queue.json
/data/sync/<user-hash>/matches.json
/data/cache/<key>.m4a|mp3|ogg|flac|wav
/data/cache/last-played.json
/data/uploads/<owner-prefix>/<key>.<ext>
```

- `cache/` is disposable. Deleting it at any time is safe; entries are rebuilt
  from the directory on start and evicted least-recently-played when full.
- `uploads/` holds listeners' local files. It is never evicted. Back it up if
  the owner expects those files to survive a host loss.
- `sync/` holds playlists, queue state and match choices in small atomically
  written JSON files. Back it up with the uploads.

Find the volume and back up only the parts that are not disposable:

```sh
docker volume ls | grep encore-edge
docker run --rm \
  -v deploy_encore-edge-data:/data:ro \
  -v /var/backups:/backup \
  alpine tar czf "/backup/encore-edge-data-$(date +%F).tgz" -C /data uploads sync
```

The service writes JSON with a temp file, fsync and rename. A power cut leaves
either the old file or the new one, never half of one.

## 8. Security notes

- Do not add a `ports:` mapping. Caddy is the only public listener; the edge
  port stays inside the Docker network.
- `mem_limit: 420m` keeps a burst here from starving `vaultwarden` and the
  other containers on this 1 CPU, ~1 GB host. Check free memory before raising
  it.
- Use a different `EDGE_ADMIN_TOKEN` from the HMAC secret and from any group
  service credential. Keep `edge.env` readable only by root.
- Media tokens are short-lived and bound to one user and one key, but a URL
  containing one works for anyone who has it. Do not put media URLs in logs,
  screenshots or chat.
- The resolve allowlist accepts only YouTube video ids and public SoundCloud
  track URLs. It is not a general proxy; do not add arbitrary URL support.
- `yt-dlp -U` executes code downloaded from the network as root inside the
  container. Restrict who can edit the unit and review the timer's journal.
  The container itself runs as the unprivileged `node` user.
