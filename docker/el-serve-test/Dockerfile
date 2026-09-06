# A throwaway "remote server" to test the SSH install wizard against:
# Ubuntu + sshd + a passwordless-sudo `deploy` user. Your public key is
# injected at start (AUTHORIZED_KEY env). No systemd — the installer
# falls back to a background launcher, exactly like a plain container.
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends openssh-server sudo curl ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/sshd \
    && useradd -m -s /bin/bash deploy \
    && echo 'deploy ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/deploy \
    && chmod 0440 /etc/sudoers.d/deploy \
    && sed -i 's/^#\?PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
EXPOSE 22 7431
ENTRYPOINT ["/entrypoint.sh"]
