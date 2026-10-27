'use strict';
// What run-pods-model.js says, with no cluster and no coder (the kind end-to-end of run pods, D15b, depends on every line of it): the turns, the
// markers of a task, the command of the probe (valid shell), and the two shapes of an answer (JSON, and server-sent events a coder can assemble).
//
//   node deploy/operator/tests/e2e/run-pods-model.test.js        (Node 18 or later: fetch, node:test)
const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const { test } = require('node:test');
const { turn, probe, marker, markers, server } = require('./run-pods-model.js');

const ask = (text, ...toolResults) => ({
  model: 'coding-model',
  messages: [
    { role: 'system', content: 'You are a coder.' },
    { role: 'user', content: text },
    ...toolResults.flatMap((r, i) => [
      { role: 'assistant', content: null, tool_calls: [{ id: `c${i}`, type: 'function', function: { name: 'x', arguments: '{}' } }] },
      { role: 'tool', tool_call_id: `c${i}`, content: r },
    ]),
  ],
});

test('a message that carries no run marker gets a plain answer and no tool call', () => {
  const t = turn(ask('Say hi.'));
  assert.equal(t.name, '');
  assert.equal(t.step, -1);
  assert.deepEqual(t.reply, { text: 'Nothing to do.' });
});

test('the three turns of a run in a scratch project', () => {
  const text = '[run:a] [hold:/tmp/release-a] Look around.';
  const one = turn(ask(text));
  assert.equal(one.step, 1);
  assert.deepEqual(one.reply.call, { name: 'start_scratch', args: { name: 'iso-a' } });

  const two = turn(ask(text, 'Scratch project ready.\nslot: iso-a\npath: /work/workspaces/x/iso-a'));
  assert.equal(two.step, 2);
  assert.equal(two.reply.call.name, 'run_command');
  const command = two.reply.call.args.command;
  assert.match(command, /echo "host=\$\(hostname\)"/);
  assert.match(command, /echo "uid=\$\(id -u\)"/);
  assert.match(command, /echo "workspaces=\$\(ls \/work\/workspaces \| tr '\\n' ' '\)"/);
  assert.match(command, /\} \| tee \/tmp\/probe-a\.out\n/, 'the probe is written to the pod, by the name of the run');
  assert.match(command, /until \[ -f '\/tmp\/release-a' \]; do sleep 1; done\necho "released=a"$/, 'and waits for its release');
  assert.doesNotMatch(command, /peer|mirror/, 'a scratch run with no peer looks at no peer and no mirror');

  const three = turn(ask(text, 'Scratch project ready.', '$ cmd\nexit code 0\n--- output ---\nhost=adam-run-0123456789ab\n'));
  assert.equal(three.step, 3);
  assert.equal(three.reply.text, 'RUNPODS-PROBE a\n$ cmd\nexit code 0\n--- output ---\nhost=adam-run-0123456789ab\n');
});

test('a run with no hold does not wait', () => {
  const command = turn(ask('[run:plain] Look around.', 'ready')).reply.call.args.command;
  assert.doesNotMatch(command, /until|released/);
  assert.match(command, /tee \/tmp\/probe-plain\.out/);
});

