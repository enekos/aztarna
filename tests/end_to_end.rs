//! End-to-end exercise: ingest fabricated hook payloads, then verify that
//! ranking, decay, scope resolution, and sequence detection all behave.

use aztarna::{db, ingest, query};
use chrono::Utc;
use rusqlite::params;
use std::path::PathBuf;
use tempfile::TempDir;

fn fresh_db() -> (TempDir, rusqlite::Connection) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("aztarna.sqlite");
    let conn = db::open(&path).unwrap();
    (tmp, conn)
}

fn payload(session: &str, cwd: &str, command: &str) -> ingest::HookPayload {
    ingest::HookPayload {
        session_id: Some(session.into()),
        cwd: cwd.into(),
        tool_name: Some("Bash".into()),
        tool_input: ingest::ToolInput {
            command: Some(command.into()),
        },
        tool_response: ingest::ToolResponse::default(),
    }
}

/// Backdate the most recently inserted row to `days_ago`. Used to test decay.
fn backdate_latest(conn: &rusqlite::Connection, days_ago: i64) {
    let new_ts = Utc::now().timestamp() - days_ago * 86_400;
    conn.execute(
        "UPDATE commands SET ts = ?1 WHERE id = (SELECT MAX(id) FROM commands)",
        params![new_ts],
    )
    .unwrap();
}

#[test]
fn ingest_and_top_groups_by_head() {
    let (_tmp, conn) = fresh_db();
    let cwd = "/tmp/proj-a";

    for _ in 0..5 {
        ingest::ingest(&conn, &payload("s1", cwd, "cargo test --lib")).unwrap();
    }
    for _ in 0..2 {
        ingest::ingest(&conn, &payload("s1", cwd, "git status -s")).unwrap();
    }
    ingest::ingest(&conn, &payload("s1", cwd, "ls -la")).unwrap();

    let scope = PathBuf::from(cwd);
    let top = query::top(
        &conn,
        &scope,
        query::DEFAULT_HALF_LIFE_DAYS,
        10,
        None,
        false,
    )
    .unwrap();

    assert_eq!(top.len(), 3);
    assert_eq!(top[0].head, "cargo test");
    assert_eq!(top[0].hits, 5);
    assert_eq!(top[1].head, "git status");
    assert_eq!(top[1].hits, 2);
}

#[test]
fn non_bash_tool_is_ignored() {
    let (_tmp, conn) = fresh_db();
    let mut p = payload("s1", "/tmp/x", "anything");
    p.tool_name = Some("Read".into());
    let id = ingest::ingest(&conn, &p).unwrap();
    assert!(id.is_none());
}

#[test]
fn empty_command_is_ignored() {
    let (_tmp, conn) = fresh_db();
    let p = payload("s1", "/tmp/x", "   ");
    let id = ingest::ingest(&conn, &p).unwrap();
    assert!(id.is_none());
}

#[test]
fn cd_command_is_ignored() {
    let (_tmp, conn) = fresh_db();
    let p = payload("s1", "/tmp/x", "cd /some/path");
    let id = ingest::ingest(&conn, &p).unwrap();
    assert!(id.is_none(), "cd should be filtered out at ingest time");

    // Verify it doesn't show up in rankings either.
    let scope = PathBuf::from("/tmp/x");
    let top = query::top(
        &conn,
        &scope,
        query::DEFAULT_HALF_LIFE_DAYS,
        10,
        None,
        false,
    )
    .unwrap();
    assert!(top.is_empty());
}

#[test]
fn cd_prefix_updates_cwd_and_scope() {
    let (_tmp, conn) = fresh_db();
    let p = payload("s1", "/tmp/x", "cd /tmp/y && cargo test");
    let id = ingest::ingest(&conn, &p).unwrap();
    assert!(id.is_some(), "cargo test after cd should be logged");

    // Scope should be /tmp/y, not /tmp/x.
    let y_scope = PathBuf::from("/tmp/y");
    let top = query::top(
        &conn,
        &y_scope,
        query::DEFAULT_HALF_LIFE_DAYS,
        10,
        None,
        false,
    )
    .unwrap();
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].head, "cargo test");

    let x_scope = PathBuf::from("/tmp/x");
    let top_x = query::top(
        &conn,
        &x_scope,
        query::DEFAULT_HALF_LIFE_DAYS,
        10,
        None,
        false,
    )
    .unwrap();
    assert!(top_x.is_empty());
}

#[test]
fn iterative_cd_prefix_updates_cwd() {
    let (_tmp, conn) = fresh_db();
    let p = payload("s1", "/tmp/x", "cd /tmp/y && cd /tmp/z && cargo build");
    let id = ingest::ingest(&conn, &p).unwrap();
    assert!(id.is_some());

    let z_scope = PathBuf::from("/tmp/z");
    let top = query::top(
        &conn,
        &z_scope,
        query::DEFAULT_HALF_LIFE_DAYS,
        10,
        None,
        false,
    )
    .unwrap();
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].head, "cargo build");
}

