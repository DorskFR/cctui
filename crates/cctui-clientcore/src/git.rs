use cctui_proto::git::GitInfo;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GitBadge {
    pub text: String,
    pub worktree: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
}

/// `main`, `detached @abc1234`, or `None` when not a repo / nothing readable.
#[must_use]
pub fn git_badge(info: Option<&GitInfo>) -> Option<GitBadge> {
    let info = info.filter(|i| i.is_repo)?;
    let worktree = info.is_worktree;
    if let Some(branch) = info.branch.as_ref() {
        return Some(GitBadge { text: branch.clone(), worktree, sha: None });
    }
    let sha = info.detached_sha.as_ref()?;
    let short: String = sha.chars().take(7).collect();
    Some(GitBadge { text: format!("detached @{short}"), worktree, sha: Some(short) })
}
