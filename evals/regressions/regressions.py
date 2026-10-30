#!/usr/bin/env python3
"""Fail and regression rates of the changes on a branch, grouped by the agent that wrote them.

A change is one first-parent commit of the branch: a squashed PR, a merged PR
(with every commit it brought in) or a direct push. Each change is attributed to
the model named most often in its Co-Authored-By trailers.

A change counts as
- reverted when a later commit says "This reverts commit <sha>" of one of its commits;
- regressed when a later fix change, within the window, modifies or deletes
  lines that `git blame` attributes to it.

Changes younger than the window are left out of the rates: their fixes may not
have landed yet.
"""

import argparse
import fnmatch
import json
import re
import statistics
import subprocess
import sys
from collections import Counter, defaultdict
from dataclasses import dataclass, field

DAY = 86400
DEFAULT_EXCLUDES = ["*.lock", "*-lock.json", "*-lock.yaml", "CHANGELOG*", "*/CHANGELOG*", "*/schema/*.json"]
VERSION_LINE = re.compile(r'^\s*"?version"?\s*[=:]')
CONVENTIONAL = re.compile(r"^(\w+)(\([^)]*\))?!?:")
CLAUDE_TRAILER = re.compile(r"^Claude\s*(.*?)\s*(\(1M context\))?\s*<", re.IGNORECASE)
REVERTS = re.compile(r"This reverts commit ([0-9a-f]{7,40})")
PR_NUMBER = re.compile(r"\(#(\d+)\)\s*$|^Merge pull request #(\d+)")
REVERTED_TITLE = re.compile(r'^Revert "(.*)"(?: \(#\d+\))?$')
FIX_SUBJECT = re.compile(r"^(fix|hotfix)\b", re.IGNORECASE)


@dataclass(eq=False)
class Change:
    sha: str
    time: int
    subject: str
    kind: str
    agent: str
    pr: int | None
    members: list[str]
    added: int = 0
    reverted_by: list[str] = field(default_factory=list)
    fixed_by: dict[str, int] = field(default_factory=dict)
    first_fix_days: float | None = None


def git(repo: str, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", repo, *args], check=True, capture_output=True, text=True, errors="replace"
    ).stdout


def classify(subject: str) -> str:
    if subject.startswith('Revert "'):
        return "revert"
    if m := CONVENTIONAL.match(subject):
        return m.group(1).lower()
    if FIX_SUBJECT.match(subject):
        return "fix"
    if re.match(r"^release\b", subject, re.IGNORECASE):
        return "release"
    return "other"


def agent_of(trailers: list[str], authors: list[str]) -> str:
    models = Counter()
    for t in trailers:
        if m := CLAUDE_TRAILER.match(t.strip()):
            models[model_name(m.group(1))] += 1
    if models:
        return models.most_common(1)[0][0]
    if any("dependabot" in a for a in authors):
        return "dependabot"
    return "human"


def model_name(raw: str) -> str:
    words = re.sub(r"^claude-", "", raw, flags=re.IGNORECASE).replace("-", " ").split()
    return " ".join(w.capitalize() for w in words) or "Claude (unnamed)"


def excluded(path: str, excludes: list[str]) -> bool:
    return any(fnmatch.fnmatch(path, g) for g in excludes)


def load_changes(repo: str, ref: str) -> tuple[list[Change], dict[str, Change]]:
    meta = {}
    log = git(repo, "log", "--format=%H%x00%an%x00%(trailers:key=Co-Authored-By,valueonly,separator=%x1f)%x1e", ref)
    for rec in log.split("\x1e"):
        if not rec.strip():
            continue
        sha, author, trailers = rec.strip("\n").split("\x00")
        meta[sha] = (author, [t for t in trailers.split("\x1f") if t])

    changes, owner = [], {}
    fp = git(repo, "log", "--first-parent", "--reverse", "--format=%H%x00%P%x00%ct%x00%s%x00%b%x1e", ref)
    for rec in fp.split("\x1e"):
        if not rec.strip():
            continue
        sha, parents, ct, subject, body = rec.strip("\n").split("\x00", 4)
        parents = parents.split()
        members = git(repo, "rev-list", f"{parents[0]}..{sha}").split() if len(parents) > 1 else [sha]
        title = subject
        if subject.startswith("Merge pull request") and body.strip():
            title = body.strip().splitlines()[0]
        pr = None
        if m := PR_NUMBER.search(subject):
            pr = int(m.group(1) or m.group(2))
        trailers = [t for s in members for t in meta.get(s, ("", []))[1]]
        authors = [meta.get(s, ("", []))[0] for s in members]
        change = Change(sha, int(ct), title, classify(title), agent_of(trailers, authors), pr, members)
        changes.append(change)
        for s in members:
            owner.setdefault(s, change)
    return changes, owner


