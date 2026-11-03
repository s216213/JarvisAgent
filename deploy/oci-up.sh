#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export DOCKER_BUILDKIT=1
export COMPOSE_DOCKER_CLI_BUILD=1
docker compose build core caddy
docker compose up -d --no-build core
docker compose up -d --no-build --force-recreate caddy searxng
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if docker compose exec -T core python3 -c "import urllib.request; urllib.request.urlopen('http://127.0.0.1:8080/health', timeout=3).read()"; then
    exit 0
  fi
  sleep 2
done
echo "rdzeń nie odpowiedział na /health" >&2
exit 1
