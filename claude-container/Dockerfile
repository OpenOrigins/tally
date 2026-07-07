FROM rust:1.89-slim-bookworm AS audit-builder

WORKDIR /build/claude-audit
COPY claude-audit/Cargo.toml claude-audit/Cargo.lock ./
COPY claude-audit/src ./src
RUN cargo build --release --bin tally-claude

FROM debian:bookworm-slim

ARG USERNAME=claude
ARG USER_UID=1000
ARG USER_GID=1000
ARG NODE_MAJOR=22

ENV DEBIAN_FRONTEND=noninteractive
ENV TALLY_LOG_ROOT=/var/log/tally-claude
ENV TALLY_WORKSPACE=/workspace
ENV TALLY_AGENT_ID=claude-container
ENV TALLY_AGENT_VERSION=claude-code-cli
ENV PATH=/opt/tally-claude/bin:${PATH}

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    bash \
    ca-certificates \
    coreutils \
    curl \
    findutils \
    git \
    gnupg \
    jq \
    less \
    ripgrep \
    tini \
    tzdata \
  && curl -fsSL "https://deb.nodesource.com/setup_${NODE_MAJOR}.x" | bash - \
  && apt-get install -y --no-install-recommends nodejs \
  && rm -rf /var/lib/apt/lists/*

# Official install method for the Claude Code CLI (npm), see:
# https://code.claude.com/docs/en/quickstart
RUN npm install -g @anthropic-ai/claude-code

RUN groupadd --gid "${USER_GID}" "${USERNAME}" \
  && useradd --uid "${USER_UID}" --gid "${USER_GID}" --create-home --shell /bin/bash "${USERNAME}" \
  && mkdir -p /workspace /var/log/tally-claude /opt/tally-claude/bin /opt/tally-claude/claude \
  && chown -R "${USERNAME}:${USERNAME}" /workspace /var/log/tally-claude /opt/tally-claude "/home/${USERNAME}"

USER ${USERNAME}

COPY --from=audit-builder --chown=${USERNAME}:${USERNAME} /build/claude-audit/target/release/tally-claude /opt/tally-claude/bin/tally-claude
COPY --chown=${USERNAME}:${USERNAME} claude-container/bin/entrypoint.sh /opt/tally-claude/bin/entrypoint.sh
COPY --chown=${USERNAME}:${USERNAME} claude-container/claude /opt/tally-claude/claude

RUN chmod +x /opt/tally-claude/bin/*

WORKDIR /workspace

ENTRYPOINT ["/usr/bin/tini", "--", "/opt/tally-claude/bin/entrypoint.sh"]
CMD ["claude"]
