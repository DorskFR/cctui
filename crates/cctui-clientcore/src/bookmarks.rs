use crate::uri::encode_uri_component;

const TITLE_MAX: usize = 120;

/// Default bookmark title: the message's first non-empty line, trimmed.
#[must_use]
pub fn default_title(body: &str) -> String {
    let Some(line) = body.split('\n').map(str::trim).find(|l| !l.is_empty()) else {
        return String::new();
    };
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= TITLE_MAX {
        return line.to_string();
    }
    let head: String = chars[..TITLE_MAX - 1].iter().collect();
    format!("{}…", head.trim_end())
}

/// Target URL for "Open session". `seq` is the seam `ensureSeqVisible` reads to
/// scroll the drawer to the source message; without it the drawer simply opens.
#[must_use]
pub fn source_href(session_id: Option<&str>, seq: Option<i64>) -> Option<String> {
    let id = session_id?;
    let query = seq.map(|s| format!("?seq={s}")).unwrap_or_default();
    Some(format!("/sessions/{}{query}", encode_uri_component(id)))
}

#[must_use]
pub const fn is_dead_link(session_id: Option<&str>) -> bool {
    session_id.is_none()
}

/// The body a saved message snapshots, as the web UI's line model writes it:
/// plain text for a spoken line, a labelled fence for a tool call or result.
///
/// `mcp` only changes the label of a tool call, as it does in the web list.
#[must_use]
pub fn snapshot_body(
    role: &str,
    text: &str,
    tool: Option<&str>,
    mcp: bool,
    lang: Option<&str>,
) -> String {
    match role {
        "tool" => {
            let kind = if mcp { "MCP" } else { "Tool" };
            let label = tool.map(|t| format!("**{kind} · {t}**\n\n")).unwrap_or_default();
            format!("{label}```{}\n{text}\n```", lang.unwrap_or(""))
        }
        "result" => {
            let label = tool.map(|t| format!("**Result · {t}**\n\n")).unwrap_or_default();
            format!("{label}```\n{text}\n```")
        }
        _ => text.to_owned(),
    }
}

/// Markdown for the clipboard: the title as a heading, then note, then body.
#[must_use]
pub fn bookmark_markdown(title: &str, note: Option<&str>, body: &str) -> String {
    let mut parts = vec![format!("# {title}")];
    if let Some(n) = note.filter(|n| !n.is_empty()) {
        parts.push(format!("> {n}"));
    }
    parts.push(body.to_string());
    parts.join("\n\n")
}

fn push_trimmed(out: &mut Vec<String>, chars: &[char]) {
    let joined: String = chars.iter().collect();
    let trimmed = joined.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
}

/// Free-text terms of a query, for `highlight_terms`; quoted phrases stay whole.
#[must_use]
pub fn query_terms(q: &str) -> Vec<String> {
    let chars: Vec<char> = q.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        if chars[i] == '"'
            && let Some(end) = chars[i + 1..].iter().position(|c| *c == '"')
            && end > 0
        {
            push_trimmed(&mut out, &chars[i + 1..i + 1 + end]);
            i += end + 2;
            continue;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        push_trimmed(&mut out, &chars[start..i]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::snapshot_body;

    #[test]
    fn a_spoken_line_snapshots_as_its_own_text() {
        assert_eq!(snapshot_body("assistant", "the fix", None, false, None), "the fix");
        assert_eq!(snapshot_body("user", "do it", Some("Read"), false, None), "do it");
    }

    #[test]
    fn a_tool_call_snapshots_as_a_labelled_fence() {
        assert_eq!(
            snapshot_body("tool", "cargo test", Some("Bash"), false, Some("sh")),
            "**Tool · Bash**\n\n```sh\ncargo test\n```"
        );
        assert_eq!(
            snapshot_body("tool", "{}", Some("mcp__x__y"), true, None),
            "**MCP · mcp__x__y**\n\n```\n{}\n```"
        );
        assert_eq!(snapshot_body("tool", "bare", None, false, None), "```\nbare\n```");
    }

    #[test]
    fn a_result_snapshots_as_an_unlit_fence() {
        assert_eq!(
            snapshot_body("result", "ok", Some("Bash"), false, Some("sh")),
            "**Result · Bash**\n\n```\nok\n```"
        );
    }
}
