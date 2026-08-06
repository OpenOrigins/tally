ARG RUST_IMAGE=public.ecr.aws/docker/library/rust:1.89-slim-bookworm@sha256:d7fc7de78bb8c1469933aeecbf801314d30d7d6e9f0578bba4cfa285bfa37fe6
ARG RUNTIME_IMAGE=public.ecr.aws/docker/library/debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241

FROM ${RUST_IMAGE} AS audit-builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY codex-audit/Cargo.toml codex-audit/Cargo.toml
COPY claude-audit/Cargo.toml claude-audit/Cargo.toml
COPY codex-audit/src codex-audit/src
COPY claude-audit/src claude-audit/src
RUN cargo build --locked --release --package tally-codex

FROM ${RUNTIME_IMAGE}

ARG USERNAME=codex
ARG USER_UID=1000
ARG USER_GID=1000
ARG CODEX_RELEASE=0.146.1

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
RUN curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 CODEX_RELEASE="${CODEX_RELEASE}" sh \
  && codex --version | grep -F "${CODEX_RELEASE}"

COPY --from=audit-builder --chown=${USERNAME}:${USERNAME} /build/target/release/tally-codex /opt/tally-codex/bin/tally-codex
COPY --chown=${USERNAME}:${USERNAME} codex-container/bin/entrypoint.sh /opt/tally-codex/bin/entrypoint.sh
COPY --chown=${USERNAME}:${USERNAME} codex-container/codex /opt/tally-codex/codex

RUN chmod +x /opt/tally-codex/bin/*

WORKDIR /workspace

LABEL org.opencontainers.image.title="Tally Codex Audit Wrapper" \
      org.opencontainers.image.description="Codex CLI with local Tally audit hooks" \
      org.opencontainers.image.source="https://github.com/OpenOrigins/tally" \
      org.opencontainers.image.licenses="NOASSERTION"

ENTRYPOINT ["/usr/bin/tini", "--", "/opt/tally-codex/bin/entrypoint.sh"]
CMD ["codex"]