def diff_stats(repo: str, change: Change, excludes: list[str]) -> list[tuple[str, list[tuple[int, int]]]]:
    """Sets change.added and returns the old-side line ranges the change modified or deleted."""
    parent = f"{change.sha}^1"
    try:
        numstat = git(repo, "diff", "--numstat", "--no-renames", parent, change.sha)
    except subprocess.CalledProcessError:
        return []
    for line in numstat.splitlines():
        added, _, path = line.split("\t", 2)
        if added != "-" and not excluded(path, excludes):
            change.added += int(added)
    if change.kind != "fix":
        return []
    ranges, path = defaultdict(list), None
    for line in git(repo, "diff", "-U0", "--no-renames", parent, change.sha).splitlines():
        if line.startswith("--- "):
            path = None if line == "--- /dev/null" else line[6:]
        elif line.startswith("@@") and path and not excluded(path, excludes):
            m = re.match(r"@@ -(\d+)(?:,(\d+))? ", line)
            start, count = int(m.group(1)), int(m.group(2) or 1)
            if count:
                ranges[path].append((start, count))
    return list(ranges.items())


def blame(repo: str, rev: str, path: str, ranges: list[tuple[int, int]]) -> Counter:
    args = ["blame", "-w", "--porcelain"]
    for start, count in ranges:
        args += ["-L", f"{start},+{count}"]
    try:
        out = git(repo, *args, rev, "--", path)
    except subprocess.CalledProcessError:
        return Counter()
    origins, current = Counter(), None
    for line in out.splitlines():
        if line.startswith("\t"):
            if current and not VERSION_LINE.match(line[1:]):
                origins[current] += 1
        elif re.match(r"^[0-9a-f]{40} ", line):
            current = line[:40]
    return origins


def analyse(repo: str, ref: str, window_days: int, excludes: list[str]) -> list[Change]:
    changes, owner = load_changes(repo, ref)

    def resolve(sha: str) -> Change | None:
        return owner.get(sha) or next((c for s, c in owner.items() if s.startswith(sha)), None)

    for change in changes:
        for path, ranges in diff_stats(repo, change, excludes):
            for origin_sha, lines in blame(repo, f"{change.sha}^1", path, ranges).items():
                origin = owner.get(origin_sha)
                if origin is None or origin is change:
                    continue
                days = (change.time - origin.time) / DAY
                if days > window_days:
                    continue
                origin.fixed_by[change.sha] = origin.fixed_by.get(change.sha, 0) + lines
                if origin.first_fix_days is None or days < origin.first_fix_days:
                    origin.first_fix_days = days
        if change.kind == "revert":
            body = git(repo, "log", "-1", "--format=%B", change.sha)
            origins = {o for sha in REVERTS.findall(body) if (o := resolve(sha))}
            if not origins and (m := REVERTED_TITLE.match(change.subject)):
                origins = {c for c in changes if c.subject == m.group(1) and c.time <= change.time}
            for origin in origins - {change}:
                origin.reverted_by.append(change.sha)
    return changes


def rows(changes: list[Change], key, now: int, window_days: int) -> list[dict]:
    groups = defaultdict(list)
    for c in changes:
        if c.kind in ("revert", "release") or now - c.time < window_days * DAY:
            continue
        groups[key(c)].append(c)
    out = []
    for name, cs in groups.items():
        regressed = [c for c in cs if c.fixed_by]
        added = sum(c.added for c in cs)
        fixed_lines = sum(sum(c.fixed_by.values()) for c in cs)
        out.append(
            {
                "group": name,
                "changes": len(cs),
                "reverted_pct": round(100 * sum(1 for c in cs if c.reverted_by) / len(cs), 1),
                "regressed_pct": round(100 * len(regressed) / len(cs), 1),
                "fix_share_pct": round(100 * sum(1 for c in cs if c.kind == "fix") / len(cs), 1),
                "lines_added": added,
                "fixed_lines_per_kloc": round(1000 * fixed_lines / added, 1) if added else 0.0,
                "median_days_to_fix": round(statistics.median(c.first_fix_days for c in regressed), 1)
                if regressed
                else None,
            }
        )
    return sorted(out, key=lambda r: -r["changes"])


def markdown(title: str, table: list[dict]) -> str:
    head = ["group", "changes", "reverted %", "regressed %", "fix share %", "lines added", "fixed lines/kLOC", "median days to fix"]
    lines = [f"### {title}", "", "| " + " | ".join(head) + " |", "|" + "---|" * len(head)]
    for r in table:
        cells = [str(v if v is not None else "–") for v in r.values()]
        lines.append("| " + " | ".join(cells) + " |")
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--repo", default=".")
    p.add_argument("--ref", default="main")
    p.add_argument("--window-days", type=int, default=14)
    p.add_argument("--exclude", action="append", default=[], help="extra path glob to ignore")
    p.add_argument("--json", metavar="PATH", help="write one row per change")
    a = p.parse_args(argv)

    changes = analyse(a.repo, a.ref, a.window_days, DEFAULT_EXCLUDES + a.exclude)
    now = int(git(a.repo, "log", "-1", "--format=%ct", a.ref))
    print(markdown("By agent", rows(changes, lambda c: c.agent, now, a.window_days)))
    print(markdown("By change type", rows(changes, lambda c: c.kind, now, a.window_days)))
    feats = [c for c in changes if c.kind == "feat"]
    print(markdown("Features only, by agent", rows(feats, lambda c: c.agent, now, a.window_days)))
    if a.json:
        with open(a.json, "w") as f:
            json.dump([c.__dict__ for c in changes], f, indent=1)
    return 0


if __name__ == "__main__":
    sys.exit(main())
