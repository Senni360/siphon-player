FROM node:24-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends ffmpeg ca-certificates curl \
  && rm -rf /var/lib/apt/lists/* \
  && curl -fsSL -o /usr/local/bin/yt-dlp https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux \
  && chmod 755 /usr/local/bin/yt-dlp

WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --omit=dev
COPY src ./src

RUN mkdir -p /data && chown node:node /data
USER node
# mem_limit is the container budget; the V8 heap must stay below it so a full
# GC happens instead of the cgroup OOM killer. yt-dlp and ffmpeg run inside the
# same budget, so leave ~120 MB for them.
ENV EDGE_DATA_DIR=/data EDGE_HOST=0.0.0.0 EDGE_PORT=8787 \
  NODE_OPTIONS="--max-old-space-size=224 --max-semi-space-size=8" \
  UV_THREADPOOL_SIZE=8
EXPOSE 8787
CMD ["node", "src/index.mjs"]
