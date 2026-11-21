'use strict';
// What k10-model.js says, with no cluster and no agent (the kind end-to-end of the tool catalogue, K10, depends on every line of it): the two
// turns, the markers of a probe, the answer when the tool result holds the pod's name and when it does not, and the two shapes of an answer (JSON,
// and server-sent events an agent can assemble).
//
//   node deploy/operator/tests/e2e/k10-model.test.js        (Node 18 or later: fetch, node:test)
const assert = require('node:assert/strict');
const { test } = require('node:test');
const { turn, marker, toolResults, server } = require('./k10-model.js');

// A tool result as the catalogue's Kubernetes server words it: the pods of the namespace as YAML, `metadata.name` among them, and no JSON.
const podsYaml = (ns, ...names) => names.map((n) => [
  '- apiVersion: v1',
  '  kind: Pod',
  '  metadata:',
  '    creationTimestamp: "2026-11-21T00:00:00Z"',
  '    labels:',
  '      app.kubernetes.io/instance: k10',
  '    name: ' + n,
  '    namespace: ' + ns,
  '    resourceVersion: "1"',
  '  spec:',
  '    containers:',
  '    - image: ghcr.io/example/agent:1',
  '      name: agent',
  '    nodeName: aap-k10-control-plane',
].join('\n')).join('\n') + '\n';

const body = (text, ...results) => ({
  model: 'k10-model',
  messages: [
    { role: 'system', content: 'You are an agent.' },
    { role: 'user', content: text },
    ...results.flatMap((r, i) => [
      { role: 'assistant', content: null, tool_calls: [{ id: `c${i}`, type: 'function', function: { name: 'kubernetes__pods_list_in_namespace', arguments: '{}' } }] },
      { role: 'tool', tool_call_id: `c${i}`, content: r },
    ]),
  ],
});
// The turn of a request with that body.
const ask = (text, ...results) => turn(body(text, ...results).messages);

test('a message that carries no probe marker gets a plain answer and no tool call', () => {
  for (const text of ['Say hi.', '[ns:aap-e2e-k10] only the namespace', '[pod:k10-0] only the pod']) {
    const t = ask(text);
    assert.equal(t.step, -1, text);
    assert.equal(t.call, undefined, text);
    assert.equal(t.text, 'Nothing to do.', text);
  }
});

test('the two turns of a probe: the catalogue tool by its prefixed name, then the pod name if the result holds it', () => {
  const text = 'List the pods of [ns:aap-e2e-k10] and tell me whether [pod:k10-0] is among them.';
  const one = ask(text);
  assert.equal(one.step, 1);
  assert.deepEqual(one.call, { name: 'kubernetes__pods_list_in_namespace', args: { namespace: 'aap-e2e-k10' } });

  const two = ask(text, podsYaml('aap-e2e-k10', 'k10-0', 'k10-model-abc', 'kubernetes-mcp-def'));
  assert.equal(two.step, 2);
  assert.equal(two.text, 'K10-PROBE aap-e2e-k10 found k10-0');
  assert.equal(two.call, undefined);
});

test('a tool result that does not hold the pod is said so, not guessed', () => {
  const text = '[ns:aap-e2e-k10] [pod:k10-0]';
  assert.equal(ask(text, podsYaml('aap-e2e-k10', 'someone-else')).text, 'K10-PROBE aap-e2e-k10 missing k10-0');
  assert.equal(ask(text, 'failed to list pods in namespace aap-e2e-k10: Forbidden').text, 'K10-PROBE aap-e2e-k10 missing k10-0');
  assert.equal(ask(text, '').text, 'K10-PROBE aap-e2e-k10 missing k10-0');
});

test('the markers are read from the first user message, and a dotted or dashed name survives', () => {
  const messages = [
    { role: 'user', content: '[ns:aap-e2e-k10] [pod:k10-0]' },
    { role: 'user', content: '[ns:another] [pod:other-1]' },
  ];
  assert.equal(marker(messages, 'ns'), 'aap-e2e-k10');
  assert.equal(marker(messages, 'pod'), 'k10-0');
  assert.equal(marker(messages, 'nothing'), '');
  assert.deepEqual(toolResults(messages), []);
});

test('over HTTP: JSON, server-sent events, the journal, and the rest', async () => {
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  const text = 'List the pods of [ns:aap-e2e-k10] and tell me whether [pod:k10-0] is among them.';
  const pods = podsYaml('aap-e2e-k10', 'k10-0', 'k10-model-abc');
  try {
    assert.equal((await fetch(`${base}/healthz`)).status, 200);
    assert.equal((await fetch(`${base}/nothing`)).status, 404);
    assert.deepEqual((await (await fetch(`${base}/v1/models`)).json()).data.map((m) => m.id), ['k10-model']);
    const post = (body) => fetch(`${base}/v1/chat/completions`, { method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: 'Bearer not-kept' }, body: JSON.stringify(body) });

    const json = await (await post(body(text))).json();
    assert.equal(json.choices[0].finish_reason, 'tool_calls');
    assert.equal(json.choices[0].message.tool_calls[0].function.name, 'kubernetes__pods_list_in_namespace');
    assert.deepEqual(JSON.parse(json.choices[0].message.tool_calls[0].function.arguments), { namespace: 'aap-e2e-k10' });

    const sse = await post({ ...body(text), stream: true });
    assert.match(sse.headers.get('content-type'), /^text\/event-stream/);
    const frames = (await sse.text()).split('\n\n').filter(Boolean);
    assert.equal(frames.at(-1), 'data: [DONE]');
    const chunks = frames.slice(0, -1).map((f) => JSON.parse(f.replace(/^data: /, '')));
    const calls = chunks.flatMap((c) => c.choices.flatMap((x) => x.delta?.tool_calls ?? []));
    assert.equal(calls[0].function.name, 'kubernetes__pods_list_in_namespace');
    const args = calls.map((c) => c.function.arguments).join('');
    assert.deepEqual(JSON.parse(args), { namespace: 'aap-e2e-k10' }, 'the arguments, put back together, are the call');
    assert.equal(chunks.at(-1).choices.length, 0, 'the last chunk before [DONE] carries the usage and no choice');
    assert.ok(chunks.at(-1).usage.total_tokens > 0);
    assert.equal(chunks.at(-2).choices[0].finish_reason, 'tool_calls');

    const answer = await post({ ...body(text, pods), stream: true });
    const textChunks = (await answer.text()).split('\n\n').filter(Boolean).slice(0, -1).map((f) => JSON.parse(f.replace(/^data: /, '')));
    assert.equal(textChunks.flatMap((c) => c.choices.map((x) => x.delta?.content ?? '')).join(''), 'K10-PROBE aap-e2e-k10 found k10-0');
    assert.equal(textChunks.at(-2).choices[0].finish_reason, 'stop');

    const journal = await (await fetch(`${base}/journal`)).json();
    assert.deepEqual(journal.map((j) => [j.step, j.streamed]), [[1, false], [1, true], [2, true]]);
    assert.ok(!JSON.stringify(journal).includes('not-kept'), 'no header of a request is kept');
    assert.deepEqual(journal[2].results, [pods]);
    assert.equal((await fetch(`${base}/journal`, { method: 'DELETE' })).status, 200);
    assert.deepEqual(await (await fetch(`${base}/journal`)).json(), []);
    assert.equal((await fetch(`${base}/v1/chat/completions`, { method: 'POST', body: '{not json' })).status, 400);
  } finally {
    server.close();
  }
});
