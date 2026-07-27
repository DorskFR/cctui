//! Layered tool-set resolution: CLI rules files, prompt `[rules]` imports, and
//! inline prompt definitions merged into one set map with source provenance.
//!
//! Precedence, lowest to highest: `--rules-base` < `--rules` < each `[rules]`
//! import in authored order < inline prompt definitions. A later layer that
//! redefines a set with `[name]:` replaces it and `[name]+:` extends it, so the
//! prompt author's inline definitions always win — the prompt owns its control
//! surface.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::ir::Workflow;
use crate::parser::rules_definitions;

/// The merged sets plus, for every set, the label of the layer that last wrote
/// it (`--explain` provenance) and any unreadable `[rules]` imports.
#[derive(Debug, Default, Clone)]
pub struct ResolvedSets {
    pub sets: HashMap<String, Vec<String>>,
    pub provenance: BTreeMap<String, String>,
    pub import_errors: Vec<String>,
}

fn apply_defs(resolved: &mut ResolvedSets, defs: &[(String, Vec<String>, bool)], source: &str) {
    for (name, members, extend) in defs {
        if *extend {
            resolved.sets.entry(name.clone()).or_default().extend(members.iter().cloned());
        } else {
            resolved.sets.insert(name.clone(), members.clone());
        }
        resolved.provenance.insert(name.clone(), source.to_string());
    }
}

fn apply_file(resolved: &mut ResolvedSets, path: &Path, source: &str) {
    if let Ok(text) = std::fs::read_to_string(path) {
        apply_defs(resolved, &rules_definitions(&text), source);
    }
}

/// Resolve the effective tool sets for `workflow` loaded from `prompt`, layering
/// the CLI rules files under the prompt's `[rules]` imports and inline sets.
#[must_use]
pub fn resolve_sets(
    rules_base: Option<&Path>,
    rules: &Path,
    prompt: &Path,
    workflow: &Workflow,
) -> ResolvedSets {
    let mut resolved = ResolvedSets::default();

    if let Some(base) = rules_base.filter(|p| p.exists()) {
        apply_file(&mut resolved, base, "--rules-base");
    }
    if rules.exists() {
        apply_file(&mut resolved, rules, "--rules");
    }

    let base_dir = prompt.parent().filter(|p| !p.as_os_str().is_empty());
    for import in &workflow.rules {
        let path = base_dir.map_or_else(|| Path::new(import).to_path_buf(), |d| d.join(import));
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                apply_defs(&mut resolved, &rules_definitions(&text), &format!("[rules] {import}"));
            }
            Err(e) => resolved
                .import_errors
                .push(format!("[rules] import '{import}' is unreadable ({}): {e}", path.display())),
        }
    }

    let inline: Vec<(String, Vec<String>, bool)> =
        workflow.sets.iter().map(|s| (s.name.clone(), s.members.clone(), s.extend)).collect();
    apply_defs(&mut resolved, &inline, "inline");

    resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        path
    }

    #[test]
    fn inline_wins_over_import_wins_over_cli() {
        let dir = tempfile::tempdir().unwrap();
        let rules = write(dir.path(), "guard-rules.md", "[net-x]: cli.example:443\n[keep]: cli\n");
        write(dir.path(), "net-common.md", "[net-x]: import.example:443\n");
        let prompt = write(
            dir.path(),
            "task.md",
            "[rules]: ./net-common.md\n[net-x]: inline.example:443\n\n# Step 1\n[transition]: Exit\n",
        );
        let wf = Workflow::compile(&std::fs::read_to_string(&prompt).unwrap()).unwrap();

        let resolved = resolve_sets(None, &rules, &prompt, &wf);
        assert_eq!(resolved.sets["net-x"], vec!["inline.example:443".to_string()]);
        assert_eq!(resolved.sets["keep"], vec!["cli".to_string()]);
        assert_eq!(resolved.provenance["net-x"], "inline");
        assert_eq!(resolved.provenance["keep"], "--rules");
        assert!(resolved.import_errors.is_empty());
    }

    #[test]
    fn import_resolves_relative_to_the_prompt() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("prompts")).unwrap();
        write(dir.path(), "shared.md", "[net-shared]: shared.example:443\n");
        let prompt = write(
            dir.path(),
            "prompts/task.md",
            "[rules]: ../shared.md\n\n# Step 1\n[transition]: Exit\n",
        );
        let wf = Workflow::compile(&std::fs::read_to_string(&prompt).unwrap()).unwrap();
        let resolved = resolve_sets(None, Path::new("/nonexistent"), &prompt, &wf);
        assert_eq!(resolved.sets["net-shared"], vec!["shared.example:443".to_string()]);
        assert!(resolved.import_errors.is_empty());
    }

    #[test]
    fn unreadable_import_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let prompt =
            write(dir.path(), "task.md", "[rules]: ./missing.md\n\n# Step 1\n[transition]: Exit\n");
        let wf = Workflow::compile(&std::fs::read_to_string(&prompt).unwrap()).unwrap();
        let resolved = resolve_sets(None, Path::new("/nonexistent"), &prompt, &wf);
        assert_eq!(resolved.import_errors.len(), 1);
        assert!(resolved.import_errors[0].contains("missing.md"));
    }

    #[test]
    fn extend_appends_across_layers() {
        let dir = tempfile::tempdir().unwrap();
        let rules = write(dir.path(), "guard-rules.md", "[net-x]: a:443\n");
        let prompt =
            write(dir.path(), "task.md", "[net-x]+: b:443\n\n# Step 1\n[transition]: Exit\n");
        let wf = Workflow::compile(&std::fs::read_to_string(&prompt).unwrap()).unwrap();
        let resolved = resolve_sets(None, &rules, &prompt, &wf);
        assert_eq!(resolved.sets["net-x"], vec!["a:443".to_string(), "b:443".to_string()]);
    }
}
