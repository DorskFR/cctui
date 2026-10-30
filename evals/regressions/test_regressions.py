import json
import os
import subprocess
import tempfile
import unittest
from contextlib import redirect_stdout
from io import StringIO

import regressions

DAY = 86400
T0 = 1_750_000_000


class Repo:
    def __init__(self, path: str):
        self.path = path
        self.run("init", "-q", "-b", "main")

    def run(self, *args: str, when: int = T0) -> str:
        env = {
            **os.environ,
            "GIT_AUTHOR_NAME": "Dev",
            "GIT_AUTHOR_EMAIL": "dev@example.com",
            "GIT_COMMITTER_NAME": "Dev",
            "GIT_COMMITTER_EMAIL": "dev@example.com",
            "GIT_AUTHOR_DATE": f"@{when} +0000",
            "GIT_COMMITTER_DATE": f"@{when} +0000",
        }
        return subprocess.run(
            ["git", "-C", self.path, "-c", "commit.gpgsign=false", *args],
            check=True, capture_output=True, text=True, env=env,
        ).stdout.strip()

    def commit(self, files: dict[str, str], message: str, day: float, model: str | None = None) -> str:
        for name, content in files.items():
            with open(os.path.join(self.path, name), "w") as f:
                f.write(content)
            self.run("add", name)
        if model:
            message += f"\n\nCo-Authored-By: Claude {model} <noreply@anthropic.com>"
        self.run("commit", "-q", "-m", message, when=T0 + int(day * DAY))
        return self.run("rev-parse", "HEAD")


def by_subject(changes):
    return {c.subject: c for c in changes}


class RegressionsTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Repo(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def analyse(self):
        return by_subject(regressions.analyse(self.tmp.name, "main", 14, regressions.DEFAULT_EXCLUDES))

    def test_fix_within_window_marks_origin_regressed(self):
        self.repo.commit({"a.py": "one\ntwo\nthree\n"}, "feat: add a", 0, "Opus 5.5 (1M context)")
        fix = self.repo.commit({"a.py": "one\nTWO\nthree\n"}, "fix: repair a", 2, "Fable 5.1")
        cs = self.analyse()
        self.assertEqual(cs["feat: add a"].agent, "Opus 5.5")
        self.assertEqual(cs["feat: add a"].fixed_by, {fix: 1})
        self.assertAlmostEqual(cs["feat: add a"].first_fix_days, 2)
        self.assertEqual(cs["fix: repair a"].fixed_by, {})

    def test_fix_after_window_is_not_a_regression(self):
        self.repo.commit({"a.py": "one\n"}, "feat: add a", 0, "Opus 5")
        self.repo.commit({"a.py": "ONE\n"}, "fix: late", 30)
        self.assertEqual(self.analyse()["feat: add a"].fixed_by, {})

    def test_non_fix_edit_is_not_a_regression(self):
        self.repo.commit({"a.py": "one\n"}, "feat: add a", 0, "Opus 5")
        self.repo.commit({"a.py": "ONE\n"}, "refactor: rename", 1)
        self.assertEqual(self.analyse()["feat: add a"].fixed_by, {})

    def test_version_lines_and_lockfiles_are_ignored(self):
        self.repo.commit({"Cargo.toml": 'version = "1.0.0"\n', "Cargo.lock": "x\n"}, "release: 1.0.0", 0)
        self.repo.commit({"Cargo.toml": 'version = "1.0.1"\n', "Cargo.lock": "y\n"}, "fix: bump (1.0.1)", 1)
        self.assertEqual(self.analyse()["release: 1.0.0"].fixed_by, {})

    def test_revert_by_sha_and_by_title(self):
        self.repo.commit({"a.py": "one\n"}, "feat: a (#1)", 0, "Opus 4.8")
        sha = self.repo.commit({"b.py": "b\n"}, "feat: b (#2)", 1, "Opus 4.8")
        self.repo.commit({"a.py": ""}, 'Revert "feat: a (#1)" (#3)', 2)
        self.repo.commit({"b.py": ""}, f'Revert "feat: b"\n\nThis reverts commit {sha}.', 3)
        cs = self.analyse()
        self.assertEqual(len(cs["feat: a (#1)"].reverted_by), 1)
        self.assertEqual(len(cs["feat: b (#2)"].reverted_by), 1)
        self.assertEqual(cs["feat: a (#1)"].pr, 1)

    def test_merged_branch_commits_belong_to_the_merge(self):
        self.repo.commit({"base.py": "base\n"}, "chore: init", 0)
        self.repo.run("switch", "-q", "-c", "topic")
        self.repo.commit({"a.py": "one\n"}, "feat: on branch", 1, "Fable 5")
        self.repo.run("switch", "-q", "main")
        self.repo.run("merge", "-q", "--no-ff", "topic", "-m", "Merge pull request #7 from o/topic\n\nfeat: topic work",
                      when=T0 + 2 * DAY)
        self.repo.commit({"a.py": "ONE\n"}, "fix: topic bug", 3)
        cs = self.analyse()
        merge = cs["feat: topic work"]
        self.assertEqual((merge.pr, merge.agent, merge.kind), (7, "Fable 5", "feat"))
        self.assertEqual(sum(merge.fixed_by.values()), 1)

    def test_report_leaves_out_changes_younger_than_window(self):
        self.repo.commit({"a.py": "one\n"}, "feat: old", 0, "Opus 5")
        self.repo.commit({"a.py": "ONE\n"}, "fix: old bug", 1, "Opus 5")
        self.repo.commit({"b.py": "b\n"}, "feat: new", 20, "Opus 5")
        out, path = StringIO(), os.path.join(self.tmp.name, "rows.json")
        with redirect_stdout(out):
            regressions.main(["--repo", self.tmp.name, "--json", path])
        self.assertIn("| Opus 5 | 2 | 0.0 | 50.0 | 50.0 |", out.getvalue())
        with open(path) as f:
            self.assertEqual(len(json.load(f)), 3)

    def test_model_names_are_normalised(self):
        self.assertEqual(regressions.agent_of(["Claude claude-opus-5 <x>"], ["Dev"]), "Opus 5")
        self.assertEqual(regressions.agent_of(["Claude <x>"], ["Dev"]), "Claude (unnamed)")
        self.assertEqual(regressions.agent_of([], ["dependabot[bot]"]), "dependabot")
        self.assertEqual(regressions.agent_of([], ["Dev"]), "human")


if __name__ == "__main__":
    unittest.main()
