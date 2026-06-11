//! Ingest path: parse a Claude Code PostToolUse hook payload from stdin and
//! insert one row per Bash invocation.
//!
//! Hook payload shape (subset we use):
//! {
//!   "session_id": "...",
//!   "cwd": "/abs/path",
//!   "tool_name": "Bash",
//!   "tool_input":  { "command": "..." },
//!   "tool_response": { "exit_code": 0, "interrupted": false, ... }
//! }

use anyhow::Result;
use chrono::Utc;
use rusqlite::{Connection, params};
use serde::Deserialize;
use std::path::PathBuf;

use crate::{normalize, scope};

#[derive(Debug, Deserialize)]
pub struct HookPayload {
    #[serde(default)]
    pub session_id: Option<String>,
    pub cwd: String,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: ToolInput,
    #[serde(default)]
    pub tool_response: ToolResponse,
}

#[derive(Debug, Default, Deserialize)]
pub struct ToolInput {
    #[serde(default)]
    pub command: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ToolResponse {
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub interrupted: Option<bool>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
}

/// Returns Ok(None) if the payload is not a Bash tool invocation or has no
/// command (which is normal — the hook fires for many tools, we only log Bash).
pub fn ingest(conn: &Connection, payload: &HookPayload) -> Result<Option<i64>> {
    let tool_ok = payload
        .tool_name
        .as_deref()
        .map(|n| n == "Bash")
        .unwrap_or(true); // if tool_name absent, assume caller filtered
    if !tool_ok {
        return Ok(None);
    }
    let Some(command) = payload.tool_input.command.as_deref() else {
        return Ok(None);
    };
    let mut command = command.trim().to_string();
    if command.is_empty() {
        return Ok(None);
    }

    let mut cwd = PathBuf::from(&payload.cwd);

    // Iteratively peel `cd <path> &&` prefixes so the scope and command
    // reflect the actual working directory (e.g. `cd /proj && cargo test`).
    while let Some((new_cwd, rest)) = normalize::peel_cd(&command, &cwd) {
        cwd = new_cwd;
        command = rest;
    }

    if command.is_empty() {
        return Ok(None);
    }

    let scope_path = scope::resolve(&cwd);

    let head = normalize::head(&command);

    // Heuristic: directory navigation is noise for command-ranking purposes.
    if head == "cd" {
        return Ok(None);
    }

    let now = Utc::now().timestamp();

    conn.execute(
        r#"INSERT INTO commands
           (ts, session_id, cwd, scope, command, head, exit_code, interrupted, duration_ms)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
        params![
            now,
            payload.session_id,
            cwd.to_string_lossy(),
            scope_path.to_string_lossy(),
            command,
            head,
            payload.tool_response.exit_code,
            payload.tool_response.interrupted.unwrap_or(false) as i64,
            payload.tool_response.duration_ms,
        ],
    )?;
    Ok(Some(conn.last_insert_rowid()))
}
