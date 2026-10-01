pub use cctui_proto::uploads::{MAX_FILE_BYTES, MAX_FILES, MAX_TOTAL_BYTES, UploadCaps};

#[must_use]
pub fn default_caps() -> UploadCaps {
    UploadCaps::default()
}

#[must_use]
pub const fn over_file_cap(size: u64, caps: UploadCaps) -> bool {
    size > caps.max_file_bytes
}

#[must_use]
pub fn over_total_cap(sizes: &[u64], caps: UploadCaps) -> bool {
    sizes.iter().sum::<u64>() > caps.max_total_bytes
}

#[must_use]
pub const fn over_count_cap(count: usize, caps: UploadCaps) -> bool {
    count > caps.max_files as usize
}

/// Split `name` into stem and extension (`a.tar.gz` -> `a.tar` + `.gz`).
#[must_use]
pub fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// A name for `incoming` that no entry of `taken` uses: a clash becomes
/// `stem-2.ext`, `stem-3.ext`, … Never replaces, so the staged list always
/// matches what the daemon wrote.
#[must_use]
pub fn unique_name(taken: &[String], incoming: &str) -> String {
    if !taken.iter().any(|t| t == incoming) {
        return incoming.to_owned();
    }
    let (stem, ext) = split_ext(incoming);
    let mut n = 2_u32;
    loop {
        let candidate = format!("{stem}-{n}{ext}");
        if !taken.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Merge `incoming` into `current` keeping names unique; returns the names as
/// actually added, for tokenizing.
pub fn merge_renamed(current: &mut Vec<String>, incoming: &[String]) -> Vec<String> {
    let mut added = Vec::with_capacity(incoming.len());
    for name in incoming {
        let unique = unique_name(current, name);
        current.push(unique.clone());
        added.push(unique);
    }
    added
}

/// Next free `paste-N.txt` index: one past the highest N seen in `names`, the
/// `[paste-N.txt]` tokens of `text` and the names already staged in `used`.
///
/// Derived rather than counted so it survives a restart whose draft still
/// references earlier pastes; without `used` a fresh draft would restart at
/// `paste-1.txt` and collide with an earlier message's upload.
#[must_use]
pub fn next_paste_index<'a>(
    names: impl IntoIterator<Item = &'a str>,
    text: &str,
    used: impl IntoIterator<Item = &'a str>,
) -> u32 {
    let mut max = 0;
    let mut scan = |s: &str| {
        for found in paste_indices(s) {
            max = max.max(found);
        }
    };
    for name in names {
        scan(name);
    }
    for name in used {
        scan(name);
    }
    scan(text);
    max + 1
}

/// Every N in a `paste-N.txt` occurrence, at a word boundary like the TS
/// `\bpaste-(\d+)\.txt\b`.
fn paste_indices(s: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    for (at, _) in s.match_indices("paste-") {
        let boundary =
            at == 0 || !matches!(bytes[at - 1], b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_');
        if !boundary {
            continue;
        }
        let digits_at = at + "paste-".len();
        let end = digits_at + bytes[digits_at..].iter().take_while(|b| b.is_ascii_digit()).count();
        if end == digits_at || !s[end..].starts_with(".txt") {
            continue;
        }
        let after = end + ".txt".len();
        let closed = bytes
            .get(after)
            .is_none_or(|b| !matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_'));
        if closed && let Ok(n) = s[digits_at..end].parse() {
            out.push(n);
        }
    }
    out
}

/// Append a `[name]` reference per attachment, skipping names the text already
/// carries so a re-pick does not duplicate.
#[must_use]
pub fn append_file_tokens(text: &str, names: &[String]) -> String {
    let mut out = text.to_owned();
    for name in names {
        let token = format!("[{name}]");
        if out.contains(&token) {
            continue;
        }
        if !out.is_empty() && !out.ends_with(char::is_whitespace) {
            out.push(' ');
        }
        out.push_str(&token);
    }
    out
}

/// Point each `[name]` token at the name staging actually gave the file.
///
/// A server-side clash is renamed (`paste-1.txt` -> `paste-1-1.txt`), and a
/// token left on the old name would resolve to some other message's upload.
#[must_use]
pub fn rewrite_file_tokens(text: &str, names: &[String], paths: &[String]) -> String {
    let mut out = text.to_owned();
    for (i, name) in names.iter().enumerate() {
        let Some(staged) = paths.get(i).map(|p| basename(p)) else { continue };
        if staged == name {
            continue;
        }
        out = out.replace(&format!("[{name}]"), &format!("[{staged}]"));
    }
    out
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Human-readable byte size, worded as the webui words it.
#[must_use]
pub fn fmt_size(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    #[allow(clippy::cast_precision_loss)]
    let kb = n as f64 / 1024.0;
    if n < 1024 * 1024 {
        return format!("{} KB", crate::format::js_to_fixed(kb, 0));
    }
    format!("{} MB", crate::format::js_to_fixed(kb / 1024.0, 1))
}

/// The cap a list of sizes breaks, worded exactly as `fileCapError` words it.
/// `None` when the list is within every cap.
#[must_use]
pub fn cap_error(sizes: &[u64], caps: UploadCaps) -> Option<String> {
    if sizes.iter().any(|s| over_file_cap(*s, caps)) {
        return Some(format!("A file exceeds the {} per-file cap", fmt_size(caps.max_file_bytes)));
    }
    if over_count_cap(sizes.len(), caps) {
        return Some(format!("Too many files (max {})", caps.max_files));
    }
    if over_total_cap(sizes, caps) {
        return Some(format!(
            "Attachments exceed the {} total cap",
            fmt_size(caps.max_total_bytes)
        ));
    }
    None
}

/// Extension for a clipboard MIME type; the webui's `MIME_EXT` table, then the
/// sanitised subtype, then `bin`.
#[must_use]
pub fn ext_for_type(mime: &str) -> String {
    let known = match mime {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/bmp" => Some("bmp"),
        "image/svg+xml" => Some("svg"),
        "image/tiff" => Some("tiff"),
        "application/pdf" => Some("pdf"),
        _ => None,
    };
    if let Some(ext) = known {
        return ext.to_owned();
    }
    let sub: String = mime
        .split('/')
        .nth(1)
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    if sub.is_empty() { "bin".to_owned() } else { sub }
}

#[cfg(test)]
mod tests {
    use super::{
        UploadCaps, append_file_tokens, cap_error, default_caps, ext_for_type, fmt_size,
        merge_renamed, next_paste_index, rewrite_file_tokens, split_ext, unique_name,
    };

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn an_extension_splits_off_the_last_dot_only() {
        assert_eq!(split_ext("a.tar.gz"), ("a.tar", ".gz"));
        assert_eq!(split_ext("plain"), ("plain", ""));
        assert_eq!(split_ext(".hidden"), (".hidden", ""));
    }

    #[test]
    fn a_clashing_name_is_renamed_never_replaced() {
        let taken = names(&["a.txt", "a-2.txt"]);
        assert_eq!(unique_name(&taken, "b.txt"), "b.txt");
        assert_eq!(unique_name(&taken, "a.txt"), "a-3.txt");
    }

    #[test]
    fn merging_reports_the_names_it_actually_added() {
        let mut current = names(&["shot.png"]);
        let added = merge_renamed(&mut current, &names(&["shot.png", "doc.pdf"]));
        assert_eq!(added, names(&["shot-2.png", "doc.pdf"]));
        assert_eq!(current, names(&["shot.png", "shot-2.png", "doc.pdf"]));
    }

    #[test]
    fn the_next_paste_index_is_one_past_every_source() {
        assert_eq!(next_paste_index([], "", []), 1);
        assert_eq!(next_paste_index(["paste-3.txt"], "", []), 4);
        assert_eq!(next_paste_index([], "see [paste-7.txt] please", []), 8);
        assert_eq!(next_paste_index([], "", ["paste-9.txt"]), 10);
        assert_eq!(next_paste_index(["paste-2.txt"], "[paste-5.txt]", ["paste-4.txt"]), 6);
    }

    #[test]
    fn a_paste_index_needs_real_word_boundaries() {
        assert_eq!(next_paste_index(["mypaste-9.txt"], "", []), 1, "no left boundary");
        assert_eq!(next_paste_index(["paste-9.txtx"], "", []), 1, "no right boundary");
        assert_eq!(next_paste_index(["paste-.txt"], "", []), 1, "no digits");
        assert_eq!(next_paste_index(["paste-9.md"], "", []), 1, "wrong extension");
    }

    #[test]
    fn tokens_are_appended_once_and_spaced() {
        assert_eq!(append_file_tokens("", &names(&["a.txt"])), "[a.txt]");
        assert_eq!(append_file_tokens("look", &names(&["a.txt"])), "look [a.txt]");
        assert_eq!(append_file_tokens("look ", &names(&["a.txt"])), "look [a.txt]");
        assert_eq!(
            append_file_tokens("has [a.txt]", &names(&["a.txt", "b.txt"])),
            "has [a.txt] [b.txt]"
        );
    }

    #[test]
    fn tokens_follow_the_name_staging_gave_the_file() {
        let text = "see [paste-1.txt] and [keep.md]";
        let staged = names(&["/w/paste-1-1.txt", "/w/keep.md"]);
        assert_eq!(
            rewrite_file_tokens(text, &names(&["paste-1.txt", "keep.md"]), &staged),
            "see [paste-1-1.txt] and [keep.md]"
        );
    }

    #[test]
    fn a_missing_staged_path_leaves_its_token_alone() {
        let text = "[a.txt] [b.txt]";
        assert_eq!(
            rewrite_file_tokens(text, &names(&["a.txt", "b.txt"]), &names(&["/w/a-2.txt"])),
            "[a-2.txt] [b.txt]"
        );
    }

    #[test]
    fn sizes_are_worded_like_the_web_ui() {
        assert_eq!(fmt_size(0), "0 B");
        assert_eq!(fmt_size(1023), "1023 B");
        assert_eq!(fmt_size(1024), "1 KB");
        assert_eq!(fmt_size(12 * 1024), "12 KB");
        assert_eq!(fmt_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(fmt_size(1_258_291), "1.2 MB");
    }

    #[test]
    fn each_cap_has_its_own_message_in_the_web_ui_order() {
        let caps = default_caps();
        assert_eq!(cap_error(&[1, 2, 3], caps), None);
        assert_eq!(
            cap_error(&[caps.max_file_bytes + 1], caps).as_deref(),
            Some("A file exceeds the 5.0 MB per-file cap")
        );
        let many = vec![1_u64; caps.max_files as usize + 1];
        assert_eq!(cap_error(&many, caps).as_deref(), Some("Too many files (max 10)"));
        let big = vec![caps.max_file_bytes; 5];
        assert_eq!(
            cap_error(&big, caps).as_deref(),
            Some("Attachments exceed the 20.0 MB total cap"),
            "the per-file cap passes, so the total is what breaks"
        );
    }

    #[test]
    fn a_tiny_cap_set_still_reports_the_file_cap_first() {
        let caps = UploadCaps { max_files: 1, max_file_bytes: 10, max_total_bytes: 10 };
        assert_eq!(
            cap_error(&[11, 11], caps).as_deref(),
            Some("A file exceeds the 10 B per-file cap")
        );
    }

    #[test]
    fn clipboard_extensions_map_then_fall_back() {
        assert_eq!(ext_for_type("image/png"), "png");
        assert_eq!(ext_for_type("image/jpeg"), "jpg");
        assert_eq!(ext_for_type("text/csv"), "csv");
        assert_eq!(ext_for_type("text/plain;charset=utf-8"), "plain");
        assert_eq!(ext_for_type("nonsense"), "bin");
        assert_eq!(ext_for_type("x/!!!"), "bin");
    }
}
