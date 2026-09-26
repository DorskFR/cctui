//! Plugin archives: a gzip'd tar an admin uploads or the server fetches,
//! extracted into memory under strict limits and validated like a plugin folder.

use std::collections::BTreeMap;
use std::io::Read;

use flate2::read::GzDecoder;

use crate::plugins::{Plugin, load_from_memory, safe_relative};

pub const MAX_ARCHIVE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_EXTRACTED_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 500;
/// Tar headers and block padding on top of the file bytes budget.
const TAR_OVERHEAD_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ArchiveError {
    #[error("archive larger than {} MB", MAX_ARCHIVE_BYTES / 1024 / 1024)]
    TooLarge,
    #[error("archive extracts to more than {} MB", MAX_EXTRACTED_BYTES / 1024 / 1024)]
    ExtractedTooLarge,
    #[error("archive has more than {MAX_ENTRIES} entries")]
    TooManyEntries,
    #[error("archive is not a gzip'd tar: {0}")]
    Malformed(String),
    #[error("archive entry `{0}` is not a regular file")]
    NotRegular(String),
    #[error("archive entry `{0}` must be a plain relative path")]
    BadPath(String),
    #[error("archive has no plugin.json at its root or in a single top-level folder")]
    NoManifest,
    #[error("{0}")]
    Invalid(String),
}

/// Files by relative path (top-level folder stripped) and that folder's name.
#[derive(Debug)]
pub struct Extracted {
    pub files: BTreeMap<String, Vec<u8>>,
    pub folder: Option<String>,
}

fn normalize_entry(raw: &str) -> Option<String> {
    let path = raw.strip_prefix("./").unwrap_or(raw).trim_end_matches('/');
    safe_relative(path).then(|| path.to_owned())
}

/// Unpack `bytes` under the size/entry limits; every entry must be a regular
/// file (directories are skipped) at a plain relative path.
pub fn extract(bytes: &[u8]) -> Result<Extracted, ArchiveError> {
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(ArchiveError::TooLarge);
    }
    let budget = (MAX_EXTRACTED_BYTES + TAR_OVERHEAD_BYTES) as u64;
    let mut archive = tar::Archive::new(GzDecoder::new(bytes).take(budget));
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    let mut entries = 0usize;
    let iter = archive.entries().map_err(|e| ArchiveError::Malformed(e.to_string()))?;
    for entry in iter {
        let mut entry = entry.map_err(|e| ArchiveError::Malformed(e.to_string()))?;
        entries += 1;
        if entries > MAX_ENTRIES {
            return Err(ArchiveError::TooManyEntries);
        }
        let raw = entry
            .path()
            .ok()
            .and_then(|p| p.to_str().map(str::to_owned))
            .ok_or_else(|| ArchiveError::BadPath(String::new()))?;
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() {
            return Err(ArchiveError::NotRegular(raw));
        }
        let path = normalize_entry(&raw).ok_or_else(|| ArchiveError::BadPath(raw.clone()))?;
        let size = usize::try_from(entry.size()).map_err(|_| ArchiveError::ExtractedTooLarge)?;
        total = total.checked_add(size).ok_or(ArchiveError::ExtractedTooLarge)?;
        if total > MAX_EXTRACTED_BYTES {
            return Err(ArchiveError::ExtractedTooLarge);
        }
        let mut data = Vec::with_capacity(size);
        entry.read_to_end(&mut data).map_err(|e| ArchiveError::Malformed(e.to_string()))?;
        if data.len() != size {
            return Err(ArchiveError::Malformed(format!("`{path}` is truncated")));
        }
        files.insert(path, data);
    }
    if files.contains_key("plugin.json") {
        return Ok(Extracted { files, folder: None });
    }
    let top = files.keys().next().and_then(|k| k.split('/').next()).map(str::to_owned);
    let Some(top) = top else { return Err(ArchiveError::NoManifest) };
    let prefix = format!("{top}/");
    if !files.contains_key(&format!("{prefix}plugin.json"))
        || !files.keys().all(|k| k.starts_with(&prefix))
    {
        return Err(ArchiveError::NoManifest);
    }
    let files = files
        .into_iter()
        .filter_map(|(k, v)| k.strip_prefix(&prefix).map(|rest| (rest.to_owned(), v)))
        .collect();
    Ok(Extracted { files, folder: Some(top) })
}

/// Extract and validate an archive into a plugin ready to register.
pub fn load_archive(bytes: &[u8], enabled: bool) -> Result<Plugin, ArchiveError> {
    let Extracted { files, folder } = extract(bytes)?;
    load_from_memory(files, folder.as_deref(), enabled)
        .map_err(|e| ArchiveError::Invalid(format!("{e:#}")))
}

#[cfg(test)]
pub mod test_support {
    use flate2::Compression;
    use flate2::write::GzEncoder;

