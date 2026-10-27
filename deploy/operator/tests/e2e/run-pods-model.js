'use strict';
// The scripted model of the kind end-to-end of run pods (D15b, run-pods-e2e.sh): an OpenAI-compatible chat completions server that plays a
// coder's model with no network and no key. It is run in the cluster by the coder's own image (Node, which that image has), from a ConfigMap
// the script makes of this file; nothing else is pulled for it. Standard library only. Nothing here is a secret.
//
// A task says what the model is to do in the words of its first message:
//
//   [run:<name>]         the name of the run: it names the scratch project (iso-<name>) and the answer
//   [repo:<path>]        (optional) a repository of the coder's own volume (a bare repository, a local path) the run works in instead of a scratch project
//   [hold:<file>]        (optional) the command waits until that file exists in the place it runs in, and says so when it is released
//   [peer:<path>]        (optional, any number) something of another run, or of another repository: the command tries to list it
//
// and the model, whatever else the request holds, makes three turns:
//
//   1. no tool result yet     the tool call start_scratch { name: "iso-<name>" } (a scratch project: a directory of the run's own, no repository), or
//                             prepare_workspace { repo_url: <repo> } with a repository
//   2. one tool result        the tool call run_command { command: <the probe below> }, which runs where the coder's deployment runs a run's commands:
//                             in a pod of its own with run pods, in the coder's container without them
//   3. two tool results       the answer: "RUNPODS-PROBE <name>" and the probe's output, as the tool returned it
//
// The probe prints one `key=value` line per fact (the host name, the user, the directory, what /work, /work/workspaces and /work/git hold, the NAMES
// of the environment variables and never a value, for each peer whether it is there from here, and with a repository whether the mirror can be
// written and what git says), writes the same lines to /tmp/probe-<name>.out of the place it runs in, and with a hold waits for the file. The
// script reads the answer, the journal and that file.
//
//   GET  /healthz              200 ok
//   GET  /v1/models            the one model the coder asks about
//   POST /v1/chat/completions  the turns above, streamed (server-sent events) when the request says stream, else one JSON body
//   GET  /journal              every request so far, as JSON: the run, the turn, whether it streamed, the model asked for, and the tool results
//                              it carried (the content of each). No header of a request is kept: the key the coder sends is not here.
//   DELETE /journal            forget them
//
// Environment: PORT (default 8080). It is not a model: it does not read the request beyond the markers and the tool results.
const http = require('http');

const PORT = Number(process.env.PORT || 8080);
const MAX_BODY = 8 * 1024 * 1024;
const journal = [];

function textOf(content) {
  if (typeof content === 'string') return content;
  if (Array.isArray(content)) return content.map((p) => (p && typeof p.text === 'string' ? p.text : '')).join('');
  return '';
}

// The first message of the person that carries the run marker; the markers are read from that one text.
function task(messages) {
  for (const m of messages) {
    if (m && m.role === 'user') {
      const t = textOf(m.content);
      if (/\[run:[a-z0-9-]+\]/.test(t)) return t;
    }
  }
  return '';
}
const markers = (text, name) => Array.from(text.matchAll(new RegExp('\\[' + name + ':([^\\]\\s]+)\\]', 'g')), (m) => m[1]);
const marker = (text, name) => markers(text, name)[0] || '';

// The command of turn 2. A path comes from the task between single quotes (a marker holds no quote and no space, the regexp above).
function probe(name, hold, peers, repo) {
  const lines = [
    '{',
    '  echo "host=$(hostname)"',
    '  echo "uid=$(id -u)"',
    '  echo "pwd=$(pwd)"',
    '  echo "work=$(ls /work | tr \'\\n\' \' \')"',
    '  echo "workspaces=$(ls /work/workspaces | tr \'\\n\' \' \')"',
    '  echo "envnames=$(env | cut -d= -f1 | sort | tr \'\\n\' \' \')"',
  ];
  peers.forEach((peer, i) => {
    lines.push("  if [ -e '" + peer + "' ]; then echo \"peer" + i + "=present\"; else echo \"peer" + i + "=absent\"; fi");
    lines.push("  echo \"peer" + i + "-ls=$(ls -a '" + peer + "' 2>&1 | tr '\\n' ' ')\"");
  });
  if (repo) {
    lines.push('  echo "git=$(ls /work/git 2>&1 | tr \'\\n\' \' \')"');
    lines.push('  echo "mirrors=$(ls -d /work/git/*/*/*.git 2>&1 | tr \'\\n\' \' \')"');
    lines.push('  mirror=$(ls -d /work/git/*/*/*.git 2>/dev/null | head -n 1)');
    lines.push('  if [ -n "$mirror" ]; then echo "mirror-write=$(touch "$mirror/probe" 2>&1 | tr \'\\n\' \' \')"; else echo "mirror-write=no mirror"; fi');
    lines.push('  echo "status=$(git status --short --branch 2>&1 | head -n 1)"');
  }
  lines.push('} | tee /tmp/probe-' + name + '.out');
  if (hold) {
    lines.push("until [ -f '" + hold + "' ]; do sleep 1; done");
    lines.push('echo "released=' + name + '"');
  }
  return lines.join('\n');
}

