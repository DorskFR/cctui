# Source from the repo root: `source site/demo/env.sh`. Creates tmp/mkt/.env on first use.
root="$(git rev-parse --show-toplevel)"
env_file="$root/tmp/mkt/.env"
if [[ ! -f "$env_file" ]]; then
	mkdir -p "$(dirname "$env_file")"
	(umask 077 && cat > "$env_file" <<EOT
CCTUI_ADMIN_TOKENS=$(openssl rand -hex 32)
CCTUI_VAULT_KEY=$(openssl rand -hex 32)
CCTUI_SERVER_PORT=8790
CCTUI_UI_PORT=8788
CCTUI_EXTERNAL_URL=http://localhost:8790
CCTUI_ALLOWED_ORIGINS=http://localhost:8788,http://127.0.0.1:8788
EOT
	)
fi
set -a; . "$env_file"; set +a
export COMPOSE_PROJECT_NAME=mkt-demo CCTUI_TOKEN="$CCTUI_ADMIN_TOKENS" CCTUI_API_URL=http://localhost:8790
