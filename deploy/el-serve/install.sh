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
PROFILE_SET=0
FROM_SOURCE=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo) REPO="$2"; shift 2 ;;
    --branch) BRANCH="$2"; shift 2 ;;
    --project) PROJECT="$2"; shift 2 ;;
    --listen) LISTEN="$2"; shift 2 ;;
    --profile) PROFILE="$2"; PROFILE_SET=1; shift 2 ;;
    --from-source) FROM_SOURCE=1; shift ;;
    *) echo "unknown flag $1"; exit 2 ;;
  esac
done
[[ -n "$PROJECT" ]] || { echo "--project <dir holding el/> is required"; exit 2; }

apt-get update -qq
apt-get install -y -qq curl ca-certificates git >/dev/null

# Prebuilt binaries from the latest el-v* release (seconds), unless
# --from-source was asked for or no asset exists for this platform.
ARCH=$(uname -m)
case "$ARCH" in x86_64|amd64) ARCH=x86_64 ;; aarch64|arm64) ARCH=aarch64 ;; esac
ASSET="zdbt-el-linux-${ARCH}.tar.gz"
DOWNLOADED=0
if [[ "$FROM_SOURCE" != 1 ]]; then
  echo "==> looking for a released binary ($ASSET)"
  API="https://api.github.com/repos/${REPO#https://github.com/}/releases"
  # A network hiccup must not turn into a 40-minute source build: only a
  # release that has no asset for this platform falls back to building.
  if ! LISTING=$(curl -fsSL --retry 3 "$API"); then
    echo "could not query $API — check the server's network/proxy and retry, or pass --from-source"
    exit 1
  fi
  URL=$(printf '%s' "$LISTING" | grep -o "https://[^\"]*/el-v[^\"]*/${ASSET}" | head -1 || true)
  if [[ -z "$URL" ]]; then
    echo "==> no released binary for linux-$ARCH — building from source"
  else
    curl -fsSL --retry 3 "$URL" -o "/tmp/$ASSET" && curl -fsSL --retry 3 "$URL.sha256" -o "/tmp/$ASSET.sha256" \
      || { echo "download of $ASSET failed — retry, or pass --from-source"; exit 1; }
    # The checksum published beside the asset must match — a truncated or
    # tampered download never gets installed.
    (cd /tmp && sha256sum -c --quiet "$ASSET.sha256") || { echo "checksum mismatch for $ASSET"; exit 1; }
    tar -xzf "/tmp/$ASSET" -C /tmp zdbt-el-serve zdbt-el-worker
    install -m 0755 /tmp/zdbt-el-serve "$PREFIX/zdbt-el-serve"
    install -m 0755 /tmp/zdbt-el-worker "$PREFIX/zdbt-el-worker"
    rm -f "/tmp/$ASSET" "/tmp/$ASSET.sha256" /tmp/zdbt-el-serve /tmp/zdbt-el-worker
    DOWNLOADED=1
    echo "==> installed $ASSET ($(basename "$(dirname "$URL")"))"
  fi
fi

if [[ "$DOWNLOADED" != 1 ]]; then
  echo "==> build dependencies"
  apt-get install -y -qq build-essential cmake pkg-config libssl-dev python3 >/dev/null
  # standalone.py reads TOML: 3.11+ has tomllib; older Pythons need tomli.
  python3 -c 'import tomllib' 2>/dev/null || apt-get install -y -qq python3-tomli >/dev/null \
    || { apt-get install -y -qq python3-pip >/dev/null && python3 -m pip install -q tomli; }
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

  echo "==> standalone workspace (only the EL crates — none of the IDE's dependencies)"
  EL=/opt/zdbt-el
  python3 "$SRC/deploy/el-serve/standalone.py" "$SRC" "$EL"

  echo "==> build (this takes a while: polars + duckdb)"
  ( cd "$EL" && cargo build --release -p el_serve -p el_worker )
  install -m 0755 "$EL/target/release/zdbt-el-serve" "$PREFIX/zdbt-el-serve"
  install -m 0755 "$EL/target/release/zdbt-el-worker" "$PREFIX/zdbt-el-worker"
fi

# The systemd unit template ships with the release too; fetch it if we
# did not clone.
SRC=${SRC:-/opt/zdbt-src}
if [[ ! -f "$SRC/deploy/el-serve/zdbt-el-serve.service" ]]; then
  mkdir -p "$SRC/deploy/el-serve"
  curl -fsSL "https://raw.githubusercontent.com/${REPO#https://github.com/}/${BRANCH}/deploy/el-serve/zdbt-el-serve.service" \
    -o "$SRC/deploy/el-serve/zdbt-el-serve.service"
