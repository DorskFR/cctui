//! Structural argv matching for capability grants: no regex, the kernel's argv
//! is already tokenized.

use std::collections::BTreeMap;

use serde::Serialize;

struct GlobalFlag {
    name: &'static str,
    takes_value: bool,
    /// Changes what the binary runs (config, exec path): allowed only when the
    /// rule lists it in `allow_flags`.
    needs_grant: bool,
}

const fn global(name: &'static str, takes_value: bool, needs_grant: bool) -> GlobalFlag {
    GlobalFlag { name, takes_value, needs_grant }
}

const GIT_GLOBALS: &[GlobalFlag] = &[
    global("-C", true, false),
    global("--git-dir", true, false),
    global("--work-tree", true, false),
    global("--namespace", true, false),
    global("--no-pager", false, false),
    global("-P", false, false),
    global("-p", false, false),
    global("--paginate", false, false),
    global("--bare", false, false),
    global("--no-optional-locks", false, false),
    global("--literal-pathspecs", false, false),
    global("--no-replace-objects", false, false),
    global("--no-advice", false, false),
    global("-c", true, true),
    global("--config-env", true, true),
    global("--exec-path", false, true),
    global("--super-prefix", true, true),
];

const GH_GLOBALS: &[GlobalFlag] = &[global("-R", true, false), global("--repo", true, false)];

fn global_model(bin: &str) -> Option<&'static [GlobalFlag]> {
    match bin {
        "git" => Some(GIT_GLOBALS),
        "gh" => Some(GH_GLOBALS),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Miss {
    Subcommand,
    Argv(String),
    Flag(String),
}

impl Miss {
    const fn rank(&self) -> u8 {
        match self {
            Self::Subcommand => 0,
            Self::Argv(_) => 1,
            Self::Flag(_) => 2,
        }
    }
}

struct Invocation<'a> {
    gated_globals: Vec<&'static str>,
    rest: &'a [String],
}

fn split_globals<'a>(bin: &str, args: &'a [String]) -> Result<Invocation<'a>, Miss> {
    let Some(model) = global_model(bin) else {
        return Ok(Invocation { gated_globals: Vec::new(), rest: args });
    };
    let mut gated_globals = Vec::new();
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if arg == "--" || arg == "-" || !arg.starts_with('-') {
            break;
        }
        let (name, attached) =
            arg.split_once('=').map_or((arg.as_str(), false), |(n, _)| (n, true));
        let flag = model.iter().find(|f| f.name == name).ok_or_else(|| {
            Miss::Argv(format!("global flag {name} is not in the {bin} argv model"))
        })?;
        if flag.takes_value && !attached {
            i += 1;
            if i >= args.len() {
                return Err(Miss::Argv(format!("global flag {name} is missing its value")));
            }
        }
        if flag.needs_grant {
            gated_globals.push(flag.name);
        }
        i += 1;
    }
    Ok(Invocation { gated_globals, rest: &args[i..] })
}

/// One granted shape of a binary's argv.
///
/// `subcommands: None` grants the binary with any arguments. Otherwise argv
/// must start (after global flags) with one of the subcommands, given as
/// space-separated words (`"pr comment"`), and every later flag must be in
/// `allow_flags`; an entry ending in `=` (`"-m="`) takes a value. Arguments
/// after `--` and non-flag arguments are not inspected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecRule {
    pub capability: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subcommands: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow_flags: Vec<String>,
    #[serde(skip)]
    pub layer: String,
}

impl ExecRule {
    /// `Some(takes_value)` when `name` is allowlisted.
    fn flag(&self, name: &str) -> Option<bool> {
        self.allow_flags.iter().find_map(|f| match f.strip_suffix('=') {
            Some(stem) if stem == name => Some(true),
            None if f == name => Some(false),
            _ => None,
        })
    }

    fn check(&self, bin: &str, args: &[String]) -> Result<(), Miss> {
        let Some(subcommands) = &self.subcommands else {
            return Ok(());
        };
        let inv = split_globals(bin, args)?;
        let words = subcommands
            .iter()
            .map(|s| s.split_whitespace().collect::<Vec<_>>())
            .find(|w| {
                inv.rest.len() >= w.len() && w.iter().zip(inv.rest).all(|(a, b)| *a == b.as_str())
            })
            .map(|w| w.len())
            .ok_or(Miss::Subcommand)?;
        if let Some(global) = inv.gated_globals.iter().find(|g| self.flag(g).is_none()) {
            return Err(Miss::Flag((*global).to_string()));
        }
        self.check_flags(&inv.rest[words..])
    }

