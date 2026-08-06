ARG RUST_IMAGE=public.ecr.aws/docker/library/rust:1.89-slim-bookworm@sha256:d7fc7de78bb8c1469933aeecbf801314d30d7d6e9f0578bba4cfa285bfa37fe6
ARG RUNTIME_IMAGE=public.ecr.aws/docker/library/node:22.23.2-bookworm-slim@sha256:d649c27dae7ba0137b3cef5dd75baa422c08dc3d9e3fc0c23dfb172dc3cc6436

FROM ${RUST_IMAGE} AS audit-builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY codex-audit/Cargo.toml codex-audit/Cargo.toml
COPY claude-audit/Cargo.toml claude-audit/Cargo.toml
COPY codex-audit/src codex-audit/src
COPY claude-audit/src claude-audit/src
RUN cargo build --locked --release --package tally-claude

FROM ${RUNTIME_IMAGE}

ARG USERNAME=node
ARG CLAUDE_CODE_VERSION=2.1.223

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
    jq \
    less \
    ripgrep \
    tini \
    tzdata \
  && rm -rf /var/lib/apt/lists/*

# Official install method for the Claude Code CLI (npm), see:
# https://code.claude.com/docs/en/quickstart
RUN npm install -g "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
  && claude --version | grep -F "${CLAUDE_CODE_VERSION}"

RUN mkdir -p /workspace /var/log/tally-claude /opt/tally-claude/bin /opt/tally-claude/claude \
  && chown -R "${USERNAME}:${USERNAME}" /workspace /var/log/tally-claude /opt/tally-claude "/home/${USERNAME}"

USER ${USERNAME}

COPY --from=audit-builder --chown=${USERNAME}:${USERNAME} /build/target/release/tally-claude /opt/tally-claude/bin/tally-claude
COPY --chown=${USERNAME}:${USERNAME} claude-container/bin/entrypoint.sh /opt/tally-claude/bin/entrypoint.sh
COPY --chown=${USERNAME}:${USERNAME} claude-container/claude /opt/tally-claude/claude

RUN chmod +x /opt/tally-claude/bin/*

WORKDIR /workspace

LABEL org.opencontainers.image.title="Tally Claude Code Audit Wrapper" \
      org.opencontainers.image.description="Claude Code CLI with local Tally audit hooks" \
      org.opencontainers.image.source="https://github.com/OpenOrigins/tally" \
      org.opencontainers.image.licenses="NOASSERTION"

ENTRYPOINT ["/usr/bin/tini", "--", "/opt/tally-claude/bin/entrypoint.sh"]
CMD ["claude"]