#[test]
fn decay_demotes_old_commands() {
    let (_tmp, conn) = fresh_db();
    let cwd = "/tmp/proj-b";

    // 1 fresh "cargo test", 8 ancient "git status" runs.
    ingest::ingest(&conn, &payload("s1", cwd, "cargo test")).unwrap();
    for _ in 0..8 {
        ingest::ingest(&conn, &payload("s1", cwd, "git status")).unwrap();
        backdate_latest(&conn, 90); // a quarter old
    }

    let scope = PathBuf::from(cwd);
    let top = query::top(
        &conn,
        &scope,
        query::DEFAULT_HALF_LIFE_DAYS,
        10,
        None,
        false,
    )
    .unwrap();

    // Even though git status has 8x the raw hits, 90-day-old runs with a
    // 14-day half-life are worth ~0.013 each → ~0.1 total, far below the
    // fresh cargo test at ~1.0.
    assert_eq!(top[0].head, "cargo test");
    assert!(top[0].score > top[1].score);
}

#[test]
fn no_decay_when_half_life_is_zero() {
    let (_tmp, conn) = fresh_db();
    let cwd = "/tmp/proj-c";
    ingest::ingest(&conn, &payload("s1", cwd, "cargo test")).unwrap();
    backdate_latest(&conn, 365);

    let scope = PathBuf::from(cwd);
    let top = query::top(&conn, &scope, 0.0, 10, None, false).unwrap();
    // half_life=0 short-circuits the SQL function to weight=1 per hit.
    assert_eq!(top[0].hits, 1);
    assert!((top[0].score - 1.0).abs() < 1e-9);
}

#[test]
fn sequences_detects_adjacent_pairs_in_session() {
    let (_tmp, conn) = fresh_db();
    let cwd = "/tmp/proj-d";

    // Two sessions, both with the same pattern: cargo build -> cargo test.
    for session in ["s1", "s2"] {
        ingest::ingest(&conn, &payload(session, cwd, "cargo build")).unwrap();
        ingest::ingest(&conn, &payload(session, cwd, "cargo test")).unwrap();
        ingest::ingest(&conn, &payload(session, cwd, "git status")).unwrap();
    }

    let scope = PathBuf::from(cwd);
    let seqs = query::sequences(&conn, &scope, 10).unwrap();

    let top_pair = &seqs[0];
    assert_eq!(top_pair.prev, "cargo build");
    assert_eq!(top_pair.next, "cargo test");
    assert_eq!(top_pair.hits, 2);
}

#[test]
fn sequences_does_not_cross_session_boundaries() {
    let (_tmp, conn) = fresh_db();
    let cwd = "/tmp/proj-e";
    ingest::ingest(&conn, &payload("s1", cwd, "cargo build")).unwrap();
    ingest::ingest(&conn, &payload("s2", cwd, "cargo test")).unwrap();

    let scope = PathBuf::from(cwd);
    let seqs = query::sequences(&conn, &scope, 10).unwrap();
    assert!(seqs.is_empty(), "no within-session adjacency, no pair");
}

#[test]
fn sequences_drops_self_transitions() {
    let (_tmp, conn) = fresh_db();
    let cwd = "/tmp/proj-f";
    // Three identical commands in a row — should NOT show up as ls -> ls.
    for _ in 0..3 {
        ingest::ingest(&conn, &payload("s1", cwd, "ls")).unwrap();
    }
    // Add a real transition so the result isn't trivially empty.
    ingest::ingest(&conn, &payload("s1", cwd, "pwd")).unwrap();

    let scope = PathBuf::from(cwd);
    let seqs = query::sequences(&conn, &scope, 10).unwrap();
    for s in &seqs {
        assert_ne!(s.prev, s.next, "self-transitions should be filtered out");
    }
    // The only real pair is ls -> pwd.
    assert_eq!(seqs.len(), 1);
    assert_eq!(seqs[0].prev, "ls");
    assert_eq!(seqs[0].next, "pwd");
}

#[test]
fn scope_isolates_projects() {
    let (_tmp, conn) = fresh_db();
    ingest::ingest(&conn, &payload("s1", "/tmp/proj-x", "cargo test")).unwrap();
    ingest::ingest(&conn, &payload("s1", "/tmp/proj-y", "go test ./...")).unwrap();

    let x = query::top(&conn, &PathBuf::from("/tmp/proj-x"), 14.0, 10, None, false).unwrap();
    let y = query::top(&conn, &PathBuf::from("/tmp/proj-y"), 14.0, 10, None, false).unwrap();

    assert_eq!(x.len(), 1);
    assert_eq!(x[0].head, "cargo test");
    assert_eq!(y.len(), 1);
    assert_eq!(y[0].head, "go test");
}

