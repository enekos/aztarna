use anyhow::{Context, Result};
use directories::ProjectDirs;
use rusqlite::{Connection, functions::FunctionFlags};
use std::path::{Path, PathBuf};

/// Default database path under the user's data dir, e.g.
/// `~/Library/Application Support/dev.eneko.aztarna/aztarna.sqlite` on macOS.
pub fn default_db_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("dev", "eneko", "aztarna")
        .context("could not resolve a data directory for aztarna")?;
    let data_dir = dirs.data_dir();
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("create data dir at {}", data_dir.display()))?;
    Ok(data_dir.join("aztarna.sqlite"))
}

pub fn open(path: &Path) -> Result<Connection> {
    let conn =
        Connection::open(path).with_context(|| format!("open sqlite at {}", path.display()))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    register_funcs(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS commands (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            ts           INTEGER NOT NULL,
            session_id   TEXT,
            cwd          TEXT NOT NULL,
            scope        TEXT NOT NULL,
            command      TEXT NOT NULL,
            head         TEXT NOT NULL,
            exit_code    INTEGER,
            interrupted  INTEGER NOT NULL DEFAULT 0,
            duration_ms  INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_commands_scope ON commands(scope);
        CREATE INDEX IF NOT EXISTS idx_commands_cwd   ON commands(cwd);
        CREATE INDEX IF NOT EXISTS idx_commands_head  ON commands(head);
        CREATE INDEX IF NOT EXISTS idx_commands_session_id ON commands(session_id, id);
        CREATE INDEX IF NOT EXISTS idx_commands_ts    ON commands(ts);

        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        INSERT OR IGNORE INTO meta(key, value) VALUES ('schema_version', '1');
        "#,
    )?;
    Ok(())
}

/// Registers `decay_score(age_seconds, half_life_seconds)` returning a
/// floating-point weight in (0, 1]. Used by ranking queries.
fn register_funcs(conn: &Connection) -> Result<()> {
    conn.create_scalar_function(
        "decay_score",
        2,
        FunctionFlags::SQLITE_DETERMINISTIC | FunctionFlags::SQLITE_UTF8,
        |ctx| {
            let age: f64 = ctx.get(0)?;
            let half_life: f64 = ctx.get(1)?;
            if half_life <= 0.0 {
                return Ok(1.0);
            }
            let lambda = std::f64::consts::LN_2 / half_life;
            Ok((-lambda * age.max(0.0)).exp())
        },
    )?;
    Ok(())
}
