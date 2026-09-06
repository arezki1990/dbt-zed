#!/usr/bin/env bash
# Installs the zdbt EL daemon on a Linux server (Debian/Ubuntu): builds
# zdbt-el-serve + zdbt-el-worker from this repo, installs them, creates
# a service user, and writes a systemd unit.
#
#   curl -fsSL https://raw.githubusercontent.com/arezki1990/dbt-zed/el-spike/deploy/el-serve/install.sh | sudo bash -s -- \
#       --project /srv/el-project --repo https://github.com/arezki1990/dbt-zed --branch el-spike
#
# Afterwards: put ZDBT_EL_TOKEN / ZDBT_EL_PROFILE and the database
# credentials in /etc/zdbt-el-serve/env, add the TLS cert/key paths to
# the unit (or keep --insecure-http behind a TLS ingress), then
#   systemctl enable --now zdbt-el-serve
set -euo pipefail

REPO="https://github.com/arezki1990/dbt-zed"
BRANCH="el-spike"
PROJECT=""
LISTEN="0.0.0.0:7431"
PREFIX="/usr/local/bin"
PROFILE="prod"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo) REPO="$2"; shift 2 ;;
    --branch) BRANCH="$2"; shift 2 ;;
    --project) PROJECT="$2"; shift 2 ;;
    --listen) LISTEN="$2"; shift 2 ;;
    --profile) PROFILE="$2"; shift 2 ;;
    *) echo "unknown flag $1"; exit 2 ;;
  esac
done
[[ -n "$PROJECT" ]] || { echo "--project <dir holding el/> is required"; exit 2; }

echo "==> build dependencies"
apt-get update -qq
apt-get install -y -qq build-essential cmake pkg-config libssl-dev git curl ca-certificates >/dev/null
if ! command -v cargo >/dev/null; then
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env"
fi

echo "==> source"
SRC=/opt/zdbt-src
if [[ -d "$SRC/.git" ]]; then
  git -C "$SRC" fetch -q origin "$BRANCH" && git -C "$SRC" checkout -q "origin/$BRANCH"
else
  git clone -q --depth 1 --branch "$BRANCH" "$REPO" "$SRC"
fi

echo "==> build (this takes a while: polars + duckdb)"
( cd "$SRC" && cargo build --release -p el_serve -p el_worker )
install -m 0755 "$SRC/target/release/zdbt-el-serve" "$PREFIX/zdbt-el-serve"
install -m 0755 "$SRC/target/release/zdbt-el-worker" "$PREFIX/zdbt-el-worker"

echo "==> service user, dirs, env"
id -u zdbt >/dev/null 2>&1 || useradd --system --home "$PROJECT" --shell /usr/sbin/nologin zdbt
mkdir -p "$PROJECT/el/.zdbt" /etc/zdbt-el-serve
chown -R zdbt:zdbt "$PROJECT"
if [[ ! -f /etc/zdbt-el-serve/env ]]; then
  # `zdbt el install-remote` places the token beforehand (over stdin);
  # a manual install gets a fresh one.
  if [[ -f /etc/zdbt-el-serve/token ]]; then
    TOKEN=$(head -1 /etc/zdbt-el-serve/token)
  else
    TOKEN=$(openssl rand -hex 24 2>/dev/null || head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n')
  fi
  cat > /etc/zdbt-el-serve/env <<ENV
# Bearer token the IDE presents (the IDE's .env holds the same value)
ZDBT_EL_TOKEN=$TOKEN
# Environment this server runs by default (deploys can pin another)
ZDBT_EL_PROFILE=$PROFILE
# Database credentials referenced as \${VAR} in el/connections.yml
# EL_PG_URL_PROD=postgres://user:pass@host:5432/db
ENV
  chmod 0600 /etc/zdbt-el-serve/env
fi

echo "==> systemd unit"
sed -e "s|@PROJECT@|$PROJECT|g" -e "s|@LISTEN@|$LISTEN|g" -e "s|@PREFIX@|$PREFIX|g" \
  "$SRC/deploy/el-serve/zdbt-el-serve.service" > /etc/systemd/system/zdbt-el-serve.service
systemctl daemon-reload

cat <<DONE

Installed. Next:
  1. Edit /etc/zdbt-el-serve/env  (token, profile, database URLs)
  2. Put el/connections.yml (with its profiles) under $PROJECT/el/
  3. TLS: add  --tls-cert /path/cert.pem --tls-key /path/key.pem  to the ExecStart
     in /etc/systemd/system/zdbt-el-serve.service, or keep --insecure-http and
     terminate TLS at your reverse proxy.
  4. systemctl enable --now zdbt-el-serve   &&   journalctl -fu zdbt-el-serve
  5. In the IDE: Remotes +  ->  https://<host>:7431, token variable ZDBT_EL_TOKEN
     (same value in your local .env), then Deploy pipelines from their canvas.
DONE
