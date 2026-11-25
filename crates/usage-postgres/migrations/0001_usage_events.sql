-- The usage ingest's one table (AD-060). `(source, id)` is the idempotency key; `ingested_at` is
-- seconds since the Unix epoch, the second the ingest received the first copy.
CREATE TABLE IF NOT EXISTS usage_events (
    source        text   NOT NULL,
    id            text   NOT NULL,
    account       text   NOT NULL,
    weight        bigint NOT NULL CHECK (weight >= 1),
    node_type     text   NOT NULL,
    pod_kind      text   NOT NULL,
    agent_service text   NOT NULL,
    run           text,
    ingested_at   bigint NOT NULL,
    PRIMARY KEY (source, id)
);
CREATE INDEX IF NOT EXISTS usage_events_ingested_at ON usage_events (ingested_at);
