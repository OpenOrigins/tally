// Minimal dummy stand-in for the Anthropic Messages API.
//
// Not a real model: it inspects the incoming request and returns a scripted
// response so that `claude --print` produces a genuine tool-use turn (which
// organically fires PreToolUse/PostToolUse) followed by a stop turn (which
// fires Stop). No network call ever reaches api.anthropic.com.
const http = require('http');

const PORT = process.env.MOCK_PORT || 4141;

function send(res, status, body) {
  const json = JSON.stringify(body);
  res.writeHead(status, { 'Content-Type': 'application/json' });
  res.end(json);
}

function hasToolResult(messages) {
  return messages.some(
    (m) =>
      Array.isArray(m.content) &&
      m.content.some((block) => block.type === 'tool_result'),
  );
}

const server = http.createServer((req, res) => {
  let body = '';
  req.on('data', (chunk) => (body += chunk));
  req.on('end', () => {
    if (req.method !== 'POST') return send(res, 404, { error: 'not found' });

    // Claude Code calls /v1/messages?beta=true — strip the query string
    // before matching, or this always falls through to the empty-object
    // branch and the CLI reports "empty or malformed response".
    const path = req.url.split('?')[0];

    if (path.endsWith('/count_tokens')) {
      return send(res, 200, { input_tokens: 42 });
    }

    if (!path.endsWith('/messages')) {
      return send(res, 200, {});
    }

    let payload = {};
    try {
      payload = JSON.parse(body || '{}');
    } catch {
      // fall through with empty payload
    }

    const messages = payload.messages || [];
    const alreadyRanTool = hasToolResult(messages);

    const content = alreadyRanTool
      ? [{ type: 'text', text: 'Dummy task complete. Stopping now.' }]
      : [
          {
            type: 'tool_use',
            id: 'toolu_dummy_001',
            name: 'Bash',
            input: { command: 'echo hello-from-dummy-tool-call' },
          },
        ];

    const stop_reason = alreadyRanTool ? 'end_turn' : 'tool_use';

    send(res, 200, {
      id: 'msg_dummy_001',
      type: 'message',
      role: 'assistant',
      model: payload.model || 'claude-sonnet-5',
      content,
      stop_reason,
      stop_sequence: null,
      usage: { input_tokens: 10, output_tokens: 10 },
    });
  });
});

server.listen(PORT, '127.0.0.1', () => {
  console.log(`[mock-server] listening on http://127.0.0.1:${PORT}`);
});