#[test]
fn top_query_filters_by_command_text() {
    let (_tmp, conn) = fresh_db();
    let cwd = "/tmp/proj-query";

    ingest::ingest(&conn, &payload("s1", cwd, "cargo test --lib")).unwrap();
    ingest::ingest(&conn, &payload("s1", cwd, "cargo build --release")).unwrap();
    ingest::ingest(&conn, &payload("s1", cwd, "git status -s")).unwrap();

    let scope = PathBuf::from(cwd);
    let top = query::top(
        &conn,
        &scope,
        query::DEFAULT_HALF_LIFE_DAYS,
        10,
        Some("test"),
        false,
    )
    .unwrap();

    assert_eq!(top.len(), 1);
    assert_eq!(top[0].head, "cargo test");
    assert_eq!(top[0].hits, 1);
}

#[test]
fn top_success_only_excludes_failures() {
    let (_tmp, conn) = fresh_db();
    let cwd = "/tmp/proj-success";

    let mut success = payload("s1", cwd, "cargo test");
    success.tool_response.exit_code = Some(0);
    ingest::ingest(&conn, &success).unwrap();

    let mut failure = payload("s1", cwd, "cargo test");
    failure.tool_response.exit_code = Some(1);
    for _ in 0..5 {
        ingest::ingest(&conn, &failure).unwrap();
    }

    let scope = PathBuf::from(cwd);
    let top_all = query::top(
        &conn,
        &scope,
        query::DEFAULT_HALF_LIFE_DAYS,
        10,
        None,
        false,
    )
    .unwrap();
    let top_ok = query::top(&conn, &scope, query::DEFAULT_HALF_LIFE_DAYS, 10, None, true).unwrap();

    assert_eq!(top_all[0].hits, 6, "all runs counted without success_only");
    assert_eq!(top_ok[0].hits, 1, "only successful runs counted");
}

#[test]
fn top_weights_failures_into_score() {
    let (_tmp, conn) = fresh_db();
    let cwd = "/tmp/proj-fail-weight";

    // 2 clean runs of cargo test.
    let mut ok = payload("s1", cwd, "cargo test");
    ok.tool_response.exit_code = Some(0);
    for _ in 0..2 {
        ingest::ingest(&conn, &ok).unwrap();
    }

    // 4 runs of make deploy, only 1 succeeds — raw hit count is higher,
    // but the success-rate weight should pull it below cargo test.
    for code in [0, 1, 1, 1] {
        let mut p = payload("s1", cwd, "make deploy");
        p.tool_response.exit_code = Some(code);
        ingest::ingest(&conn, &p).unwrap();
    }

    let scope = PathBuf::from(cwd);
    let top = query::top(
        &conn,
        &scope,
        query::DEFAULT_HALF_LIFE_DAYS,
        10,
        None,
        false,
    )
    .unwrap();

    assert_eq!(top[0].head, "cargo test");
    assert!((top[0].success_rate - 1.0).abs() < 1e-9);
    assert_eq!(top[1].head, "make deploy");
    assert_eq!(top[1].hits, 4);
    assert!((top[1].success_rate - 0.25).abs() < 1e-9);
    assert!(top[0].score > top[1].score);
}

#[test]
fn unknown_exit_code_counts_as_success() {
    let (_tmp, conn) = fresh_db();
    let cwd = "/tmp/proj-unknown-exit";
    // payload() leaves exit_code None — the common case for older rows.
    ingest::ingest(&conn, &payload("s1", cwd, "cargo build")).unwrap();

    let scope = PathBuf::from(cwd);
    let top = query::top(
        &conn,
        &scope,
        query::DEFAULT_HALF_LIFE_DAYS,
        10,
        None,
        false,
    )
    .unwrap();
    assert!((top[0].success_rate - 1.0).abs() < 1e-9);
    assert!(top[0].score > 0.0, "NULL exit code must not zero the score");
}

#[test]
fn failing_view_ranks_by_failure_mass() {
    let (_tmp, conn) = fresh_db();
    let cwd = "/tmp/proj-failing";

    let fail = |command: &str, n: usize| {
        for _ in 0..n {
            let mut p = payload("s1", cwd, command);
            p.tool_response.exit_code = Some(1);
            ingest::ingest(&conn, &p).unwrap();
        }
    };
    fail("make deploy", 3);
    fail("cargo test", 1);

    // A pure-success command must not appear in the failing view.
    let mut ok = payload("s1", cwd, "cargo build");
    ok.tool_response.exit_code = Some(0);
    ingest::ingest(&conn, &ok).unwrap();

    let scope = PathBuf::from(cwd);
    let rows = query::failing(&conn, &scope, query::DEFAULT_HALF_LIFE_DAYS, 10).unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].head, "make deploy");
    assert_eq!(rows[0].failures, 3);
    assert_eq!(rows[1].head, "cargo test");
    assert!(rows.iter().all(|r| r.head != "cargo build"));
}

