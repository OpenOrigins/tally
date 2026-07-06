FROM rust:1.89-slim-bookworm AS audit-builder

WORKDIR /build/codex-audit
COPY codex-audit/Cargo.toml codex-audit/Cargo.lock ./
COPY codex-audit/src ./src
RUN cargo build --release --bin tally-codex

FROM debian:bookworm-slim

ARG USERNAME=codex
ARG USER_UID=1000
ARG USER_GID=1000

ENV DEBIAN_FRONTEND=noninteractive
ENV CODEX_HOME=/codex-home
ENV TALLY_LOG_ROOT=/var/log/tally-codex
ENV TALLY_WORKSPACE=/workspace
ENV TALLY_AGENT_ID=codex-container
ENV TALLY_AGENT_VERSION=codex-cli
ENV CODEX_NON_INTERACTIVE=1
ENV CODEX_INSTALL_DIR=/home/codex/.local/bin
ENV PATH=/opt/tally-codex/bin:/home/codex/.local/bin:${PATH}

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    bash \
    ca-certificates \
    coreutils \
    curl \
    findutils \
    git \
    jq \
    less \
    ripgrep \
    tini \
    tzdata \
  && rm -rf /var/lib/apt/lists/*

RUN groupadd --gid "${USER_GID}" "${USERNAME}" \
  && useradd --uid "${USER_UID}" --gid "${USER_GID}" --create-home --shell /bin/bash "${USERNAME}" \
  && mkdir -p /workspace /codex-home /var/log/tally-codex /opt/tally-codex/bin /opt/tally-codex/codex "/home/${USERNAME}/.local/bin" \
  && chown -R "${USERNAME}:${USERNAME}" /workspace /codex-home /var/log/tally-codex /opt/tally-codex "/home/${USERNAME}/.local"

USER ${USERNAME}
RUN curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 sh

COPY --from=audit-builder --chown=${USERNAME}:${USERNAME} /build/codex-audit/target/release/tally-codex /opt/tally-codex/bin/tally-codex
COPY --chown=${USERNAME}:${USERNAME} codex-container/bin/entrypoint.sh /opt/tally-codex/bin/entrypoint.sh
COPY --chown=${USERNAME}:${USERNAME} codex-container/codex /opt/tally-codex/codex

RUN chmod +x /opt/tally-codex/bin/*

WORKDIR /workspace

ENTRYPOINT ["/usr/bin/tini", "--", "/opt/tally-codex/bin/entrypoint.sh"]
CMD ["codex"]
