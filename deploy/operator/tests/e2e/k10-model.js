'use strict';
// The scripted model of the kind end-to-end of the tool catalogue (K10, k10-e2e.sh): an OpenAI-compatible chat completions server that plays a
// folder agent's model with no network and no key. It is run in the cluster by the coder's own image (Node, which that image has), from a
// ConfigMap the script makes of this file; nothing else is pulled for it. Standard library only. Nothing here is a secret.
//
// The person's message carries the namespace to look at, `[ns:<name>]`, and the name of a pod of that namespace, `[pod:<name>]`, and the model
// makes two turns:
//
//   1. no tool result yet   the tool call `kubernetes__pods_list_in_namespace { namespace: <ns> }` — the catalogue's Kubernetes server,
//                           reached through the ToolProvider named `kubernetes` (the platform prefixes a server's tools with `<name>__`)
//   2. one tool result      `K10-PROBE <ns> found <pod>` when the tool result holds the pod's name, `K10-PROBE <ns> missing` when it does not.
//                           The test is a plain substring one: the model does not parse the server's answer, so the answer's shape is the
//                           server's business, and the proof is that the pod's name reached the model at all.
//
// A request that carries neither marker (a title, a summary) is answered plainly, so that nothing waits for a tool.
//
// The journal keeps, per request, the tool results it carried (their content) and nothing else: no header of a request is kept, so the model key
// the agent sends is not here.
//
//   GET  /healthz              200 ok
//   GET  /v1/models            the one model the agent asks about
//   POST /v1/chat/completions  the turns above, streamed (server-sent events) when the request says stream, else one JSON body
//   GET  /journal              every request so far, as JSON
//   DELETE /journal            forget them
//
// Environment: PORT (default 8080).
const http = require('http');

const PORT = Number(process.env.PORT || 8080);
const MAX_BODY = 8 * 1024 * 1024;
const journal = [];

function textOf(content) {
  if (typeof content === 'string') return content;
  if (Array.isArray(content)) return content.map((p) => (p && typeof p.text === 'string' ? p.text : '')).join('');
  return '';
}

// The first value of `[<marker>:<value>]` in the first user message that carries it, or ''.
function marker(messages, name) {
  const re = new RegExp('\\[' + name + ':([a-zA-Z0-9][-a-zA-Z0-9.]*)\\]');
  for (const m of messages) {
    if (m && m.role === 'user') {
      const found = re.exec(textOf(m.content));
      if (found) return found[1];
    }
  }
  return '';
}

// The tool results of the request: each message of role `tool`, with its content.
function toolResults(messages) {
  return messages.filter((m) => m && m.role === 'tool').map((m) => textOf(m.content));
}

function turn(messages) {
  const ns = marker(messages, 'ns');
  const pod = marker(messages, 'pod');
  const results = toolResults(messages);
  if (!ns || !pod) {
    // Not a probe of this script (a title, a summary): a plain answer, so that nothing waits for a tool.
    return { step: -1, results, text: 'Nothing to do.' };
  }
  if (results.length === 0) {
    return { step: 1, results, call: { name: 'kubernetes__pods_list_in_namespace', args: { namespace: ns } } };
  }
  const seen = results[results.length - 1].includes(pod);
  return { step: 2, results, text: 'K10-PROBE ' + ns + ' ' + (seen ? 'found ' + pod : 'missing ' + pod) };
}

let seq = 0;
function completion(model, reply) {
  const base = { id: 'chatcmpl-k10-' + ++seq, object: 'chat.completion', created: Math.floor(Date.now() / 1000), model };
  const usage = { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 };
  if (reply.call) {
    return Object.assign(base, {
      choices: [{
        index: 0,
        message: {
          role: 'assistant',
          content: null,
          tool_calls: [{ id: 'k10-call-' + seq, type: 'function', function: { name: reply.call.name, arguments: JSON.stringify(reply.call.args) } }],
        },
        finish_reason: 'tool_calls',
      }],
      usage,
    });
  }
  return Object.assign(base, { choices: [{ index: 0, message: { role: 'assistant', content: reply.text }, finish_reason: 'stop' }], usage });
}