fi

echo "==> service user, dirs, env"
id -u zdbt >/dev/null 2>&1 || useradd --system --home "$PROJECT" --shell /usr/sbin/nologin zdbt
mkdir -p "$PROJECT/el/.zdbt" /etc/zdbt-el-serve
chown -R zdbt:zdbt "$PROJECT"
# Replaces one variable in the env file, keeping everything else the
# operator put there.
set_env() {
  { grep -v "^$1=" /etc/zdbt-el-serve/env || true; echo "$1=$2"; } > /etc/zdbt-el-serve/env.new
  mv /etc/zdbt-el-serve/env.new /etc/zdbt-el-serve/env
}
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
# ORACLE_USER=zdbt
# ORACLE_PASSWORD=a-password
ENV
else
  # Re-install: keep the operator's env (database URLs) but apply what this
  # run brought — the fresh token `zdbt el install-remote` just placed, and
  # an explicit --profile.
  [[ -f /etc/zdbt-el-serve/token ]] && set_env ZDBT_EL_TOKEN "$(head -1 /etc/zdbt-el-serve/token)"
  [[ "$PROFILE_SET" == 1 ]] && set_env ZDBT_EL_PROFILE "$PROFILE"
fi
# The pre-placed token is consumed — a later manual re-run keeps the env as is.
rm -f /etc/zdbt-el-serve/token
# The daemon runs as zdbt and reads this file itself (the launcher sources
# it); root-only would leave it token-less. Readable by root and zdbt only.
chown root:zdbt /etc/zdbt-el-serve/env
chmod 0640 /etc/zdbt-el-serve/env

# A launcher that works with or without systemd (containers have none).
cat > "$PREFIX/zdbt-el-serve-start" <<START
#!/usr/bin/env bash
set -a; source /etc/zdbt-el-serve/env; set +a
exec "$PREFIX/zdbt-el-serve" --project "$PROJECT" --listen "$LISTEN" --insecure-http \
  --worker "$PREFIX/zdbt-el-worker" \$ZDBT_EL_SERVE_FLAGS
START
chmod 0755 "$PREFIX/zdbt-el-serve-start"

if [[ -d /run/systemd/system ]] && systemctl --version >/dev/null 2>&1; then
  echo "==> systemd unit"
  sed -e "s|@PROJECT@|$PROJECT|g" -e "s|@LISTEN@|$LISTEN|g" -e "s|@PREFIX@|$PREFIX|g" \
    "$SRC/deploy/el-serve/zdbt-el-serve.service" > /etc/systemd/system/zdbt-el-serve.service
  systemctl daemon-reload
  systemctl enable zdbt-el-serve >/dev/null 2>&1 || true
  # restart, not start: a re-install must pick up the new binary and env
  systemctl restart zdbt-el-serve || true
  START_HINT="systemctl status zdbt-el-serve   &&   journalctl -fu zdbt-el-serve"
else
  echo "==> no systemd here (container?) — starting the daemon in the background"
  chown -R zdbt:zdbt "$PROJECT"
  pkill -x zdbt-el-serve >/dev/null 2>&1 || true; sleep 1
  nohup runuser -u zdbt -- "$PREFIX/zdbt-el-serve-start" > /var/log/zdbt-el-serve.log 2>&1 &
  sleep 2
  START_HINT="tail -f /var/log/zdbt-el-serve.log   (restart: zdbt-el-serve-start)"
fi

cat <<DONE

Installed and started. Next:
  1. /etc/zdbt-el-serve/env holds the token, the profile and the database URL
     slots — fill the URLs your el/connections.yml references (Oracle
     needs only its user, password and connect string: no client software).
  2. Put el/connections.yml (with its profiles) under $PROJECT/el/, then restart.
  3. TLS: set ZDBT_EL_SERVE_FLAGS="--tls-cert /path/cert.pem --tls-key /path/key.pem"
     in /etc/zdbt-el-serve/env and drop --insecure-http from the launcher, or keep
     it and terminate TLS at your reverse proxy.
  4. Logs: $START_HINT
  5. In the IDE the server is declared already (install-remote) — otherwise
     Remotes + -> https://<host>:7431 with token variable ZDBT_EL_TOKEN.
DONE
