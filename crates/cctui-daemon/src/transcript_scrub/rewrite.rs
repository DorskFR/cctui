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
use super::liveness::alive;

const TMP_SUFFIX: &str = ".cctui-scrub";

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
    /// Hits on the lines that fit, the only ones a rewrite masks.
    pub stats: BTreeMap<String, usize>,
    pub values_unfit: usize,
    pub written: bool,
    pub busy: bool,
}

impl FileOutcome {
    #[must_use]
    pub fn values_found(&self) -> usize {
        self.stats.values().sum::<usize>() + self.values_unfit
    }
}

enum Line {
    Clean,
    Fit(Vec<u8>, BTreeMap<String, usize>),
    /// Left as is: it would grow, or a thinking block would not survive.
    Unfit(usize),
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
        values_unfit: 0,
        written: false,
        busy: false,
    };
    let mut ctx = FileCtx::new(target);
    let read = for_each_line(&target.path, |raw| {
        match scrub_line(raw, patterns, &mut ctx, audit.as_deref_mut()) {
            Line::Clean => {}
            Line::Fit(_, hits) => {
                out.lines_changed += 1;
                for (cat, n) in hits {
                    *out.stats.entry(cat).or_insert(0) += n;
                }
            }
            Line::Unfit(n) => {
                out.lines_unfit += 1;
                out.values_unfit += n;
            }
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
    ctx: &mut FileCtx,
    audit: Option<&mut Audit>,
) -> Line {
    let Some(body) = raw.strip_suffix(b"\n") else { return Line::Clean };
    let Ok(body) = std::str::from_utf8(body) else { return Line::Clean };
    let json = body.trim_end_matches([' ', '\t', '\r']);
    let trailer = &body[json.len()..];
    let Ok(mut value) = serde_json::from_str::<Value>(json) else { return Line::Clean };
    ctx.observe(&value);
    let Scrubbed { hits, thinking_intact } = scrub_value(&mut value, patterns, audit, ctx, json);
    if hits.is_empty() {
        return Line::Clean;
    }
    let found = hits.values().sum();
    if !thinking_intact {
        return Line::Unfit(found);
    }
    let Ok(scrubbed) = serde_json::to_string(&value) else { return Line::Unfit(found) };
    let Some(pad) = json.len().checked_sub(scrubbed.len()) else { return Line::Unfit(found) };
    let mut line = Vec::with_capacity(raw.len());
    line.extend_from_slice(scrubbed.as_bytes());
    line.resize(line.len() + pad, b' ');
    line.extend_from_slice(trailer.as_bytes());
    line.push(b'\n');
    Line::Fit(line, hits)
}

struct Scrubbed {
    hits: BTreeMap<String, usize>,
    /// Every thinking block re-serializes to the bytes it was read from.
    thinking_intact: bool,
}

/// The same passes the server's re-scrub runs over a stored payload, minus
/// thinking blocks: the harness replays them on resume and the provider
/// rejects one whose signature or data changed.
fn scrub_value(
    value: &mut Value,
    patterns: &CompiledPatterns,
    mut audit: Option<&mut Audit>,
    ctx: &FileCtx,
    original: &str,
) -> Scrubbed {
    let held = Held::detach(value);
    let user = redact::mask_user_json(value, &UserLiterals::new());
    if let Some(audit) = audit.as_deref_mut() {
        audit.sample(value, patterns);
    }
    let mut hits = redact::redact_json_stats(value, patterns);
    if user > 0 {
        *hits.entry(redact::USER_CATEGORY.to_owned()).or_insert(0) += user;
    }
    let thinking_intact = held.restore(value, original);
    if let Some(audit) = audit
        && !hits.is_empty()
        && let Ok(before) = serde_json::from_str::<Value>(original)
    {
        audit.record(&before, value, ctx);
    }
    Scrubbed { hits, thinking_intact }
}

#[derive(Clone)]
enum Step {
    Key(String),
    Index(usize),
}

/// Thinking blocks lifted out of a line while the rest is redacted.
struct Held(Vec<(Vec<Step>, Value)>);

impl Held {
    fn detach(value: &mut Value) -> Self {
        let mut held = Vec::new();
        take_thinking(value, &mut Vec::new(), &mut held);
        Self(held)
    }

    fn restore(self, value: &mut Value, original: &str) -> bool {
        let mut intact = true;
        for (path, block) in self.0 {
            let slot = path.iter().try_fold(&mut *value, |cur, step| match step {
                Step::Key(k) => cur.get_mut(k.as_str()),
                Step::Index(i) => cur.get_mut(*i),
            });
            intact &= serde_json::to_string(&block).is_ok_and(|s| original.contains(&s));
            match slot {
                Some(slot) => *slot = block,
                None => intact = false,
            }
        }
        intact
    }
}

fn is_thinking(value: &Value) -> bool {
    value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| t == "thinking" || t == "redacted_thinking")
}

fn take_thinking(value: &mut Value, path: &mut Vec<Step>, out: &mut Vec<(Vec<Step>, Value)>) {
    if is_thinking(value) {
        out.push((path.clone(), value.take()));
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                path.push(Step::Key(key.clone()));
                take_thinking(child, path, out);
                path.pop();
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter_mut().enumerate() {
                path.push(Step::Index(i));
                take_thinking(child, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

fn tmp_path(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_string_lossy();
    Some(path.with_file_name(format!(".{name}.{}{TMP_SUFFIX}", std::process::id())))
}

/// The writer's pid, for a name `tmp_path` produces.
pub(super) fn tmp_owner(name: &str) -> Option<u32> {
    let inner = name.strip_prefix('.')?.strip_suffix(TMP_SUFFIX)?;
    let (original, pid) = inner.rsplit_once('.')?;
    let jsonl = Path::new(original).extension().is_some_and(|e| e == "jsonl");
    if !jsonl || pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    pid.parse().ok()
}

/// Remove the tmp copies a killed rewrite left behind: regular files named
/// like `tmp_path`'s whose writer is this process or gone, under `dir`.
pub(super) fn clear_stale_tmp(dir: &Path) -> usize {
    clear_in(dir, std::process::id())
}

fn clear_in(dir: &Path, own: u32) -> usize {
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if kind.is_dir() {
            removed += clear_in(&path, own);
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let Some(pid) = entry.file_name().to_str().and_then(tmp_owner) else { continue };
        if (pid == own || !alive(i64::from(pid))) && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// `Ok(false)` when the original changed since `stamp` was taken.
pub(super) fn rewrite(path: &Path, patterns: &CompiledPatterns, stamp: Stamp) -> io::Result<bool> {
    let tmp = tmp_path(path).ok_or_else(|| io::Error::other("no file name"))?;
    if fs::symlink_metadata(&tmp).is_ok_and(|m| m.is_file()) {
        fs::remove_file(&tmp)?;
    }
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
    let mut ctx = FileCtx::detached();
    let read = for_each_line(path, |raw| match scrub_line(raw, patterns, &mut ctx, None) {
        Line::Fit(line, _) => writer.write_all(&line),
        Line::Clean | Line::Unfit(_) => writer.write_all(raw),
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
