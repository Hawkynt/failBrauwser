//! The hosts of the user's OpenSSH client configuration (`~/.ssh/config`, `ssh_config(5)`), for
//! the Add Server form: the `Host` names to pick from, and what OpenSSH makes of one of them
//! (where it really connects, as whom, with which key) to show beside it.
//!
//! Connecting is left to OpenSSH itself on Linux: GVfs runs `ssh` with the name as given, so
//! everything the configuration says (keys, jump hosts, options this reader knows nothing about)
//! applies. What is read here only has to be right for display, and for filling the form on
//! Windows, whose own client does not read the file. Read as `ssh_config(5)` says: the first
//! value obtained for a keyword wins, `Host` patterns with `*`, `?` and `!`, `Include` with
//! wildcards relative to `~/.ssh`; `Match` blocks are skipped, since what they test (the
//! command line, the network) is not known here.

use std::path::{Path, PathBuf};

/// One line of a configuration: a keyword (lower case) and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    key: String,
    args: Vec<String>,
}

/// What OpenSSH makes of a `Host` name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resolved {
    /// The name picked.
    pub alias: String,
    /// Where it connects (`HostName`, else the name itself).
    pub host_name: String,
    /// `User`, when the configuration names one.
    pub user: Option<String>,
    /// `Port`, when the configuration names one.
    pub port: Option<u16>,
    /// `IdentityFile`s, `~` and `%d %h %r %u %%` expanded.
    pub identity_files: Vec<String>,
    /// `ProxyJump`, when set.
    pub proxy_jump: Option<String>,
}

impl Resolved {
    /// "whisper@10.0.0.5:2222, key ~/.ssh/neon, through bastion" — what the form shows.
    pub fn summary(&self, home: &Path) -> String {
        let mut s = match &self.user {
            Some(u) => format!("{u}@{}", self.host_name),
            None => self.host_name.clone(),
        };
        if let Some(p) = self.port {
            s.push_str(&format!(":{p}"));
        }
        if let Some(key) = self.identity_files.first() {
            let shown = Path::new(key).strip_prefix(home).map(|r| format!("~/{}", r.display())).unwrap_or_else(|_| key.clone());
            s.push_str(&format!(", key {shown}"));
        }
        if let Some(j) = &self.proxy_jump {
            s.push_str(&format!(", through {j}"));
        }
        s
    }
}

/// The configuration file read: `~/.ssh/config` (the tests name another one through
/// `FAILBRAUWSER_SSH_CONFIG`).
pub fn path() -> PathBuf {
    std::env::var_os("FAILBRAUWSER_SSH_CONFIG").filter(|p| !p.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(".ssh").join("config"))
}

fn home() -> PathBuf {
    gtk::glib::home_dir()
}

/// The `Host` names of the user's configuration that name one machine (no wildcards), in the
/// order they are written. Empty when there is no configuration.
pub fn hosts() -> Vec<String> {
    let lines = load(&path(), &home());
    host_names(&lines)
}

/// What OpenSSH makes of `alias` under the user's configuration; `None` when the configuration
/// does not name it in a `Host` line of its own.
pub fn resolve(alias: &str) -> Option<Resolved> {
    let lines = load(&path(), &home());
    host_names(&lines).iter().any(|h| h.eq_ignore_ascii_case(alias)).then(|| resolve_in(&lines, alias, &home(), &local_user()))
}

fn local_user() -> String {
    gtk::glib::user_name().to_string_lossy().into_owned()
}

fn host_names(lines: &[Line]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in lines.iter().filter(|l| l.key == "host") {
        for a in &l.args {
            if !a.contains(['*', '?', '!']) && !out.iter().any(|o| o.eq_ignore_ascii_case(a)) {
                out.push(a.clone());
            }
        }
    }
    out
}

/// The lines of the file at `file` with every `Include` put in its place (at most 16 deep, as
/// OpenSSH allows).
fn load(file: &Path, home: &Path) -> Vec<Line> {
    let mut out = Vec::new();
    load_into(file, home, 0, &mut out);
    out
}