function stream(res, model, reply) {
  const id = 'chatcmpl-k10-' + ++seq;
  const created = Math.floor(Date.now() / 1000);
  const chunk = (delta, finish) => ({ id, object: 'chat.completion.chunk', created, model, choices: [{ index: 0, delta, finish_reason: finish }] });
  const frames = [];
  if (reply.call) {
    const args = JSON.stringify(reply.call.args);
    const cut = Math.max(1, Math.floor(args.length / 2));
    frames.push(chunk({ role: 'assistant', content: null, tool_calls: [{ index: 0, id: 'k10-call-' + seq, type: 'function', function: { name: reply.call.name, arguments: '' } }] }, null));
    frames.push(chunk({ tool_calls: [{ index: 0, function: { arguments: args.slice(0, cut) } }] }, null));
    frames.push(chunk({ tool_calls: [{ index: 0, function: { arguments: args.slice(cut) } }] }, null));
    frames.push(chunk({}, 'tool_calls'));
  } else {
    const cut = Math.max(1, Math.floor(reply.text.length / 2));
    frames.push(chunk({ role: 'assistant', content: '' }, null));
    frames.push(chunk({ content: reply.text.slice(0, cut) }, null));
    frames.push(chunk({ content: reply.text.slice(cut) }, null));
    frames.push(chunk({}, 'stop'));
  }
  frames.push({ id, object: 'chat.completion.chunk', created, model, choices: [], usage: { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 } });
  res.writeHead(200, { 'Content-Type': 'text/event-stream; charset=utf-8', 'Cache-Control': 'no-cache' });
  res.end(frames.map((f) => 'data: ' + JSON.stringify(f) + '\n\n').join('') + 'data: [DONE]\n\n');
}

function send(res, status, body, type) {
  const data = typeof body === 'string' ? body : JSON.stringify(body);
  res.writeHead(status, { 'Content-Type': type || 'application/json', 'Content-Length': Buffer.byteLength(data) });
  res.end(data);
}

const server = http.createServer((req, res) => {
  const path = req.url.split('?')[0].replace(/\/+$/, '');
  if (req.method === 'GET' && path === '/healthz') return send(res, 200, 'ok', 'text/plain');
  if (req.method === 'GET' && path === '/journal') return send(res, 200, journal);
  if (req.method === 'DELETE' && path === '/journal') {
    journal.length = 0;
    return send(res, 200, { cleared: true });
  }
  if (req.method === 'GET' && (path === '/v1/models' || path === '/models')) {
    return send(res, 200, { object: 'list', data: [{ id: 'k10-model', object: 'model' }] });
  }
  if (req.method === 'POST' && (path === '/v1/chat/completions' || path === '/chat/completions')) {
    const parts = [];
    let size = 0;
    req.on('data', (c) => {
      size += c.length;
      if (size > MAX_BODY) {
        res.writeHead(413);
        res.end();
        req.destroy();
        return;
      }
      parts.push(c);
    });
    req.on('end', () => {
      let body;
      try {
        body = JSON.parse(Buffer.concat(parts).toString('utf8'));
      } catch (_) {
        return send(res, 400, { error: { message: 'not JSON' } });
      }
      const messages = Array.isArray(body.messages) ? body.messages : [];
      const reply = turn(messages);
      journal.push({ step: reply.step, model: body.model || '', streamed: Boolean(body.stream), results: reply.results });
      if (body.stream) return stream(res, body.model || 'k10-model', reply);
      return send(res, 200, completion(body.model || 'k10-model', reply));
    });
    return undefined;
  }
  return send(res, 404, { error: { message: 'not found' } });
});

if (require.main === module) {
  server.listen(PORT, () => {
    // eslint-disable-next-line no-console
    console.log('k10-model listening on ' + PORT);
  });
}

module.exports = { turn, marker, toolResults, server };
