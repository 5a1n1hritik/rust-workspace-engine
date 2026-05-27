# sandbox.Dockerfile
FROM rust:1.75-slim

# Safe low-privileged user without login shell access
RUN useradd -m -u 1001 -s /usr/sbin/nologin miller_jail

# Locked down isolated workspace geometry
RUN mkdir -p /workspace \
    && chown -R miller_jail:miller_jail /workspace \
    && chmod 700 /workspace

WORKDIR /workspace
USER miller_jail
ENV HOME=/home/miller_jail
