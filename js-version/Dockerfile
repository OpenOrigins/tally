# Claude Code requires Node >=22 (verified against the published package's
# engines field), and Node itself doubles as the runtime for the dummy mock
# Anthropic API server used in testing. Alpine (not Debian) here: Debian's
# CDN was serving a corrupted large package index (different bad hash on
# every retry) at build time; the claude-code installer explicitly ships a
# linux-x64-musl binary, so Alpine is a fully supported, unaffected path.
FROM node:22-alpine

# This build environment's egress is flaky (transient I/O errors on random
# packages from both apt and apk mirrors) — retry a few times before failing.
RUN for i in 1 2 3 4 5; do \
      apk add --no-cache bash jq curl ca-certificates && break; \
      echo "apk add failed (attempt $i), retrying..."; sleep 2; \
    done

# npm's own fetch of the ~75MB platform-native binary reliably fails on this
# egress (TLS record decryption errors partway through long transfers — see
# docker/fetch_native_binary.sh for the full diagnosis), leaving a ~500-byte
# stub. Install the small wrapper package via npm as normal, then fetch the
# native binary ourselves in retried chunks and hand it to install.cjs.
COPY docker/fetch_native_binary.sh /tmp/fetch_native_binary.sh
RUN chmod +x /tmp/fetch_native_binary.sh \
    && npm install -g @anthropic-ai/claude-code \
    && CC_VERSION="$(npm view @anthropic-ai/claude-code version)" \
    && /tmp/fetch_native_binary.sh @anthropic-ai/claude-code-linux-x64-musl "$CC_VERSION" \
         /usr/local/lib/node_modules/@anthropic-ai/claude-code-linux-x64-musl \
    && node /usr/local/lib/node_modules/@anthropic-ai/claude-code/install.cjs \
    && size=$(stat -c%s /usr/local/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe) \
    && [ "$size" -gt 1000000 ] || (echo "FATAL: claude native binary never installed" && exit 1) \
    && rm -f /tmp/fetch_native_binary.sh

# claude-code refuses --dangerously-skip-permissions as root ("cannot be
# used with root/sudo privileges") — needed here because the dummy test has
# no human to approve tool calls. Run everything as an unprivileged user.
RUN addgroup -S claude && adduser -S claude -G claude

WORKDIR /workspace

COPY .claude/ /workspace/.claude/
COPY hooks/ /workspace/hooks/
COPY test/ /workspace/test/

RUN chmod +x /workspace/hooks/*.sh /workspace/test/*.sh

ENV CLAUDE_HOOK_LOG_DIR=/var/log/claude-hooks
RUN mkdir -p /var/log/claude-hooks \
    && chown -R claude:claude /workspace /var/log/claude-hooks

USER claude

CMD ["bash", "/workspace/test/run_all_tests.sh"]
