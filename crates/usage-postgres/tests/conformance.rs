#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The kit's eight rules against a real Postgres, and what a store of one process cannot show: two writers at
//! once, and a store opened again. Skips unless `AAP_TEST_DATABASE_URL` is set; fails instead under
//! `AAP_TEST_REQUIRE_DB=1`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use aap_usage::testkit::{self, event};
use aap_usage::{Inserted, SpendLine, SpendQuery, UsageStore, Window};
use aap_usage_postgres::PgUsageStore;
use tokio_postgres::{Config, NoTls};

static N: AtomicU32 = AtomicU32::new(0);

fn url() -> Option<String> {
    match std::env::var("AAP_TEST_DATABASE_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            assert!(
                std::env::var("AAP_TEST_REQUIRE_DB").as_deref() != Ok("1"),
                "AAP_TEST_REQUIRE_DB=1 and AAP_TEST_DATABASE_URL is not set"
            );
            eprintln!("skipped: AAP_TEST_DATABASE_URL is not set");
            None
        }
    }
}

/// A store in a schema of its own (so every rule starts empty); `schema` names it again to reopen.
async fn open(url: &str, schema: &str) -> PgUsageStore {
    let mut config: Config = url.parse().unwrap();
    config.options(format!("-c search_path={schema}"));
    let (client, connection) = config.connect(NoTls).await.unwrap();
    tokio::spawn(connection);
    client
        .batch_execute(&format!("CREATE SCHEMA IF NOT EXISTS {schema}"))
        .await
        .unwrap();
    let store = PgUsageStore::new(Arc::new(client));
    store.migrate().await.unwrap();
    store
}

fn fresh() -> String {
    format!(
        "t_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    )
}

#[tokio::test]
async fn every_rule_of_the_kit_passes_on_postgres() {
    let Some(url) = url() else { return };
    testkit::check_idempotent(&open(&url, &fresh()).await)
        .await
        .unwrap();
    testkit::check_first_copy_stands(&open(&url, &fresh()).await)
        .await
        .unwrap();
    testkit::check_source_scopes_id(&open(&url, &fresh()).await)
        .await
        .unwrap();
    testkit::check_sums_and_order(&open(&url, &fresh()).await)
        .await
        .unwrap();
    testkit::check_window_half_open(&open(&url, &fresh()).await)
        .await
        .unwrap();
    testkit::check_account_filter(&open(&url, &fresh()).await)
        .await
        .unwrap();
    testkit::check_invalid_refused(&open(&url, &fresh()).await)
        .await
        .unwrap();
    testkit::check_empty_window(&open(&url, &fresh()).await)
        .await
        .unwrap();
}

fn all() -> SpendQuery {
    SpendQuery {
        window: Window {
            from: 0,
            to: 10_000,
        },
        account: None,
    }
}

#[tokio::test]
async fn two_concurrent_writers_leave_one_row_and_one_new() {
    let Some(url) = url() else { return };
    let schema = fresh();
    // Two connections, as two replicas would hold.
    let a = open(&url, &schema).await;
    let b = open(&url, &schema).await;
    let mut tasks = Vec::new();
    for i in 0..20 {
        let store = if i % 2 == 0 { a.clone() } else { b.clone() };
        tasks.push(tokio::spawn(async move {
            store
                .insert(event("m1", "pod-1", 10), 100 + i)
                .await
                .unwrap()
        }));
    }
    let mut news = 0;
    for t in tasks {
        if t.await.unwrap() == Inserted::New {
            news += 1;
        }
    }
    assert_eq!(news, 1);
    let lines = a.spend(all()).await.unwrap();
    assert_eq!(
        lines,
        vec![SpendLine {
            account: "acct-a".into(),
            node_type: "std".into(),
            weighted_minutes: 1
        }]
    );
}

#[tokio::test]
async fn a_reopened_store_still_knows_the_event_and_the_spend_is_the_distinct_sum() {
    let Some(url) = url() else { return };
    let schema = fresh();
    {
        let first = open(&url, &schema).await;
        assert_eq!(
            first.insert(event("m1", "p1", 1), 50).await.unwrap(),
            Inserted::New
        );
        assert_eq!(
            first.insert(event("m1", "p2", 1), 60).await.unwrap(),
            Inserted::New
        );
    } // the first store and its client are dropped
    let again = open(&url, &schema).await; // migrate runs again: safe
    assert_eq!(
        again.insert(event("m1", "p1", 1), 900).await.unwrap(),
        Inserted::Duplicate
    );
    assert_eq!(
        again.insert(event("m1", "p3", 1), 70).await.unwrap(),
        Inserted::New
    );
    // Three distinct events of weight 1; the duplicate, sent at 900, counts nowhere.
    let lines = again.spend(all()).await.unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].weighted_minutes, 3);
}
