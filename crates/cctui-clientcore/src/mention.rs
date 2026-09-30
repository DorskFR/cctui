#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MentionTrigger {
    pub start: usize,
    pub query: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MentionSession {
    pub id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub machine_name: Option<String>,
}

/// Find an active `#query` immediately before the caret (a char offset).
///
/// `None` when the caret is not inside one: no `#`, whitespace between `#` and
/// the caret, or the `#` glued to a preceding word character (`C#`, a URL
/// fragment).
#[must_use]
pub fn find_trigger(text: &str, caret: usize) -> Option<MentionTrigger> {
    let chars: Vec<char> = text.chars().collect();
    let caret = caret.min(chars.len());
    let before = &chars[..caret];
    let hash = before.iter().rposition(|c| *c == '#')?;
    let query: String = before[hash + 1..].iter().collect();
    if query.chars().any(char::is_whitespace) {
        return None;
    }
    if hash > 0 {
        let prev = before[hash - 1];
        if prev == '#' || prev == '_' || prev.is_alphanumeric() {
            return None;
        }
    }
    Some(MentionTrigger { start: hash, query })
}

/// Every session an agent could still be pointed at: any bucket, completed ones
/// included, but never archived or draft.
#[must_use]
pub fn mentionable_sessions(
    sessions: &[MentionSession],
    exclude_id: Option<&str>,
) -> Vec<MentionSession> {
    sessions
        .iter()
        .filter(|s| Some(s.id.as_str()) != exclude_id)
        .filter(|s| s.status != "archived" && s.status != "draft")
        .cloned()
        .collect()
}

/// Case-insensitive match on name, id, working dir and machine name.
#[must_use]
pub fn filter_mentions(sessions: &[MentionSession], query: &str) -> Vec<MentionSession> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return sessions.to_vec();
    }
    sessions
        .iter()
        .filter(|s| {
            [
                s.name.as_deref(),
                Some(s.id.as_str()),
                s.working_dir.as_deref(),
                s.machine_name.as_deref(),
            ]
            .into_iter()
            .flatten()
            .any(|f| f.to_lowercase().contains(&q))
        })
        .cloned()
        .collect()
}

#[must_use]
pub fn mention_token(id: &str, name: Option<&str>) -> String {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => format!("#{id} ({n})"),
        None => format!("#{id}"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MentionInsertion {
    pub text: String,
    pub caret: usize,
}

/// Replace the `#query` under the caret with the mention token plus a trailing
/// space.
#[must_use]
pub fn apply_mention(
    text: &str,
    caret: usize,
    trigger: &MentionTrigger,
    id: &str,
    name: Option<&str>,
) -> MentionInsertion {
    let chars: Vec<char> = text.chars().collect();
    let caret = caret.min(chars.len());
    let token = format!("{} ", mention_token(id, name));
    let head: String = chars[..trigger.start.min(chars.len())].iter().collect();
    let tail: String = chars[caret..].iter().collect();
    MentionInsertion {
        text: format!("{head}{token}{tail}"),
        caret: trigger.start + token.chars().count(),
    }
}

/// Wrap-around move of the highlighted row.
#[must_use]
pub const fn move_selection(index: usize, delta: i32, length: usize) -> usize {
    if length == 0 {
        return 0;
    }
    let step = if delta < 0 { length - 1 } else { 1 };
    (index + step) % length
}