    /// A gzip'd tar of `(path, bytes)` regular files; the name is written raw
    /// so tests can smuggle paths the builder would refuse.
    pub fn tgz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for (path, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.as_old_mut().name[..path.len()].copy_from_slice(path.as_bytes());
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    pub const MANIFEST: &[u8] = br#"{"id":"demo","name":"Demo","version":"1.0.0","cctuiApi":1,"web":"web/index.js","skills":["demo"]}"#;

    /// A valid plugin archive, optionally wrapped in a top-level folder.
    pub fn demo_tgz(folder: Option<&str>, version: &str) -> Vec<u8> {
        let manifest = String::from_utf8_lossy(MANIFEST).replace("1.0.0", version);
        let p = |rel: &str| folder.map_or_else(|| rel.to_owned(), |f| format!("{f}/{rel}"));
        tgz(&[
            (&p("plugin.json"), manifest.as_bytes()),
            (&p("web/index.js"), b"export default { cctuiApi: 1 };"),
            (&p("skills/demo/SKILL.md"), b"# demo"),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{MANIFEST, demo_tgz, tgz};
    use super::{ArchiveError, MAX_ARCHIVE_BYTES, MAX_ENTRIES, extract, load_archive};
    use crate::plugins::{PluginSource, resolve_static};
    use flate2::Compression;
    use flate2::write::GzEncoder;

    #[test]
    fn loads_root_and_single_folder_layouts() {
        for folder in [None, Some("demo")] {
            let plugin = load_archive(&demo_tgz(folder, "1.0.0"), false).unwrap();
            assert_eq!(plugin.manifest.id, "demo");
            assert_eq!(plugin.source, PluginSource::Installed);
            assert!(!plugin.instance_enabled);
            assert_eq!(plugin.skill_files, vec!["demo/SKILL.md"]);
            assert!(plugin.web_hash.is_some());
            assert_eq!(
                resolve_static(&plugin, "web/index.js").unwrap(),
                b"export default { cctuiApi: 1 };"
            );
            assert!(resolve_static(&plugin, "../plugin.json").is_none());
            assert!(resolve_static(&plugin, "web/nope.js").is_none());
        }
    }

    #[test]
    fn folder_must_match_the_manifest_id() {
        let err = load_archive(&demo_tgz(Some("other"), "1.0.0"), false).unwrap_err();
        assert!(
            matches!(err, ArchiveError::Invalid(ref m) if m.contains("does not match its folder")),
            "{err}"
        );
    }

    #[test]
    fn rejects_traversal_absolute_and_symlink_entries() {
        for bad in ["../evil.js", "web/../../evil.js", ".hidden/x"] {
            let err = extract(&tgz(&[("plugin.json", MANIFEST), (bad, b"x")])).unwrap_err();
            assert!(matches!(err, ArchiveError::BadPath(_)), "{bad}: {err}");
        }
        let err = extract(&tgz(&[("plugin.json", MANIFEST), ("/etc/passwd", b"x")])).unwrap_err();
        assert!(matches!(err, ArchiveError::BadPath(_) | ArchiveError::Malformed(_)), "{err}");

        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_cksum();
        builder.append_link(&mut header, "web/index.js", "/etc/passwd").unwrap();
        let bytes = builder.into_inner().unwrap().finish().unwrap();
        assert_eq!(extract(&bytes).unwrap_err(), ArchiveError::NotRegular("web/index.js".into()));
    }

    #[test]
    fn enforces_size_and_entry_limits() {
        assert_eq!(extract(&vec![0; MAX_ARCHIVE_BYTES + 1]).unwrap_err(), ArchiveError::TooLarge);
        let bomb = tgz(&[("plugin.json", MANIFEST), ("web/big.bin", &vec![0u8; 21 * 1024 * 1024])]);
        assert!(bomb.len() < 100 * 1024, "zeros should compress well");
        assert_eq!(extract(&bomb).unwrap_err(), ArchiveError::ExtractedTooLarge);
        let names: Vec<String> = (0..=MAX_ENTRIES).map(|i| format!("web/f{i}")).collect();
        let entries: Vec<(&str, &[u8])> =
            names.iter().map(|n| (n.as_str(), b"x" as &[u8])).collect();
        assert_eq!(extract(&tgz(&entries)).unwrap_err(), ArchiveError::TooManyEntries);
    }

    #[test]
    fn rejects_missing_or_bad_manifest() {
        assert_eq!(extract(&tgz(&[("web/index.js", b"x")])).unwrap_err(), ArchiveError::NoManifest);
        assert_eq!(
            extract(&tgz(&[("a/plugin.json", MANIFEST), ("b/x", b"x")])).unwrap_err(),
            ArchiveError::NoManifest
        );
        assert!(matches!(extract(b"not an archive").unwrap_err(), ArchiveError::Malformed(_)));
        let err = load_archive(&tgz(&[("plugin.json", b"{not json")]), false).unwrap_err();
        assert!(matches!(err, ArchiveError::Invalid(_)), "{err}");
        let no_web = tgz(&[("plugin.json", MANIFEST)]);
        let err = load_archive(&no_web, false).unwrap_err();
        assert!(
            matches!(err, ArchiveError::Invalid(ref m) if m.contains("does not exist")),
            "{err}"
        );
        let api2 = String::from_utf8_lossy(MANIFEST).replace("\"cctuiApi\":1", "\"cctuiApi\":2");
        let err = load_archive(&tgz(&[("plugin.json", api2.as_bytes())]), false).unwrap_err();
        assert!(matches!(err, ArchiveError::Invalid(ref m) if m.contains("cctuiApi")), "{err}");
    }
}
