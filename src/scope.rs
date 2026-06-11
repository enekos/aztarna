//! "Scope" is the logical grouping for ranking. By default it's the git
//! repository root of the current working directory, so a session in
//! `~/eneko_projects/aztarna/src/` ranks against everything ever run in
//! `~/eneko_projects/aztarna/`. Falls back to the cwd itself when not in
//! a git repo.

use std::path::{Path, PathBuf};

pub fn resolve(cwd: &Path) -> PathBuf {
    git_root(cwd).unwrap_or_else(|| cwd.to_path_buf())
}

fn git_root(start: &Path) -> Option<PathBuf> {
    let mut cur = start;
    loop {
        if cur.join(".git").exists() {
            return Some(cur.to_path_buf());
        }
        cur = cur.parent()?;
    }
}
