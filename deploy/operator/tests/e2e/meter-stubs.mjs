#!/usr/bin/env node
// The two stubs of the pod-minute meter's kind end-to-end (ANR-1447), in one process on two ports, on the runner:
//
//   * the orchestrator's broker (`GET <base>/internal/runs/<run>/account`), which the operator's meter asks for the
//     account of a run pod (`crates/usage-broker/src/lib.rs`): a bearer it was told, `{"accountId": ...}`, and a 404
//     `run_not_bound` for every other caller (the broker answers 404 to every caller it does not accept);
//   * an OTLP/HTTP JSON logs collector (`POST /v1/logs`), which the operator's sink posts to
//     (`crates/usage-otlp/src/encode.rs`): a 200 `{}` (no `partialSuccess.rejectedLogRecords`, so nothing is rejected)
//     and the body written to a file the script reads, so that a whole line can be asserted without printing it.
//
// Nothing is logged but a method, a path, a status and a byte count: **no Authorization header, no token, no account and
// no body**. The bodies are the evidence of the end-to-end, not a log. The two servers bind 0.0.0.0 on ports the script
// lets the kernel pick (`METER_BROKER_PORT`/`METER_COLLECTOR_PORT` empty or 0), print each port and `meter-stubs: ready`,
// and stop on SIGTERM/SIGINT.
//
// Environment: METER_STUB_DIR (where the received bodies go, required), METER_BROKER_TOKEN (the bearer to require,
// required), METER_BROKER_ACCOUNT (default `acct-e2e`), METER_BROKER_PORT and METER_COLLECTOR_PORT (0: pick one).
import { createServer } from "node:http";
import { mkdirSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const dir = process.env.METER_STUB_DIR || "";
const token = process.env.METER_BROKER_TOKEN || "";
const account = process.env.METER_BROKER_ACCOUNT || "acct-e2e";
const brokerPort = Number(process.env.METER_BROKER_PORT || 0);
const collectorPort = Number(process.env.METER_COLLECTOR_PORT || 0);

// Fixed words and no value: the message is what a person reads when the script's own check has already failed.
if (!dir) {
  process.stderr.write("meter-stubs: METER_STUB_DIR is not set\n");
  process.exit(2);
}
if (!token) {
  process.stderr.write("meter-stubs: METER_BROKER_TOKEN is not set\n");
  process.exit(2);
}
mkdirSync(dir, { recursive: true });

// The most of a body that is read: the sink's batch is 16 lines in this end-to-end, so this is a bound, not a limit.
const MAX_BODY = 4 * 1024 * 1024;
let received = 0;
let listening = 0;

function log(line) {
  process.stdout.write(`meter-stubs: ${line}\n`);
}

function ready() {
  listening += 1;
  if (listening === 2) log("ready");
}

// readBody: the request's body, or a 413 past MAX_BODY. The body is never echoed and never logged.
function readBody(req, res, onBody) {
  const chunks = [];
  let size = 0;
  let over = false;
  req.on("data", (chunk) => {
    if (over) return;
    size += chunk.length;
    if (size > MAX_BODY) {
      over = true;
      chunks.length = 0;
      res.writeHead(413, { "content-type": "application/json" });
      res.end('{"code":"too_large"}');
      req.destroy();
      return;
    }
    chunks.push(chunk);
  });
  req.on("end", () => {
    if (!over) onBody(Buffer.concat(chunks));
  });
}

function notFound(res, body) {
  res.writeHead(404, { "content-type": "application/json" });
  res.end(body);
}

// The broker: GET /internal/runs/<one segment>/account, with the bearer, answers the account. Any other path, method or
// bearer is the broker's 404 `run_not_bound` (what it answers to every caller it does not accept).
const broker = createServer((req, res) => {
  const path = req.url || "";
  const one = /^\/internal\/runs\/([^/]+)\/account$/.exec(path);
  if (req.method !== "GET" || one === null) {
    notFound(res, '{"code":"run_not_bound"}');
    log(`broker ${req.method} ${path} 404`);
    return;
  }
  if (req.headers.authorization !== `Bearer ${token}`) {
    notFound(res, '{"code":"run_not_bound"}');
    log(`broker GET ${path} 404`);
    return;
  }
  res.writeHead(200, { "content-type": "application/json" });
  res.end(JSON.stringify({ accountId: account }));
  log(`broker GET ${path} 200`);
});

// The collector: POST /v1/logs, a JSON body (OTLP/HTTP JSON), answered 200 `{}` and written to a file of its own. A body
// that is not JSON is 400 and is not written: the sink would drop the batch for good, which is a failure the script shows.
const collector = createServer((req, res) => {
  const path = req.url || "";
  if (req.method !== "POST" || path !== "/v1/logs") {
    notFound(res, "{}");
    log(`collector ${req.method} ${path} 404`);
    return;
  }
  readBody(req, res, (body) => {
    try {
      JSON.parse(body.toString("utf8"));
    } catch {
      res.writeHead(400, { "content-type": "application/json" });
      res.end('{"code":"invalid"}');
      log("collector POST /v1/logs 400");
      return;
    }
    // A rename on the same directory: the script never reads half a body.
    const name = `received-${(received += 1)}.json`;
    const part = join(dir, `.${name}.part`);
    writeFileSync(part, body);
    renameSync(part, join(dir, name));
    res.writeHead(200, { "content-type": "application/json" });
    res.end("{}");
    log(`collector POST /v1/logs 200 ${body.length}`);
  });
});

broker.listen(brokerPort, "0.0.0.0", () => {
  log(`broker listening on 0.0.0.0:${broker.address().port}`);
  ready();
});
collector.listen(collectorPort, "0.0.0.0", () => {
  log(`collector listening on 0.0.0.0:${collector.address().port}`);
  ready();
});

function stop() {
  broker.close();
  collector.close();
  process.exit(0);
}
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
