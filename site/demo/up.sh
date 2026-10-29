#!/usr/bin/env bash
# Start the isolated demo stack (UI :8788, API :8790) and load the demo data.
set -euo pipefail
source "$(git rev-parse --show-toplevel)/site/demo/env.sh"
docker compose -p "$COMPOSE_PROJECT_NAME" -f "$root/deploy/local/docker-compose.yaml" --env-file "$env_file" up -d
until curl -sf -o /dev/null "$CCTUI_API_URL/api/v1/version"; do sleep 1; done
bash "$root/site/demo/seed.sh"