function turn(body) {
  const messages = Array.isArray(body.messages) ? body.messages : [];
  const text = task(messages);
  const name = marker(text, 'run');
  const results = messages.filter((m) => m && m.role === 'tool').map((m) => textOf(m.content));
  if (!name) {
    // Not a run of this script (a title, a summary): a plain answer, so that nothing waits for a tool.
    return { name: '', step: -1, results, reply: { text: 'Nothing to do.' } };
  }
  const repo = marker(text, 'repo');
  if (results.length === 0) {
    const call = repo ? { name: 'prepare_workspace', args: { repo_url: repo } } : { name: 'start_scratch', args: { name: 'iso-' + name } };
    return { name, step: 1, results, reply: { call } };
  }
  if (results.length === 1) {
    const command = probe(name, marker(text, 'hold'), markers(text, 'peer'), repo);
    return { name, step: 2, results, reply: { call: { name: 'run_command', args: { command } } } };
  }
  return { name, step: 3, results, reply: { text: 'RUNPODS-PROBE ' + name + '\n' + results[results.length - 1] } };
}

let seq = 0;
function completion(model, reply) {
  const base = { id: 'chatcmpl-runpods-' + ++seq, object: 'chat.completion', created: Math.floor(Date.now() / 1000), model };
  const usage = { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 };
  if (reply.call) {
    return Object.assign(base, {
      choices: [{
        index: 0,
        message: {
          role: 'assistant',
          content: null,
          tool_calls: [{ id: 'rp-call-' + seq, type: 'function', function: { name: reply.call.name, arguments: JSON.stringify(reply.call.args) } }],
        },
        finish_reason: 'tool_calls',
      }],
      usage,
    });
  }
  return Object.assign(base, { choices: [{ index: 0, message: { role: 'assistant', content: reply.text }, finish_reason: 'stop' }], usage });
}

function stream(res, model, reply) {
  const id = 'chatcmpl-runpods-' + ++seq;
  const created = Math.floor(Date.now() / 1000);
  const chunk = (delta, finish) => ({ id, object: 'chat.completion.chunk', created, model, choices: [{ index: 0, delta, finish_reason: finish }] });
  const frames = [];
  if (reply.call) {
    const args = JSON.stringify(reply.call.args);
    const cut = Math.max(1, Math.floor(args.length / 2));
    frames.push(chunk({ role: 'assistant', content: null, tool_calls: [{ index: 0, id: 'rp-call-' + seq, type: 'function', function: { name: reply.call.name, arguments: '' } }] }, null));
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
    return send(res, 200, { object: 'list', data: [{ id: 'coding-model', object: 'model' }] });
  }
  if (req.method === 'POST' && (path === '/v1/chat/completions' || path === '/chat/completions')) {
    const parts = [];
    let size = 0;
    req.on('data', (c) => {
      size += c.length;
      if (size > MAX_BODY) req.destroy();
      else parts.push(c);
    });
    req.on('end', () => {
      let body;
      try {
        body = JSON.parse(Buffer.concat(parts).toString('utf8'));
      } catch (e) {
        return send(res, 400, { error: { message: 'invalid JSON' } });
      }
      const model = typeof body.model === 'string' ? body.model : 'coding-model';
      const t = turn(body);
      journal.push({ n: journal.length + 1, at: new Date().toISOString(), run: t.name, step: t.step, stream: body.stream === true, model, toolResults: t.results });
      if (body.stream === true) return stream(res, model, t.reply);
      return send(res, 200, completion(model, t.reply));
    });
    return undefined;
  }
  return send(res, 404, { error: { message: 'not found' } });
});

if (require.main === module) {
  server.listen(PORT, '0.0.0.0', () => process.stderr.write('run-pods-model: listening on :' + PORT + '\n'));
  // Stop at once: a client that keeps its connection alive (the coder's) must not hold the process until the kubelet kills it.
  for (const sig of ['SIGTERM', 'SIGINT']) {
    process.on(sig, () => {
      server.close(() => process.exit(0));
      if (typeof server.closeAllConnections === 'function') server.closeAllConnections();
    });
  }
}
module.exports = { turn, probe, marker, markers, task, server };