#[test]
fn allowlist_emits_prefix_rules_and_skips_read_only() {
    let (_tmp, conn) = fresh_db();
    let cwd = "/tmp/proj-allow";

    let mut ok = payload("s1", cwd, "cargo test --lib");
    ok.tool_response.exit_code = Some(0);
    for _ in 0..3 {
        ingest::ingest(&conn, &ok).unwrap();
    }
    // Read-only for Claude Code already — must not produce a rule.
    ingest::ingest(&conn, &payload("s1", cwd, "ls -la")).unwrap();
    ingest::ingest(&conn, &payload("s1", cwd, "git status -s")).unwrap();

    let scope = PathBuf::from(cwd);
    let rules = query::allowlist(&conn, &scope, query::DEFAULT_HALF_LIFE_DAYS, 10, 0.0).unwrap();

    assert_eq!(rules, vec!["Bash(cargo test:*)".to_string()]);
}

#[test]
fn allowlist_respects_min_score_and_limit() {
    let (_tmp, conn) = fresh_db();
    let cwd = "/tmp/proj-allow-min";

    for _ in 0..5 {
        ingest::ingest(&conn, &payload("s1", cwd, "cargo build")).unwrap();
    }
    ingest::ingest(&conn, &payload("s1", cwd, "cargo clippy")).unwrap();
    ingest::ingest(&conn, &payload("s1", cwd, "cargo fmt")).unwrap();

    let scope = PathBuf::from(cwd);

    // min_score above a single fresh hit (~1.0) keeps only cargo build.
    let rules = query::allowlist(&conn, &scope, query::DEFAULT_HALF_LIFE_DAYS, 10, 2.0).unwrap();
    assert_eq!(rules, vec!["Bash(cargo build:*)".to_string()]);

    // limit=1 keeps only the top-ranked head.
    let rules = query::allowlist(&conn, &scope, query::DEFAULT_HALF_LIFE_DAYS, 1, 0.0).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0], "Bash(cargo build:*)");
}

#[test]
fn allowlist_cli_emits_settings_json_shape() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("aztarna.sqlite");
    let bin = env!("CARGO_BIN_EXE_aztarna");

    let payload = r#"{"session_id":"s1","cwd":"/tmp/proj-cli","tool_name":"Bash",
        "tool_input":{"command":"cargo test"},"tool_response":{"exit_code":0}}"#;
    let mut child = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "log"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());

    let out = Command::new(bin)
        .args([
            "--db",
            db.to_str().unwrap(),
            "allowlist",
            "--cwd",
            "/tmp/proj-cli",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        doc["permissions"]["allow"],
        serde_json::json!(["Bash(cargo test:*)"])
    );
}

#[test]
fn context_block_includes_top_and_sequences() {
    let (_tmp, conn) = fresh_db();
    let cwd = "/tmp/proj-g";
    for _ in 0..3 {
        ingest::ingest(&conn, &payload("s1", cwd, "cargo build")).unwrap();
        ingest::ingest(&conn, &payload("s1", cwd, "cargo test")).unwrap();
    }

    let scope = PathBuf::from(cwd);
    let block = query::context_block(&conn, &scope, 14.0, 10, 10).unwrap();
    assert!(block.contains("Frequently-used commands"));
    assert!(block.contains("cargo test"));
    assert!(block.contains("cargo build"));
    assert!(block.contains("Common follow-ups"));
    assert!(block.contains("cargo build` → `cargo test"));
}

#[test]
fn context_block_empty_when_no_data() {
    let (_tmp, conn) = fresh_db();
    let block = query::context_block(&conn, &PathBuf::from("/nowhere"), 14.0, 10, 10).unwrap();
    assert_eq!(block, "");
}

#[test]
fn stats_reports_counts() {
    let (_tmp, conn) = fresh_db();
    ingest::ingest(&conn, &payload("s1", "/tmp/a", "ls")).unwrap();
    ingest::ingest(&conn, &payload("s1", "/tmp/b", "pwd")).unwrap();
    let s = query::stats(&conn).unwrap();
    assert_eq!(s.total_rows, 2);
    assert_eq!(s.distinct_scopes, 2);
    assert_eq!(s.distinct_heads, 2);
}
