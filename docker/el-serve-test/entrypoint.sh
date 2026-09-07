#!/usr/bin/env bash
set -euo pipefail
if [[ -z "${AUTHORIZED_KEY:-}" ]]; then
  echo "AUTHORIZED_KEY is not set — pass your public key (e.g. \$(cat ~/.ssh/id_ed25519.pub))" >&2
  exit 2
fi
install -d -m 0700 -o deploy -g deploy /home/deploy/.ssh
printf '%s\n' "$AUTHORIZED_KEY" > /home/deploy/.ssh/authorized_keys
chown deploy:deploy /home/deploy/.ssh/authorized_keys
chmod 0600 /home/deploy/.ssh/authorized_keys
ssh-keygen -A >/dev/null
# No systemd in here: once the installer has run, bring the daemon back on
# every container (re)start so a Docker restart does not leave the IDE's
# Remote tab with "connection refused".
if [[ -x /usr/local/bin/zdbt-el-serve-start ]] && id -u zdbt >/dev/null 2>&1; then
  nohup runuser -u zdbt -- /usr/local/bin/zdbt-el-serve-start > /var/log/zdbt-el-serve.log 2>&1 &
fi
exec /usr/sbin/sshd -D -e
