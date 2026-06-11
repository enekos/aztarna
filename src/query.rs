//! Ranking + retrieval queries.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{Connection, params};
use serde::Serialize;
use std::path::Path;

/// Half-life of 14 days: a command run 14 days ago counts half as much as
/// one run today. Picked so a weekly cadence still dominates ranking but
/// last-quarter habits fade.
pub const DEFAULT_HALF_LIFE_DAYS: f64 = 14.0;

#[derive(Debug, Serialize)]
pub struct TopRow {
    pub head: String,
    pub score: f64,
    pub hits: i64,
    pub last_command: String,
}

#[derive(Debug, Serialize)]
pub struct SequenceRow {
    pub prev: String,
    pub next: String,
    pub hits: i64,
}

pub fn top(
    conn: &Connection,
    scope: &Path,
    half_life_days: f64,
    limit: usize,
) -> Result<Vec<TopRow>> {
    let now = Utc::now().timestamp();
    let half_life_secs = half_life_days * 86_400.0;

    // For each head: decay-weighted score, raw hit count, and the most recent
    // full command for that head (helpful when surfacing examples to humans).
    let mut stmt = conn.prepare(
        r#"
        WITH scored AS (
            SELECT
                head,
                command,
                ts,
                decay_score(CAST(?1 - ts AS REAL), ?2) AS w
            FROM commands
            WHERE scope = ?3
        ),
        agg AS (
            SELECT
                head,
                SUM(w)     AS score,
                COUNT(*)   AS hits,
                MAX(ts)    AS last_ts
            FROM scored
            GROUP BY head
        )
        SELECT
            agg.head,
            agg.score,
            agg.hits,
            (SELECT command FROM commands
              WHERE scope = ?3 AND head = agg.head
              ORDER BY ts DESC LIMIT 1) AS last_command
        FROM agg
        ORDER BY agg.score DESC, agg.hits DESC
        LIMIT ?4
        "#,
    )?;

    let rows = stmt.query_map(
        params![now, half_life_secs, scope.to_string_lossy(), limit as i64],
        |row| {
            Ok(TopRow {
                head: row.get(0)?,
                score: row.get(1)?,
                hits: row.get(2)?,
                last_command: row.get(3)?,
            })
        },
    )?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Most common (prev_head -> next_head) transitions inside a single session,
/// scoped by either the previous or next row's scope. Self-transitions are
/// filtered out — they're usually noise (re-running the same `ls`).
pub fn sequences(
    conn: &Connection,
    scope: &Path,
    limit: usize,
) -> Result<Vec<SequenceRow>> {
    let mut stmt = conn.prepare(
        r#"
        WITH ordered AS (
            SELECT
                id, session_id, head, scope,
                LAG(head)  OVER (PARTITION BY session_id ORDER BY id) AS prev_head,
                LAG(scope) OVER (PARTITION BY session_id ORDER BY id) AS prev_scope
            FROM commands
            WHERE session_id IS NOT NULL
        )
        SELECT prev_head, head AS next_head, COUNT(*) AS hits
        FROM ordered
        WHERE prev_head IS NOT NULL
          AND prev_head <> head
          AND (scope = ?1 OR prev_scope = ?1)
        GROUP BY prev_head, next_head
        ORDER BY hits DESC, next_head
        LIMIT ?2
        "#,
    )?;

    let rows = stmt.query_map(params![scope.to_string_lossy(), limit as i64], |row| {
        Ok(SequenceRow {
            prev: row.get(0)?,
            next: row.get(1)?,
            hits: row.get(2)?,
        })
    })?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct Stats {
    pub total_rows: i64,
    pub distinct_scopes: i64,
    pub distinct_heads: i64,
    pub oldest_ts: Option<i64>,
    pub newest_ts: Option<i64>,
}

pub fn stats(conn: &Connection) -> Result<Stats> {
    let (total_rows, distinct_scopes, distinct_heads, oldest_ts, newest_ts): (
        i64,
        i64,
        i64,
        Option<i64>,
        Option<i64>,
    ) = conn.query_row(
        r#"SELECT
              COUNT(*),
              COUNT(DISTINCT scope),
              COUNT(DISTINCT head),
              MIN(ts), MAX(ts)
           FROM commands"#,
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    Ok(Stats {
        total_rows,
        distinct_scopes,
        distinct_heads,
        oldest_ts,
        newest_ts,
    })
}

/// Human-readable context block intended for a SessionStart hook to inject
/// into the model's context. Empty string when there's nothing to say.
pub fn context_block(
    conn: &Connection,
    scope: &Path,
    half_life_days: f64,
    top_n: usize,
    seq_n: usize,
) -> Result<String> {
    let tops = top(conn, scope, half_life_days, top_n)?;
    if tops.is_empty() {
        return Ok(String::new());
    }
    let seqs = sequences(conn, scope, seq_n)?;

    let mut out = String::new();
    out.push_str(&format!(
        "## Frequently-used commands in {}\n\n",
        scope.display()
    ));
    out.push_str("Ranked by recency-weighted frequency. Prefer these forms when they fit the task.\n\n");
    for (i, row) in tops.iter().enumerate() {
        let example = truncate_one_line(&row.last_command, 80);
        let noun = if row.hits == 1 { "run" } else { "runs" };
        out.push_str(&format!(
            "{:>2}. `{}` — {} {} (example: `{}`)\n",
            i + 1,
            row.head,
            row.hits,
            noun,
            example
        ));
    }
    if !seqs.is_empty() {
        out.push_str("\n### Common follow-ups\n\n");
        for s in &seqs {
            out.push_str(&format!("- `{}` → `{}` ({}×)\n", s.prev, s.next, s.hits));
        }
    }
    Ok(out)
}

fn truncate_one_line(s: &str, n: usize) -> String {
    // Collapse newlines so multi-line commands stay one row, then hard-truncate.
    let one_line: String = s
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ; ");
    if one_line.chars().count() <= n {
        one_line
    } else {
        let cut: String = one_line.chars().take(n.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}
