FROM debian:bookworm-slim

ARG USERNAME=codex
ARG USER_UID=1000
ARG USER_GID=1000

ENV DEBIAN_FRONTEND=noninteractive
ENV CODEX_HOME=/codex-home
ENV TALLY_LOG_ROOT=/var/log/tally-codex
ENV TALLY_WORKSPACE=/workspace
ENV CODEX_NON_INTERACTIVE=1
ENV CODEX_INSTALL_DIR=/home/codex/.local/bin
ENV PATH=/home/codex/.local/bin:${PATH}

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
    python3 \
    python3-venv \
    ripgrep \
    tini \
    tzdata \
  && rm -rf /var/lib/apt/lists/*

RUN groupadd --gid "${USER_GID}" "${USERNAME}" \
  && useradd --uid "${USER_UID}" --gid "${USER_GID}" --create-home --shell /bin/bash "${USERNAME}" \
  && mkdir -p /workspace /codex-home /var/log/tally-codex /opt/tally-codex "/home/${USERNAME}/.local/bin" \
  && chown -R "${USERNAME}:${USERNAME}" /workspace /codex-home /var/log/tally-codex /opt/tally-codex "/home/${USERNAME}/.local"

USER ${USERNAME}
RUN curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 sh

COPY --chown=${USERNAME}:${USERNAME} bin /opt/tally-codex/bin
COPY --chown=${USERNAME}:${USERNAME} codex /opt/tally-codex/codex
COPY --chown=${USERNAME}:${USERNAME} hooks /opt/tally-codex/hooks
COPY --chown=${USERNAME}:${USERNAME} lib /opt/tally-codex/lib
COPY --chown=${USERNAME}:${USERNAME} monitors /opt/tally-codex/monitors

RUN chmod +x /opt/tally-codex/bin/* /opt/tally-codex/hooks/*.py /opt/tally-codex/monitors/*.py

WORKDIR /workspace

ENTRYPOINT ["/usr/bin/tini", "--", "/opt/tally-codex/bin/entrypoint.sh"]
CMD ["codex"]