fn load_into(file: &Path, home: &Path, depth: usize, out: &mut Vec<Line>) {
    let Ok(text) = std::fs::read_to_string(file) else { return };
    for line in parse(&text) {
        if line.key == "include" {
            if depth < 16 {
                for pattern in &line.args {
                    for f in include_files(pattern, home) {
                        load_into(&f, home, depth + 1, out);
                    }
                }
            }
        } else {
            out.push(line);
        }
    }
}

/// The files an `Include` names: `~` is the home folder, a relative path is relative to
/// `~/.ssh`, and wildcards in the last part of the path match files there (sorted, as glob(3)).
fn include_files(pattern: &str, home: &Path) -> Vec<PathBuf> {
    let p = if let Some(rest) = pattern.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        home.join(".ssh").join(pattern)
    };
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !name.contains(['*', '?']) {
        return vec![p];
    }
    let Some(dir) = p.parent() else { return Vec::new() };
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| d.flatten().filter(|e| wildcard(&name, &e.file_name().to_string_lossy()) && e.path().is_file()).map(|e| e.path()).collect())
        .unwrap_or_default();
    found.sort();
    found
}

/// The configuration's lines: `Keyword args` or `Keyword=args`, `#` lines and blank lines left
/// out, arguments in double quotes kept whole.
fn parse(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let split = line.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(line.len());
        let key = line[..split].to_ascii_lowercase();
        let rest = line[split..].trim_start();
        let rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
        out.push(Line { key, args: words(rest) });
    }
    out
}

fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut any) = (false, false);
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            // A comment after the arguments.
            '#' if !quoted && !any => break,
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// `*` and `?` as `ssh_config(5)` PATTERNS have them, without regard to case.
fn wildcard(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.to_lowercase().chars().collect(), text.to_lowercase().chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// Whether a `Host` line's patterns take `host`: one of them matches it and no negated one does.
fn host_matches(patterns: &[String], host: &str) -> bool {
    let mut hit = false;
    for p in patterns {
        if let Some(neg) = p.strip_prefix('!') {
            if wildcard(neg, host) {
                return false;
            }
        } else if wildcard(p, host) {
            hit = true;
        }
    }
    hit
}

fn resolve_in(lines: &[Line], alias: &str, home: &Path, local_user: &str) -> Resolved {
    // Lines before the first Host or Match apply to every host.
    let mut active = true;
    let (mut host_name, mut user, mut port, mut jump) = (None::<String>, None::<String>, None::<u16>, None::<String>);
    let mut keys: Vec<String> = Vec::new();
    for l in lines {
        match l.key.as_str() {
            "host" => active = host_matches(&l.args, alias),
            "match" => active = false,
            _ if !active => {}
            "hostname" => {
                host_name.get_or_insert_with(|| l.args.first().cloned().unwrap_or_default());
            }
            "user" => {
                user.get_or_insert_with(|| l.args.first().cloned().unwrap_or_default());
            }
            "port" => {
                if port.is_none() {
                    port = l.args.first().and_then(|p| p.parse().ok());
                }
            }
            "proxyjump" => {
                jump.get_or_insert_with(|| l.args.first().cloned().unwrap_or_default());
            }
            "identityfile" => keys.extend(l.args.first().cloned()),
            _ => {}
        }
    }
    let host_name = host_name.map(|h| h.replace("%h", alias).replace("%%", "%")).filter(|h| !h.is_empty()).unwrap_or_else(|| alias.to_string());
    let remote_user = user.clone().unwrap_or_else(|| local_user.to_string());
    let expand = |k: &str| {
        let k = match k.strip_prefix("~/") {
            Some(rest) => format!("{}/{rest}", home.display()),
            None => k.to_string(),
        };
        // `%%` is a percent sign, never the start of another token.
        k.replace("%%", "\u{0}").replace("%d", &home.display().to_string()).replace("%h", &host_name).replace("%r", &remote_user).replace("%u", local_user).replace('\u{0}', "%")
    };
    Resolved {
        alias: alias.to_string(),
        host_name: host_name.clone(),
        user,
        port,
        identity_files: keys.iter().map(|k| expand(k)).collect(),
        proxy_jump: jump.filter(|j| !j.eq_ignore_ascii_case("none")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "\
# defaults
ServerAliveInterval 30

Host neon neon_root
    HostName 10.0.0.5
    IdentityFile ~/.ssh/neon_%r

Host neon_root
    User root

Host neon
    User whisper
    Port=2222

Host \"lab box\" *.lab !gate.lab
    HostName %h.example.org
    ProxyJump gate.lab

Host *
    User nobody
    Port 22
    IdentityFile %d/.ssh/default_key

Match host neon
    User matched
";

    fn lines() -> Vec<Line> {
        parse(CONFIG)
    }

    #[test]
    fn the_host_names_a_user_can_pick_are_those_without_wildcards() {
        assert_eq!(host_names(&lines()), ["neon", "neon_root", "lab box"]);
    }

    #[test]
    fn the_first_value_obtained_wins_and_identity_files_add_up() {
        let home = Path::new("/home/me");
        let r = resolve_in(&lines(), "neon", home, "me");
        assert_eq!(r.host_name, "10.0.0.5");
        assert_eq!(r.user.as_deref(), Some("whisper"), "the Host neon block comes before Host * and Match");
        assert_eq!(r.port, Some(2222), "Keyword=value is read like Keyword value");
        assert_eq!(r.identity_files, ["/home/me/.ssh/neon_whisper", "/home/me/.ssh/default_key"]);
        assert_eq!(r.summary(home), "whisper@10.0.0.5:2222, key ~/.ssh/neon_whisper");

        let root = resolve_in(&lines(), "neon_root", home, "me");
        assert_eq!((root.user.as_deref(), root.port), (Some("root"), Some(22)));
        assert_eq!(root.identity_files[0], "/home/me/.ssh/neon_root");
    }

    #[test]
    fn patterns_take_wildcards_and_negations() {
        let home = Path::new("/home/me");
        let r = resolve_in(&lines(), "db.lab", home, "me");
        assert_eq!(r.host_name, "db.lab.example.org", "%h is the name asked for");
        assert_eq!(r.proxy_jump.as_deref(), Some("gate.lab"));
        let gate = resolve_in(&lines(), "gate.lab", home, "me");
        assert_eq!(gate.host_name, "gate.lab", "a negated pattern keeps the block out");
        assert_eq!(gate.proxy_jump, None);
        assert_eq!(resolve_in(&lines(), "lab box", home, "me").host_name, "lab box.example.org", "a quoted name stays one argument");
        assert!(wildcard("n?on*", "NEON_root") && !wildcard("neon", "neon2"));
    }

    #[test]
    fn a_host_the_configuration_does_not_name_connects_as_itself() {
        let r = resolve_in(&parse("Host other\n  HostName x\n"), "pi.local", Path::new("/h"), "me");
        assert_eq!(r, Resolved { alias: "pi.local".into(), host_name: "pi.local".into(), ..Default::default() });
    }

    #[test]
    fn includes_are_read_in_place_relative_to_the_ssh_folder() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir_all(ssh.join("config.d")).unwrap();
        std::fs::write(ssh.join("config"), "Include config.d/*.conf\nHost late\n  User late\n").unwrap();
        std::fs::write(ssh.join("config.d/b.conf"), "Host bravo\n  HostName b.example\n").unwrap();
        std::fs::write(ssh.join("config.d/a.conf"), "Host alpha\n  HostName a.example\n").unwrap();
        std::fs::write(ssh.join("config.d/skip.txt"), "Host skipped\n").unwrap();
        let lines = load(&ssh.join("config"), home.path());
        assert_eq!(host_names(&lines), ["alpha", "bravo", "late"]);
        assert_eq!(resolve_in(&lines, "bravo", home.path(), "me").host_name, "b.example");
        // An Include that includes itself stops at the depth limit instead of looping.
        std::fs::write(ssh.join("config"), "Include config\nHost loop\n").unwrap();
        assert_eq!(host_names(&load(&ssh.join("config"), home.path())), ["loop"]);
    }

    #[test]
    fn no_configuration_is_no_hosts() {
        assert!(host_names(&load(Path::new("/nonexistent/ssh_config"), Path::new("/nonexistent"))).is_empty());
    }
}