    fn check_flags(&self, args: &[String]) -> Result<(), Miss> {
        let mut i = 0;
        while let Some(arg) = args.get(i) {
            i += 1;
            if arg == "--" {
                break;
            }
            if let Some(long) = arg.strip_prefix("--") {
                let (stem, attached) =
                    long.split_once('=').map_or((long, false), |(n, _)| (n, true));
                let name = format!("--{stem}");
                match self.flag(&name) {
                    None => return Err(Miss::Flag(name)),
                    Some(true) if !attached => i += 1,
                    Some(_) => {}
                }
            } else if let Some(shorts) = arg.strip_prefix('-').filter(|s| !s.is_empty()) {
                let count = shorts.chars().count();
                for (k, c) in shorts.chars().enumerate() {
                    let name = format!("-{c}");
                    match self.flag(&name) {
                        None => return Err(Miss::Flag(name)),
                        Some(true) => {
                            if k + 1 == count {
                                i += 1;
                            }
                            break;
                        }
                        Some(false) => {}
                    }
                }
            }
        }
        Ok(())
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Every binary a step's capabilities grant, with the argv shapes allowed for
/// each. An exec of a binary absent from the table is denied.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ExecTable {
    bins: BTreeMap<String, Vec<ExecRule>>,
}

impl ExecTable {
    pub fn insert(&mut self, bin: &str, rule: ExecRule) {
        self.bins.entry(bin.to_string()).or_default().push(rule);
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.bins.is_empty()
    }

    #[must_use]
    pub fn rules(&self, bin: &str) -> &[ExecRule] {
        self.bins.get(bin).map_or(&[], Vec::as_slice)
    }

