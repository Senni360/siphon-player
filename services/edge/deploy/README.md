# Deploying Encore Edge

This is an example deployment for one small VPS. The service listens on
loopback and Caddy terminates HTTPS. Nothing in this directory contains a
secret.

## 1. Host packages

Ubuntu/Debian, Node 24 or newer:

```sh
sudo apt update
sudo apt install -y ffmpeg caddy
# Install Node 24 from your preferred source, then check:
node --version   # v24.x
sudo useradd --system --home /var/lib/encore-edge --shell /usr/sbin/nologin encore-edge
```

Install `yt-dlp` as a standalone binary so the nightly updater can replace it:

```sh
sudo curl -L https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp \
  -o /usr/local/bin/yt-dlp
sudo chmod 0755 /usr/local/bin/yt-dlp
sudo /usr/local/bin/yt-dlp --version
```

`ffmpeg` is only used with `-c:a copy`, so no extra encoders are needed.

## 2. Application

```sh
sudo git clone <your-repository-url> /opt/encore-edge
cd /opt/encore-edge/services/edge
sudo npm ci --omit=dev
```

If you do not clone the whole repository, copy `services/edge` alone. Keep
`node_modules` out of git.

Create the state directory. Cache and uploads are both below it:

```sh
sudo install -d -o encore-edge -g encore-edge -m 0750 /var/lib/encore-edge
```

## 3. Environment

Generate two independent secrets and write them only to the environment file:

```sh
sudo install -d -m 0750 -o root -g encore-edge /etc/encore-edge
sudo tee /etc/encore-edge/edge.env >/dev/null <<'EOF'
EDGE_TOKEN_SECRET=REPLACE_WITH_64_HEX_CHARACTERS
EDGE_ADMIN_TOKEN=REPLACE_WITH_ANOTHER_INDEPENDENT_SECRET
EDGE_DATA_DIR=/var/lib/encore-edge
EDGE_HOST=127.0.0.1
EDGE_PORT=8787
EDGE_ALLOWED_ORIGINS=https://encore.senni.dev
EDGE_CACHE_MAX_BYTES=10737418240
EDGE_UPLOAD_USER_MAX_BYTES=5368709120
EDGE_UPLOAD_FILE_MAX_BYTES=536870912
EDGE_SYNC_USER_MAX_BYTES=4194304
EDGE_LOG_LEVEL=info
EOF
sudo chmod 0640 /etc/encore-edge/edge.env
sudo chown root:encore-edge /etc/encore-edge/edge.env
```

`EDGE_TOKEN_SECRET` must match the secret configured on the Vercel group
service. Both sides mint the same `e1.<payload>.<hmac>` token format with
`signToken`/`mintToken` from `src/tokens.mjs`. A token secret shorter than 32
characters makes the service exit at startup, by design.

Change `EDGE_ALLOWED_ORIGINS` to the exact browser origin(s) the phone uses.
Native clients send no `Origin` and are unaffected. No wildcard is accepted.

Do not commit the environment file. No secret is logged by the service.

## 4. systemd

```sh
sudo cp deploy/encore-edge.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now encore-edge
sudo systemctl status encore-edge
sudo journalctl -u encore-edge -f
```

The unit runs as `encore-edge`, keeps the filesystem read-only except the
state directory, and asks for a clean SIGTERM shutdown. Check that state
survives a restart:

```sh
sudo systemctl restart encore-edge
```

Log lines are JSON events with a route template and status code, never a
request URL or token.

## 5. Caddy

```sh
sudo cp deploy/Caddyfile /etc/caddy/Caddyfile
sudo caddy validate --config /etc/caddy/Caddyfile
sudo systemctl reload caddy
```

Point the DNS name at the host and allow inbound 80/443 only. Caddy proxies
WebSocket upgrades and long media reads automatically. Keep compression off
for this host; the example Caddyfile also discards access logs because a media
URL contains a token.

Verify from outside:

```sh
curl -sS https://edge.encore.senni.dev/health
# {"v":1,"ok":true}

curl -sS -H "Authorization: Bearer $EDGE_ADMIN_TOKEN" \
  https://edge.encore.senni.dev/status
```

`/status` is disabled unless `EDGE_ADMIN_TOKEN` is set.

## 6. Nightly `yt-dlp` update

```sh
sudo cp deploy/encore-edge-ytdlp-update.service /etc/systemd/system/
sudo cp deploy/encore-edge-ytdlp-update.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now encore-edge-ytdlp-update.timer
systemctl list-timers encore-edge-ytdlp-update.timer
```

The timer runs at 04:30 with up to 30 minutes of jitter. If `yt-dlp` was
installed with pip, edit the unit to run
`/opt/encore-edge/venv/bin/pip install --upgrade yt-dlp` instead. A failed
update only means the next resolution may use an older extractor; the service
does not restart.

## 7. Storage and backups

```sh
du -sh /var/lib/encore-edge/cache /var/lib/encore-edge/uploads /var/lib/encore-edge/sync
```

- `cache/` is disposable. Deleting it at any time is safe; entries are rebuilt
  from the directory on start and evicted least-recently-played when full.
- `uploads/` holds listeners' local files. It is never evicted. Back it up if
  the owner expects those files to survive a host loss.
- `sync/` holds playlists, queue state and match choices in small atomically
  written JSON files. Back it up with the uploads.

The service writes JSON with a temp file, fsync and rename. A power cut leaves
either the old file or the new one, never half of one.

## 8. Security notes

- Keep `EDGE_HOST=127.0.0.1`; Caddy is the only public listener.
- Use a different `EDGE_ADMIN_TOKEN` from the HMAC secret and from any group
  service credential.
- Media tokens are short-lived and bound to one user and one key, but a URL
  containing one works for anyone who has it. Do not put media URLs in logs,
  screenshots or chat.
- The resolve allowlist accepts only YouTube video ids and public SoundCloud
  track URLs. It is not a general proxy; do not add arbitrary URL support.
- Run the service as an unprivileged user and keep the state directory
  unreadable to other local users.
- `yt-dlp -U` executes code downloaded from the network as root. That is the
  usual arrangement for a standalone updater; restrict who can edit the unit
  and review the timer's journal.