test('peers are looked at in the order they are named, and a repository is prepared and probed', () => {
  const text = '[run:c] [repo:/work/seed/r1.git] [hold:/tmp/release-c] [peer:/work/workspaces/b/iso-b] [peer:/work/git/example.test/p/other.git] Work in /work/seed/r1.git.';
  assert.deepEqual(markers(text, 'peer'), ['/work/workspaces/b/iso-b', '/work/git/example.test/p/other.git']);
  assert.equal(marker(text, 'repo'), '/work/seed/r1.git');
  assert.equal(marker(text, 'nothing'), '');
  const one = turn(ask(text));
  assert.deepEqual(one.reply.call, { name: 'prepare_workspace', args: { repo_url: '/work/seed/r1.git' } });
  const command = turn(ask(text, 'Worktree ready.')).reply.call.args.command;
  const at = (needle) => command.indexOf(needle);
  assert.ok(at("peer0=present") > 0 && at("'/work/workspaces/b/iso-b'") > 0 && at("peer1=present") > at("peer0=present"));
  assert.match(command, /echo "git=\$\(ls \/work\/git /);
  assert.match(command, /echo "mirrors=\$\(ls -d \/work\/git\/\*\/\*\/\*\.git /);
  assert.match(command, /mirror-write=/);
  assert.match(command, /git status --short --branch/);
});

test('every command the model can send is valid shell', () => {
  const cases = [
    probe('a', '/tmp/release-a', [], ''),
    probe('b', '/tmp/release-b', ['/work/workspaces/id/iso-a'], ''),
    probe('c', '/tmp/release-c', ['/work/workspaces/id/iso-b', '/work/git/example.test/p/other.git'], '/work/seed/r1.git'),
    probe('plain', '', [], ''),
  ];
  for (const c of cases) {
    const r = spawnSync('sh', ['-n', '-c', c], { encoding: 'utf8' });
    assert.equal(r.status, 0, `${r.stderr}\n${c}`);
  }
});

test('the probe, run for real, prints one key=value line per fact and what a peer is', () => {
  const dir = require('node:fs').mkdtempSync(require('node:path').join(require('node:os').tmpdir(), 'rpm-'));
  try {
    const release = `${dir}/release`;
    require('node:fs').writeFileSync(release, '');
    const peer = `${dir}/peer`;
    const command = probe('t', release, [peer, `${dir}`], '').replace('/tmp/probe-t.out', `${dir}/probe.out`);
    const r = spawnSync('sh', ['-c', command], { encoding: 'utf8' });
    assert.equal(r.status, 0, r.stderr);
    const lines = Object.fromEntries(r.stdout.split('\n').filter((l) => /^[a-z0-9-]+=/.test(l)).map((l) => [l.split('=')[0], l.slice(l.indexOf('=') + 1)]));
    for (const key of ['host', 'uid', 'pwd', 'work', 'workspaces', 'envnames', 'peer0', 'peer0-ls', 'peer1', 'peer1-ls', 'released']) {
      assert.ok(key in lines, `no ${key}= line in ${r.stdout}`);
    }
    assert.equal(lines.peer0, 'absent');
    assert.match(lines['peer0-ls'], /No such file or directory/);
    assert.equal(lines.peer1, 'present');
    assert.equal(lines.released, 't');
    assert.doesNotMatch(lines.envnames, /=/, 'only the names of the variables, never a value');
    assert.equal(require('node:fs').readFileSync(`${dir}/probe.out`, 'utf8').includes('envnames='), true, 'and the same lines are in the file');
  } finally {
    require('node:fs').rmSync(dir, { recursive: true, force: true });
  }
});

test('over HTTP: JSON, server-sent events, the journal, and the rest', async () => {
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  try {
    assert.equal((await fetch(`${base}/healthz`)).status, 200);
    assert.equal((await fetch(`${base}/nothing`)).status, 404);
    assert.deepEqual((await (await fetch(`${base}/v1/models`)).json()).data.map((m) => m.id), ['coding-model']);
    const post = (body) => fetch(`${base}/v1/chat/completions`, { method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: 'Bearer not-kept' }, body: JSON.stringify(body) });

    const json = await (await post(ask('[run:h] go'))).json();
    assert.equal(json.choices[0].finish_reason, 'tool_calls');
    assert.deepEqual(JSON.parse(json.choices[0].message.tool_calls[0].function.arguments), { name: 'iso-h' });

    const sse = await post({ ...ask('[run:h] go', 'ready'), stream: true });
    assert.match(sse.headers.get('content-type'), /^text\/event-stream/);
    const frames = (await sse.text()).split('\n\n').filter(Boolean);
    assert.equal(frames.at(-1), 'data: [DONE]');
    const chunks = frames.slice(0, -1).map((f) => JSON.parse(f.replace(/^data: /, '')));
    const calls = chunks.flatMap((c) => c.choices.flatMap((x) => x.delta?.tool_calls ?? []));
    assert.equal(calls[0].function.name, 'run_command');
    const args = calls.map((c) => c.function.arguments).join('');
    assert.match(JSON.parse(args).command, /tee \/tmp\/probe-h\.out/, 'the arguments, put back together, are the command');
    assert.equal(chunks.at(-1).choices.length, 0, 'the last chunk before [DONE] carries the usage and no choice');
    assert.ok(chunks.at(-1).usage.total_tokens > 0);
    assert.equal(chunks.at(-2).choices[0].finish_reason, 'tool_calls');

    const text = await post({ ...ask('[run:h] go', 'ready', 'done'), stream: true });
    const textChunks = (await text.text()).split('\n\n').filter(Boolean).slice(0, -1).map((f) => JSON.parse(f.replace(/^data: /, '')));
    assert.equal(textChunks.flatMap((c) => c.choices.map((x) => x.delta?.content ?? '')).join(''), 'RUNPODS-PROBE h\ndone');
    assert.equal(textChunks.at(-2).choices[0].finish_reason, 'stop');

    const journal = await (await fetch(`${base}/journal`)).json();
    assert.deepEqual(journal.map((j) => [j.run, j.step, j.stream]), [['h', 1, false], ['h', 2, true], ['h', 3, true]]);
    assert.ok(!JSON.stringify(journal).includes('not-kept'), 'no header of a request is kept');
    assert.deepEqual(journal[2].toolResults, ['ready', 'done']);
    assert.equal((await fetch(`${base}/journal`, { method: 'DELETE' })).status, 200);
    assert.deepEqual(await (await fetch(`${base}/journal`)).json(), []);
    assert.equal((await fetch(`${base}/v1/chat/completions`, { method: 'POST', body: '{not json' })).status, 400);
  } finally {
    server.close();
  }
});
