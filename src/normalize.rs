//! Command normalization. Turns a raw command line into a stable "head"
//! that groups similar invocations for ranking purposes.
//!
//! Examples:
//!   "cargo test --release -- --nocapture"  -> "cargo test"
//!   "git status -s"                        -> "git status"
//!   "ls"                                   -> "ls"
//!   "FOO=bar npm run dev"                  -> "npm run"
//!   "find . -name '*.rs' | head -20"       -> "find ."
//!   "sudo apt-get install -y curl"         -> "apt-get install"

const STRIP_PREFIXES: &[&str] = &["sudo", "env", "time", "nohup"];

/// Two-token verbs whose second word is meaningful (subcommands).
/// For other binaries, we keep just the first token to avoid over-splitting.
const TWO_TOKEN_VERBS: &[&str] = &[
    "cargo", "git", "npm", "pnpm", "yarn", "bun", "deno",
    "go", "gh", "docker", "kubectl", "make", "just",
    "uv", "pip", "poetry", "rye", "python", "python3",
    "node", "rake", "bundle", "rails", "mix", "brew",
    "aws", "gcloud", "az", "terraform", "tofu", "ansible",
    "psql", "mysql", "redis-cli", "sqlite3",
    "rustup", "rustc", "rg", "fd", "fzf", "tmux", "ssh",
    "apt", "apt-get", "dnf", "yum", "pacman", "apk",
    "systemctl", "journalctl", "launchctl",
    "claude", "anthropic",
];

pub fn head(command: &str) -> String {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Take leading chunk before pipe / and-or / sequence operators.
    let leading = split_leading(trimmed);
    let mut tokens = tokenize(leading);

    // Strip leading env-var assignments and wrapper commands. Interleave —
    // `env FOO=bar cmd` and `sudo FOO=bar cmd` both need to peel off two
    // different kinds of prefix before reaching the real command.
    loop {
        let Some(t) = tokens.first() else { break };
        if is_env_assignment(t) {
            tokens.remove(0);
            continue;
        }
        if STRIP_PREFIXES.contains(&t.as_str()) {
            tokens.remove(0);
            continue;
        }
        break;
    }

    if tokens.is_empty() {
        return String::new();
    }

    let first = basename(&tokens[0]);

    if TWO_TOKEN_VERBS.contains(&first.as_str()) {
        if let Some(second) = tokens.get(1) {
            // Skip flags as the "second token"; just use first if next is a flag.
            if !second.starts_with('-') {
                return format!("{first} {second}");
            }
        }
    }

    first
}

/// If the command starts with `cd <path> [chain-op] ...`, return the resolved
/// new cwd and the remaining command string. Handles iterative `cd` chains.
pub fn peel_cd(command: &str, cwd: &std::path::Path) -> Option<(std::path::PathBuf, String)> {
    let leading = split_leading(command);
    let tokens = tokenize(leading);

    if tokens.first()?.as_str() != "cd" {
        return None;
    }

    let path_arg = tokens.get(1)?;
    let new_cwd = cwd.join(path_arg);

    let rest = &command[leading.len()..];
    let rest = rest.trim_start();
    let rest = rest
        .strip_prefix("&&")
        .or_else(|| rest.strip_prefix("||"))
        .or_else(|| rest.strip_prefix(";"))
        .or_else(|| rest.strip_prefix("|"))
        .unwrap_or(rest)
        .trim_start();

    if rest.is_empty() {
        return None;
    }

    Some((new_cwd, rest.to_string()))
}

pub(crate) fn split_leading(s: &str) -> &str {
    // Conservative: split on ` | `, ` || `, ` && `, ` ; `. We require spaces
    // around the operator so we don't split inside paths or arguments.
    let candidates = [" | ", " || ", " && ", " ; ", ";"];
    let mut best = s.len();
    for c in &candidates {
        if let Some(idx) = s.find(c) {
            if idx < best {
                best = idx;
            }
        }
    }
    s[..best].trim_end()
}

pub(crate) fn tokenize(s: &str) -> Vec<String> {
    // Cheap shell-ish tokenization. Respects single + double quotes; does NOT
    // handle escapes or nesting — good enough for the head, which only looks
    // at the first 1–2 tokens.
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for ch in s.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (None, c) if c == '\'' || c == '"' => quote = Some(c),
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            (_, c) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn is_env_assignment(t: &str) -> bool {
    if let Some(eq) = t.find('=') {
        let name = &t[..eq];
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            && name.chars().next().is_some_and(|c| !c.is_ascii_digit())
    } else {
        false
    }
}

fn basename(t: &str) -> String {
    t.rsplit('/').next().unwrap_or(t).to_string()
}

#[cfg(test)]
mod tests {
    use super::head;

    #[test]
    fn simple() {
        assert_eq!(head("ls"), "ls");
        assert_eq!(head("ls -la"), "ls");
    }

    #[test]
    fn two_token_verbs() {
        assert_eq!(head("cargo test --release -- --nocapture"), "cargo test");
        assert_eq!(head("git status -s"), "git status");
        assert_eq!(head("npm run dev"), "npm run");
        assert_eq!(head("gh pr list"), "gh pr");
    }

    #[test]
    fn strip_env_and_wrappers() {
        assert_eq!(head("FOO=bar npm run dev"), "npm run");
        assert_eq!(head("FOO=bar BAR=baz cargo test"), "cargo test");
        assert_eq!(head("sudo apt-get install -y curl"), "apt-get install");
        assert_eq!(head("env RUST_LOG=debug cargo run"), "cargo run");
    }

    #[test]
    fn pipes_and_chains() {
        assert_eq!(head("find . -name '*.rs' | head -20"), "find");
        assert_eq!(head("cargo build && cargo test"), "cargo build");
        assert_eq!(head("ls; pwd"), "ls");
    }

    #[test]
    fn absolute_paths() {
        assert_eq!(head("/usr/local/bin/cargo test"), "cargo test");
        assert_eq!(head("./scripts/run.sh"), "run.sh");
    }

    #[test]
    fn flag_as_second_token() {
        // cargo with only a flag (rare but possible) → just "cargo"
        assert_eq!(head("cargo --version"), "cargo");
    }

    #[test]
    fn empty() {
        assert_eq!(head(""), "");
        assert_eq!(head("   "), "");
    }
}
