const http = require('http');

const port = Number(process.env.MOCK_PORT || 4141);

function send(response, status, body) {
  response.writeHead(status, { 'Content-Type': 'application/json' });
  response.end(JSON.stringify(body));
}

function hasToolResult(messages) {
  return messages.some(
    (message) =>
      Array.isArray(message.content) &&
      message.content.some((block) => block.type === 'tool_result'),
  );
}

const server = http.createServer((request, response) => {
  let body = '';
  request.on('data', (chunk) => {
    body += chunk;
  });
  request.on('end', () => {
    if (request.method !== 'POST') {
      send(response, 404, { error: 'not found' });
      return;
    }

    const path = request.url.split('?')[0];
    if (path.endsWith('/count_tokens')) {
      send(response, 200, { input_tokens: 42 });
      return;
    }
    if (!path.endsWith('/messages')) {
      send(response, 200, {});
      return;
    }

    let payload = {};
    try {
      payload = JSON.parse(body || '{}');
    } catch {
      // The client receives a valid response below even for malformed test input.
    }

    const ranTool = hasToolResult(payload.messages || []);
    const content = ranTool
      ? [{ type: 'text', text: 'Dummy task complete. Stopping now.' }]
      : [
          {
            type: 'tool_use',
            id: 'toolu_dummy_001',
            name: 'Bash',
            input: { command: 'echo hello-from-dummy-tool-call' },
          },
        ];

    send(response, 200, {
      id: 'msg_dummy_001',
      type: 'message',
      role: 'assistant',
      model: payload.model || 'claude-sonnet-5',
      content,
      stop_reason: ranTool ? 'end_turn' : 'tool_use',
      stop_sequence: null,
      usage: { input_tokens: 10, output_tokens: 10 },
    });
  });
});

server.listen(port, '127.0.0.1', () => {
  process.stdout.write(`mock Claude API listening on port ${port}\n`);
});