    /// The table as pretty JSON, for golden tests and diagnostics.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Judge an exec: `Ok` names the capability that grants it, `Err` says why
    /// none does. The binary comes from `path`; argv[0] is caller-chosen.
    pub fn check(&self, path: &str, argv: &[String]) -> Result<&str, String> {
        let bin = basename(path);
        let Some(rules) = self.bins.get(bin).filter(|r| !r.is_empty()) else {
            return Err(format!("`{bin}` is not granted by any capability of this step"));
        };
        let args = argv.get(1..).unwrap_or(&[]);
        let mut best: Option<(&ExecRule, Miss)> = None;
        for rule in rules {
            match rule.check(bin, args) {
                Ok(()) => return Ok(&rule.capability),
                Err(miss) => {
                    if best.as_ref().is_none_or(|(_, b)| miss.rank() > b.rank()) {
                        best = Some((rule, miss));
                    }
                }
            }
        }
        Err(match best {
            Some((rule, Miss::Flag(flag))) => {
                let allowed = if rule.allow_flags.is_empty() {
                    "no flags".to_string()
                } else {
                    rule.allow_flags.join(", ")
                };
                let cap = &rule.capability;
                format!("`{bin}`: flag {flag} is not granted by {cap} (allows {allowed})")
            }
            Some((_, Miss::Argv(reason))) => format!("`{bin}`: {reason}"),
            _ => {
                let granted: Vec<&str> = rules
                    .iter()
                    .filter_map(|r| r.subcommands.as_ref())
                    .flatten()
                    .map(String::as_str)
                    .collect();
                format!(
                    "`{bin}`: subcommand is not granted by this step's capabilities (granted: {})",
                    granted.join(", ")
                )
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(cap: &str, subs: Option<&[&str]>, flags: &[&str]) -> ExecRule {
        ExecRule {
            capability: cap.to_string(),
            subcommands: subs.map(|s| s.iter().map(ToString::to_string).collect()),
            allow_flags: flags.iter().map(ToString::to_string).collect(),
            layer: "test".to_string(),
        }
    }

    fn table(rules: &[(&str, ExecRule)]) -> ExecTable {
        let mut t = ExecTable::default();
        for (bin, r) in rules {
            t.insert(bin, r.clone());
        }
        t
    }

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    fn push_table() -> ExecTable {
        table(&[
            ("git", rule("git:push", Some(&["push"]), &["--set-upstream", "-u"])),
            ("git", rule("git:read", Some(&["status", "log"]), &["-n=", "--oneline"])),
        ])
    }

    fn check(t: &ExecTable, args: &[&str]) -> Result<String, String> {
        t.check("/usr/bin/git", &argv(args)).map(str::to_string)
    }

    #[test]
    fn git_dash_upper_c_value_is_not_read_as_the_subcommand() {
        let t = push_table();
        assert_eq!(check(&t, &["git", "-C", "/tmp", "push"]).unwrap(), "git:push");
        assert_eq!(check(&t, &["git", "-C", "/tmp/push", "status"]).unwrap(), "git:read");
        assert!(check(&t, &["git", "-C", "/tmp", "commit"]).is_err());
        assert!(check(&t, &["git", "-C"]).unwrap_err().contains("missing its value"));
    }

    #[test]
    fn git_no_pager_is_skipped() {
        let t = push_table();
        assert_eq!(check(&t, &["git", "--no-pager", "push", "-u", "origin"]).unwrap(), "git:push");
        let log = check(&t, &["git", "--git-dir=/w/.git", "--no-pager", "log"]);
        assert_eq!(log.unwrap(), "git:read");
    }

    #[test]
    fn git_push_force_is_an_unknown_flag() {
        let t = push_table();
        let err = check(&t, &["git", "push", "--force", "origin", "main"]).unwrap_err();
        assert!(err.contains("--force"), "{err}");
        assert!(err.contains("git:push"), "{err}");
        assert!(check(&t, &["git", "push", "--force-with-lease"]).is_err());
        assert!(check(&t, &["git", "push", "-f"]).is_err());
    }

    #[test]
    fn git_dash_c_consumes_its_value_and_needs_a_grant() {
        let t = push_table();
        let err = check(&t, &["git", "-c", "k=v", "push"]).unwrap_err();
        assert!(err.contains("flag -c"), "{err}");
        let granted = table(&[("git", rule("git:push", Some(&["push"]), &["-c"]))]);
        assert_eq!(check(&granted, &["git", "-c", "k=v", "push"]).unwrap(), "git:push");
        assert!(check(&granted, &["git", "-c", "push", "status"]).is_err());
    }

    #[test]
    fn end_of_options_stops_flag_checks() {
        let t = push_table();
        assert_eq!(check(&t, &["git", "push", "origin", "--", "--force"]).unwrap(), "git:push");
        assert!(check(&t, &["git", "push", "--force", "--", "x"]).is_err());
        assert!(check(&t, &["git", "--", "push"]).is_err());
    }

    #[test]
    fn bundled_short_flags_are_checked_one_by_one() {
        let t = table(&[("git", rule("git:commit", Some(&["commit"]), &["-a", "-v", "-m="]))]);
        assert!(check(&t, &["git", "commit", "-av"]).is_ok());
        let err = check(&t, &["git", "commit", "-anv"]).unwrap_err();
        assert!(err.contains("flag -n"), "{err}");
        assert!(check(&t, &["git", "commit", "-amfix-n"]).is_ok());
        assert!(check(&t, &["git", "commit", "-m", "-not-a-flag"]).is_ok());
        assert!(check(&t, &["git", "commit", "-am", "--amend"]).is_ok());
        assert!(check(&t, &["git", "commit", "-ma", "--amend"]).is_err());
    }

    #[test]
    fn long_flag_values_attached_or_separate() {
        let t = push_table();
        assert!(check(&t, &["git", "log", "-n", "5"]).is_ok());
        assert!(check(&t, &["git", "log", "--oneline=x"]).is_ok());
        assert!(check(&t, &["git", "log", "--format=%H"]).is_err());
    }

    #[test]
    fn unknown_flag_denies() {
        let t = push_table();
        let err = check(&t, &["git", "status", "--porcelain"]).unwrap_err();
        assert!(err.contains("flag --porcelain"), "{err}");
        assert!(err.contains("git:read"), "{err}");
    }

    #[test]
    fn unknown_global_flag_denies() {
        let t = push_table();
        let err = check(&t, &["git", "--frobnicate", "push"]).unwrap_err();
        assert!(err.contains("--frobnicate"), "{err}");
    }

    #[test]
    fn unknown_bin_denies() {
        let t = push_table();
        let err = t.check("/usr/bin/curl", &argv(&["curl", "x"])).unwrap_err();
        assert!(err.contains("`curl` is not granted"), "{err}");
        assert!(t.check("", &argv(&["git", "push"])).is_err());
    }

    #[test]
    fn the_exec_path_names_the_binary_not_argv0() {
        let t = push_table();
        assert!(t.check("/usr/bin/git", &argv(&["harmless", "push", "--force"])).is_err());
        assert!(t.check("/tmp/evil", &argv(&["git", "push"])).is_err());
        assert!(t.check("/usr/bin/git", &argv(&["anything", "push"])).is_ok());
    }

    #[test]
    fn ungranted_subcommand_lists_what_is_granted() {
        let err = check(&push_table(), &["git", "reset", "--hard"]).unwrap_err();
        assert!(err.contains("granted: push, status, log"), "{err}");
    }

    #[test]
    fn gh_multi_word_subcommands_and_repo_global() {
        let t = table(&[(
            "gh",
            rule("gh:comment", Some(&["pr comment", "issue comment"]), &["--body=", "-b="]),
        )]);
        let gh = |args: &[&str]| t.check("/usr/bin/gh", &argv(args)).map(str::to_string);
        assert!(gh(&["gh", "pr", "comment", "12", "--body", "-x"]).is_ok());
        assert!(gh(&["gh", "-R", "o/r", "issue", "comment", "3", "-b", "hi"]).is_ok());
        assert!(gh(&["gh", "pr", "merge", "12"]).is_err());
        assert!(gh(&["gh", "pr"]).is_err());
        assert!(gh(&["gh", "pr", "comment", "12", "--edit-last"]).is_err());
    }

    #[test]
    fn a_bin_only_rule_grants_any_arguments() {
        let t = table(&[("cargo", rule("make:test", None, &[]))]);
        assert!(t.check("/usr/bin/cargo", &argv(&["cargo", "test", "--all", "-q"])).is_ok());
        assert!(t.check("/usr/bin/cargo", &argv(&["cargo"])).is_ok());
    }

    #[test]
    fn unmodelled_bins_take_the_first_argument_as_subcommand() {
        let t = table(&[("make", rule("make:test", Some(&["test", "lint"]), &[]))]);
        assert!(t.check("/usr/bin/make", &argv(&["make", "test"])).is_ok());
        assert!(t.check("/usr/bin/make", &argv(&["make", "-C", "x", "test"])).is_err());
        assert!(t.check("/usr/bin/make", &argv(&["make", "deploy"])).is_err());
    }
}
