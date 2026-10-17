#!/usr/bin/env bash
# Build the images here, stream them to the Droplet over SSH, and restart the stack.
# Building locally avoids compiling Rust on a small Droplet (it can run out of memory).
#
# Usage (Git Bash on Windows works):
#   DEPLOY_HOST=root@203.0.113.10 ./deploy/deploy.sh
set -euo pipefail
cd "$(dirname "$0")/.."

: "${DEPLOY_HOST:?set DEPLOY_HOST, e.g. root@203.0.113.10}"
REMOTE_DIR=/opt/bomber
COMPOSE="docker compose -f deploy/compose.yaml -f deploy/compose.build.yaml"

echo "==> Building images"
# DOMAIN is only needed at run time on the server; any value satisfies compose here.
DOMAIN=build $COMPOSE build

echo "==> Shipping images to $DEPLOY_HOST"
docker save bomber-game:latest bomber-web:latest | gzip | ssh "$DEPLOY_HOST" 'gunzip | docker load'

echo "==> Updating compose file and restarting"
ssh "$DEPLOY_HOST" "mkdir -p $REMOTE_DIR"
scp deploy/compose.yaml "$DEPLOY_HOST:$REMOTE_DIR/compose.yaml"
ssh "$DEPLOY_HOST" "cd $REMOTE_DIR \
  && test -f .env || { echo 'Missing $REMOTE_DIR/.env — create it with DOMAIN=your.subdomain'; exit 1; } \
  && cd $REMOTE_DIR && docker compose up -d && docker image prune -f"

echo "==> Done. Health check:"
DOMAIN_ON_SERVER=$(ssh "$DEPLOY_HOST" "grep -E '^DOMAIN=' $REMOTE_DIR/.env | cut -d= -f2")
curl -fsS "https://$DOMAIN_ON_SERVER/healthz" && echo
