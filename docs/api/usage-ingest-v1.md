# The usage ingest contract (v1)

What the pod meter sends and what the ledger reads. Decided in [AD-060](../architecture/11-decisions.md); the port is [`aap-usage`](../../crates/usage/README.md). The ingest records **weighted pod-minutes** only: it holds no rate and no amount, and the reader gives them meaning. A breaking change is `v2` and a new file.

## The event

A [CloudEvents 1.0](https://github.com/cloudevents/spec/blob/v1.0.2/cloudevents/formats/json-format.md) structured-JSON event, `Content-Type: application/cloudevents+json`.

| Attribute | Value |
|---|---|
| `specversion` | `"1.0"` |
| `type` | `com.vymalo.agents.pod-minute.v1` |
| `source` | The meter instance that reports it. |
| `id` | Lower-case hex of `sha256(pod uid, 0x00, minute)`, `minute` being whole minutes since the Unix epoch in decimal. Same pod and minute, same id, from whichever replica reports it. |
| `subject` | The account the minute is for. |
| `time` | The end of the minute, RFC 3339. Informational: spend is counted by **ingestion** time. |
| `data.weight` | Integer, at least 1: the weighted units of this minute. |
| `data.nodeType` | The node type the pod ran on. |
| `data.podKind` | What the pod was: `agent` or `run`. |
| `data.agentService` | The agent service it belongs to. |
| `data.run` | Optional: the run it served. |

Every text value is 1 to 128 bytes of `[A-Za-z0-9._:/-]`, so free text and personal data cannot pass. An event outside that is refused with the name of the field and never its value.

```json
{
  "specversion": "1.0",
  "type": "com.vymalo.agents.pod-minute.v1",
  "source": "meter/operator-0",
  "id": "3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855a",
  "subject": "acct-7f3a",
  "time": "2026-11-25T10:31:00Z",
  "datacontenttype": "application/json",
  "data": { "weight": 2, "nodeType": "std", "podKind": "run", "agentService": "coder", "run": "run-91" }
}
```

## Once

The key is `(source, id)`. The first copy stands: a repeat, even with other data, changes nothing and keeps the first copy's ingestion time. The same `id` from another `source` is another event. The answer says which: `201` for a new event, `200` for a duplicate (the port's `Inserted::New` and `Inserted::Duplicate`).

## The spend query

The ledger asks for **ingestion** time, in seconds since the Unix epoch, over a half-open window `[from, to)`, optionally for one account.

```json
{ "from": 1795600000, "to": 1795603600, "account": "acct-7f3a" }
```

```json
{ "lines": [
  { "account": "acct-7f3a", "nodeType": "big", "weightedMinutes": 8 },
  { "account": "acct-7f3a", "nodeType": "std", "weightedMinutes": 3 }
] }
```

* Lines are sorted by account, then node type; a pair with no minute has no line.
* An empty or reversed window is an error (`EmptyWindow`), not an empty answer.
* **The lag.** An event can arrive after its minute. A reader asks only for windows that end at least one lag before now (the ingest's own setting, never less than the meter's delay), and asks the same window again to get the same answer: a window's content changes only by an event whose ingestion time falls in it.
* A sum that does not fit in 64 bits is `Overflow`; narrow the window.
