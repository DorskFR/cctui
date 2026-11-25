use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use cctui_crypto::redact::{self, CompiledPatterns, UserLiterals};
use serde_json::Value;

use super::Target;
use super::audit::{Audit, FileCtx};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    DryRun,
    Apply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub inode: u64,
}

impl Stamp {
    pub fn of(path: &Path) -> io::Result<Self> {
        let meta = fs::symlink_metadata(path)?;
        if !meta.is_file() {
            return Err(io::Error::other("not a regular file"));
        }
        Ok(Self { len: meta.len(), modified: meta.modified().ok(), inode: meta.ino() })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOutcome {
    pub stamp: Stamp,
    pub lines_changed: usize,
    pub lines_unfit: usize,
    pub stats: BTreeMap<String, usize>,
    pub written: bool,
    pub busy: bool,
}

enum Line {
    Clean,
    Fit(Vec<u8>),
    Unfit,
}

pub fn process_file(
    target: &Target,
    patterns: &CompiledPatterns,
    mode: Mode,
    mut audit: Option<&mut Audit>,
) -> io::Result<FileOutcome> {
    let stamp = Stamp::of(&target.path)?;
    let mut out = FileOutcome {
        stamp,
        lines_changed: 0,
        lines_unfit: 0,
        stats: BTreeMap::new(),
        written: false,
        busy: false,
    };
    let mut ctx = FileCtx::new(target);
    let read = for_each_line(&target.path, |raw| {
        match scrub_line(raw, patterns, &mut out.stats, &mut ctx, audit.as_deref_mut()) {
            Line::Clean => {}
            Line::Fit(_) => out.lines_changed += 1,
            Line::Unfit => out.lines_unfit += 1,
        }
        Ok(())
    })?;
    if read != stamp.len {
        out.busy = true;
        return Ok(out);
    }
    if mode == Mode::Apply && out.lines_changed > 0 {
        if rewrite(&target.path, patterns, stamp)? {
            out.written = true;
            out.stamp = Stamp::of(&target.path)?;
        } else {
            out.busy = true;
        }
    }
    Ok(out)
}

fn for_each_line(path: &Path, mut f: impl FnMut(&[u8]) -> io::Result<()>) -> io::Result<u64> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut buf = Vec::new();
    let mut total = 0u64;
    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Ok(total);
        }
        total += n as u64;
        f(&buf)?;
    }
}

fn scrub_line(
    raw: &[u8],
    patterns: &CompiledPatterns,
    stats: &mut BTreeMap<String, usize>,
    ctx: &mut FileCtx,
    audit: Option<&mut Audit>,
) -> Line {
    let Some(body) = raw.strip_suffix(b"\n") else { return Line::Clean };
    let Ok(body) = std::str::from_utf8(body) else { return Line::Clean };
    let json = body.trim_end_matches([' ', '\t', '\r']);
    let trailer = &body[json.len()..];
    let Ok(mut value) = serde_json::from_str::<Value>(json) else { return Line::Clean };
    ctx.observe(&value);
    let hits = scrub_value(&mut value, patterns, audit, ctx, json);
    if hits.is_empty() {
        return Line::Clean;
    }
    for (cat, n) in hits {
        *stats.entry(cat).or_insert(0) += n;
    }
    let Ok(scrubbed) = serde_json::to_string(&value) else { return Line::Unfit };
    let Some(pad) = json.len().checked_sub(scrubbed.len()) else { return Line::Unfit };
    let mut line = Vec::with_capacity(raw.len());
    line.extend_from_slice(scrubbed.as_bytes());
    line.resize(line.len() + pad, b' ');
    line.extend_from_slice(trailer.as_bytes());
    line.push(b'\n');
    Line::Fit(line)
}

/// The same passes the server's re-scrub runs over a stored payload.
fn scrub_value(
    value: &mut Value,
    patterns: &CompiledPatterns,
    mut audit: Option<&mut Audit>,
    ctx: &FileCtx,
    original: &str,
) -> BTreeMap<String, usize> {
    let user = redact::mask_user_json(value, &UserLiterals::new());
    if let Some(audit) = audit.as_deref_mut() {
        audit.sample(value, patterns);
    }
    let mut hits = redact::redact_json_stats(value, patterns);
    if user > 0 {
        *hits.entry(redact::USER_CATEGORY.to_owned()).or_insert(0) += user;
    }
    if let Some(audit) = audit
        && !hits.is_empty()
        && let Ok(before) = serde_json::from_str::<Value>(original)
    {
        audit.record(&before, value, ctx);
    }
    hits
}

fn tmp_path(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_string_lossy();
    Some(path.with_file_name(format!(".{name}.{}.cctui-scrub", std::process::id())))
}

/// `Ok(false)` when the original changed since `stamp` was taken.
pub(super) fn rewrite(path: &Path, patterns: &CompiledPatterns, stamp: Stamp) -> io::Result<bool> {
    let tmp = tmp_path(path).ok_or_else(|| io::Error::other("no file name"))?;
    let result = write_and_swap(path, &tmp, patterns, stamp);
    if !matches!(result, Ok(true)) {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn write_and_swap(
    path: &Path,
    tmp: &Path,
    patterns: &CompiledPatterns,
    stamp: Stamp,
) -> io::Result<bool> {
    let perms = fs::metadata(path)?.permissions();
    let file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(tmp)?;
    let mut writer = BufWriter::new(file);
    let mut stats = BTreeMap::new();
    let mut ctx = FileCtx::detached();
    let read =
        for_each_line(path, |raw| match scrub_line(raw, patterns, &mut stats, &mut ctx, None) {
            Line::Fit(line) => writer.write_all(&line),
            Line::Clean | Line::Unfit => writer.write_all(raw),
        })?;
    let file = writer.into_inner().map_err(io::IntoInnerError::into_error)?;
    if read != stamp.len || file.metadata()?.len() != stamp.len {
        return Ok(false);
    }
    file.set_permissions(perms)?;
    if let Some(modified) = stamp.modified {
        file.set_modified(modified)?;
    }
    file.sync_all()?;
    if Stamp::of(path)? != stamp {
        return Ok(false);
    }
    fs::rename(tmp, path)?;
    Ok(true)
}
