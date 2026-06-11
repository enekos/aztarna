use anyhow::{Context, Result};
use aztarna::{db, ingest, query, scope};
use clap::{Parser, Subcommand};
use std::io::Read;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "aztarna", version, about = "Per-cwd bash command ranker for Claude Code")]
struct Cli {
    /// Override the database path. Defaults to the per-user data dir.
    #[arg(long, global = true, env = "AZTARNA_DB")]
    db: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Initialize the database (runs implicitly on every command; this is for
    /// explicit "where will it live?" use).
    Init,

    /// Ingest a Claude Code PostToolUse hook payload from stdin.
    Log {
        /// Echo a one-line summary of what was logged (off by default to keep
        /// hook output quiet).
        #[arg(long)]
        verbose: bool,
    },

    /// List the top-ranked commands for a scope, decay-weighted.
    Top {
        /// cwd to derive scope from (defaults to current working dir).
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Override scope directly (skips git-root resolution).
        #[arg(long, conflicts_with = "cwd")]
        scope: Option<PathBuf>,
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value_t = query::DEFAULT_HALF_LIFE_DAYS)]
        half_life_days: f64,
        /// Emit JSON instead of a human table.
        #[arg(long)]
        json: bool,
    },

    /// List the most common (prev -> next) command transitions for a scope.
    Sequences {
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long, conflicts_with = "cwd")]
        scope: Option<PathBuf>,
        #[arg(short = 'n', long, default_value_t = 15)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },

    /// Emit a markdown context block ready for a SessionStart hook.
    Context {
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long, conflicts_with = "cwd")]
        scope: Option<PathBuf>,
        #[arg(long, default_value_t = 12)]
        top: usize,
        #[arg(long, default_value_t = 8)]
        sequences: usize,
        #[arg(long, default_value_t = query::DEFAULT_HALF_LIFE_DAYS)]
        half_life_days: f64,
    },

    /// Database stats — useful for sanity checks.
    Stats,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db_path = match &cli.db {
        Some(p) => p.clone(),
        None => db::default_db_path()?,
    };
    let conn = db::open(&db_path)?;

    match cli.cmd {
        Cmd::Init => {
            println!("aztarna db ready at {}", db_path.display());
        }
        Cmd::Log { verbose } => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("read stdin")?;
            if buf.trim().is_empty() {
                return Ok(());
            }
            let payload: ingest::HookPayload =
                serde_json::from_str(&buf).context("parse PostToolUse payload as JSON")?;
            match ingest::ingest(&conn, &payload)? {
                Some(id) if verbose => {
                    eprintln!("aztarna: logged row {id}");
                }
                _ => {}
            }
        }
        Cmd::Top {
            cwd,
            scope,
            limit,
            half_life_days,
            json,
        } => {
            let scope_path = resolve_scope(cwd, scope)?;
            let rows = query::top(&conn, &scope_path, half_life_days, limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                print_top(&scope_path, &rows);
            }
        }
        Cmd::Sequences {
            cwd,
            scope,
            limit,
            json,
        } => {
            let scope_path = resolve_scope(cwd, scope)?;
            let rows = query::sequences(&conn, &scope_path, limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                print_sequences(&scope_path, &rows);
            }
        }
        Cmd::Context {
            cwd,
            scope,
            top,
            sequences,
            half_life_days,
        } => {
            let scope_path = resolve_scope(cwd, scope)?;
            let block = query::context_block(&conn, &scope_path, half_life_days, top, sequences)?;
            print!("{block}");
        }
        Cmd::Stats => {
            let s = query::stats(&conn)?;
            println!("{}", serde_json::to_string_pretty(&s)?);
        }
    }
    Ok(())
}

fn resolve_scope(cwd: Option<PathBuf>, explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(s) = explicit {
        return Ok(s);
    }
    let cwd = match cwd {
        Some(p) => p,
        None => std::env::current_dir()?,
    };
    Ok(scope::resolve(&cwd))
}

fn print_top(scope: &std::path::Path, rows: &[query::TopRow]) {
    if rows.is_empty() {
        println!("(no commands logged for scope {})", scope.display());
        return;
    }
    println!("scope: {}", scope.display());
    println!("{:>3}  {:>8}  {:>5}  {:<24}  {}", "#", "score", "hits", "head", "last");
    for (i, r) in rows.iter().enumerate() {
        let last = truncate(&r.last_command, 60);
        println!(
            "{:>3}  {:>8.3}  {:>5}  {:<24}  {}",
            i + 1,
            r.score,
            r.hits,
            r.head,
            last
        );
    }
}

fn print_sequences(scope: &std::path::Path, rows: &[query::SequenceRow]) {
    if rows.is_empty() {
        println!("(no sequences for scope {})", scope.display());
        return;
    }
    println!("scope: {}", scope.display());
    println!("{:>5}  {:<24} -> {}", "hits", "prev", "next");
    for r in rows {
        println!("{:>5}  {:<24} -> {}", r.hits, r.prev, r.next);
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let cut: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}
