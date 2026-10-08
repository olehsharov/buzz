"""Tests for scripts/claude-sessions-list (run: python3 scripts/test_claude_sessions_list.py)."""

from __future__ import annotations

import contextlib
import importlib.machinery
import importlib.util
import io
import json
import os
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
_loader = importlib.machinery.SourceFileLoader("claude_sessions_list", os.path.join(HERE, "claude-sessions-list"))
_spec = importlib.util.spec_from_loader("claude_sessions_list", _loader)
csl = importlib.util.module_from_spec(_spec)
_loader.exec_module(csl)

SID_A = "aaaaaaaa-1111-4111-8111-111111111111"
SID_B = "bbbbbbbb-2222-4222-8222-222222222222"
SID_C = "cccccccc-3333-4333-8333-333333333333"


def msg(sid, cwd="/home/u/proj", branch="main", kind="user", content="hello", **extra):
    rec = {
        "type": kind,
        "sessionId": sid,
        "cwd": cwd,
        "gitBranch": branch,
        "version": "2.1.287",
        "timestamp": "2026-10-01T00:00:00.000Z",
        "isSidechain": False,
        "message": {"role": kind, "content": content},
    }
    rec.update(extra)
    return rec


class Tree:
    def __init__(self, root):
        self.root = root
        os.makedirs(os.path.join(root, "projects"), exist_ok=True)
        os.makedirs(os.path.join(root, "sessions"), exist_ok=True)

    def session(self, sid, records, project="-home-u-proj", mtime=None, raw_lines=()):
        d = os.path.join(self.root, "projects", project)
        os.makedirs(d, exist_ok=True)
        path = os.path.join(d, sid + ".jsonl")
        with open(path, "w", encoding="utf-8") as fh:
            for line in raw_lines:
                fh.write(line + "\n")
            for rec in records:
                fh.write(json.dumps(rec, separators=(",", ":")) + "\n")
        if mtime is not None:
            os.utime(path, (mtime, mtime))
        return path

    def live(self, pid, sid, name=None, name_source=None, status="idle", proc_start="123"):
        data = {"pid": pid, "sessionId": sid, "cwd": "/x", "status": status, "updatedAt": 1, "procStart": proc_start}
        if name is not None:
            data["name"] = name
        if name_source is not None:
            data["nameSource"] = name_source
        with open(os.path.join(self.root, "sessions", f"{pid}.json"), "w") as fh:
            json.dump(data, fh)
        # A key file the tool must never need to open.
        with open(os.path.join(self.root, "sessions", f"{pid}.deadbeef.key"), "w") as fh:
            fh.write("SECRET")

    def history(self, entries):
        with open(os.path.join(self.root, "history.jsonl"), "w") as fh:
            for e in entries:
                fh.write(json.dumps(e) + "\n")


def never_live(pid, start):
    return False


class Base(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tree = Tree(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def rows(self, **kw):
        kw.setdefault("is_live", never_live)
        return csl.collect(self.tree.root, **kw)

    def one(self, **kw):
        rows = self.rows(**kw)
        self.assertEqual(len(rows), 1, rows)
        return rows[0]


class TitlePriority(Base):
    def test_custom_title_wins_and_last_occurrence_is_used(self):
        self.tree.session(SID_A, [
            msg(SID_A),
            {"type": "custom-title", "customTitle": "old name", "sessionId": SID_A},
            {"type": "ai-title", "aiTitle": "ai name", "sessionId": SID_A},
            {"type": "custom-title", "customTitle": "new   name\nhere", "sessionId": SID_A},
            {"type": "last-prompt", "lastPrompt": "lp", "sessionId": SID_A},
            msg(SID_A, kind="assistant", content="done"),
        ])
        self.tree.live(10, SID_A, name="user name", name_source="user")
        row = self.one(is_live=lambda p, s: True)
        self.assertEqual((row["title"], row["title_source"]), ("new name here", "custom-title"))

    def test_user_session_name_beats_ai_title(self):
        self.tree.session(SID_A, [msg(SID_A), {"type": "ai-title", "aiTitle": "ai name", "sessionId": SID_A}])
        self.tree.live(10, SID_A, name="My Session", name_source="user")
        row = self.one()
        self.assertEqual((row["title"], row["title_source"]), ("My Session", "session-name"))

    def test_derived_session_name_is_ignored(self):
        self.tree.session(SID_A, [
            msg(SID_A),
            {"type": "ai-title", "aiTitle": "first", "sessionId": SID_A},
            {"type": "ai-title", "aiTitle": "second", "sessionId": SID_A},
        ])
        self.tree.live(10, SID_A, name="derived-name", name_source="derived")
        row = self.one()
        self.assertEqual((row["title"], row["title_source"]), ("second", "ai-title"))

    def test_last_prompt_record(self):
        self.tree.session(SID_A, [
            msg(SID_A),
            {"type": "last-prompt", "lastPrompt": "first", "sessionId": SID_A},
            {"type": "last-prompt", "lastPrompt": "fix the build", "sessionId": SID_A},
        ])
        row = self.one()
        self.assertEqual((row["title"], row["title_source"]), ("fix the build", "last-prompt"))

    def test_last_user_prompt_skips_meta_tool_results_and_wrappers(self):
        self.tree.session(SID_A, [
            msg(SID_A, content="typed prompt"),
            msg(SID_A, content=[{"type": "tool_result", "content": "x"}]),
            msg(SID_A, content="<command-name>/clear</command-name>"),
            msg(SID_A, content="caveat", isMeta=True),
            msg(SID_A, kind="assistant", content="answer"),
        ])
        row = self.one()
        self.assertEqual((row["title"], row["title_source"]), ("typed prompt", "user-prompt"))

    def test_history_fallback(self):
        self.tree.session(SID_A, [msg(SID_A, content=[{"type": "tool_result", "content": "x"}])])
        self.tree.history([
            {"display": "from history 1", "project": "/p", "sessionId": SID_A, "timestamp": 1},
            {"display": "from history 2", "project": "/p", "sessionId": SID_A, "timestamp": 2},
            {"display": "other", "project": "/p", "sessionId": SID_B, "timestamp": 3},
        ])
        row = self.one()
        self.assertEqual((row["title"], row["title_source"]), ("from history 2", "history"))

    def test_basename_fallback(self):
        self.tree.session(SID_A, [msg(SID_A, cwd="/srv/repo", content=[{"type": "tool_result"}])])
        row = self.one()
        self.assertEqual((row["title"], row["title_source"]), ("repo aaaaaaaa", "fallback"))

    def test_title_truncated_to_80(self):
        self.tree.session(SID_A, [msg(SID_A), {"type": "custom-title", "customTitle": "x" * 200, "sessionId": SID_A}])
        row = self.one()
        self.assertEqual(len(row["title"]), 80)
        self.assertTrue(row["title"].endswith("…"))

    def test_nested_title_lookalike_is_not_trusted(self):
        nested = msg(SID_A, toolUseResult={"type": "ai-title", "aiTitle": "nested"})
        self.tree.session(SID_A, [msg(SID_A), {"type": "ai-title", "aiTitle": "real", "sessionId": SID_A}, nested])
        self.assertEqual(self.one()["title"], "real")

    def test_title_text_inside_message_is_not_a_record(self):
        self.tree.session(SID_A, [
            msg(SID_A, content='see {"type":"ai-title","aiTitle":"fake"}'),
        ])
        row = self.one()
        self.assertEqual(row["title_source"], "user-prompt")


class Exclusion(Base):
    def test_subagent_and_tool_result_dirs_are_skipped(self):
        self.tree.session(SID_A, [msg(SID_A)])
        sub = os.path.join(self.tree.root, "projects", "-home-u-proj", SID_A, "subagents")
        os.makedirs(sub)
        with open(os.path.join(sub, "agent-x.jsonl"), "w") as fh:
            fh.write(json.dumps(msg(SID_B)) + "\n")
        tr = os.path.join(self.tree.root, "projects", "-home-u-proj", "tool-results")
        os.makedirs(tr)
        with open(os.path.join(tr, SID_C + ".jsonl"), "w") as fh:
            fh.write(json.dumps(msg(SID_C)) + "\n")
        self.assertEqual([r["id"] for r in self.rows()], [SID_A])

    def test_sidechain_transcript_is_skipped(self):
        self.tree.session(SID_A, [msg(SID_A)])
        self.tree.session(SID_B, [msg(SID_B, isSidechain=True)])
        self.assertEqual([r["id"] for r in self.rows()], [SID_A])


class Fields(Base):
    def test_cwd_from_record_not_dir_name(self):
        self.tree.session(
            SID_A,
            [msg(SID_A, cwd="/home/u/my.proj", branch="feat/x")],
            project="-home-u-my-proj",
            raw_lines=['{"type":"permission-mode","permissionMode":"default"}', "not json"],
        )
        row = self.one()
        self.assertEqual((row["cwd"], row["git_branch"]), ("/home/u/my.proj", "feat/x"))
        self.assertEqual(row["size_bytes"], os.path.getsize(
            os.path.join(self.tree.root, "projects", "-home-u-my-proj", SID_A + ".jsonl")))

    def test_cwd_unknown_when_absent(self):
        self.tree.session(SID_A, [{"type": "custom-title", "customTitle": "t", "sessionId": SID_A}])
        row = self.one()
        self.assertIsNone(row["cwd"])
        self.assertEqual(row["title"], "t")

    def test_empty_file(self):
        self.tree.session(SID_A, [])
        row = self.one()
        self.assertEqual((row["title"], row["cwd"]), ("unknown aaaaaaaa", None))


class Liveness(Base):
    def test_live_fields_and_running_filter(self):
        self.tree.session(SID_A, [msg(SID_A)])
        self.tree.session(SID_B, [msg(SID_B)])
        self.tree.live(4242, SID_A, status="busy")
        self.tree.live(4343, SID_B)
        live = lambda pid, start: pid == 4242
        rows = {r["id"]: r for r in self.rows(is_live=live)}
        self.assertEqual((rows[SID_A]["live"], rows[SID_A]["pid"], rows[SID_A]["status"]), (True, 4242, "busy"))
        self.assertFalse(rows[SID_B]["live"])
        self.assertNotIn("pid", rows[SID_B])
        self.assertEqual([r["id"] for r in self.rows(is_live=live, running=True)], [SID_A])

    def test_proc_is_live_checks_start_time(self):
        with tempfile.TemporaryDirectory() as proc:
            os.makedirs(os.path.join(proc, "77"))
            fields = ["S"] + ["0"] * 18 + ["555"] + ["0"] * 5
            with open(os.path.join(proc, "77", "stat"), "w") as fh:
                fh.write("77 (cl aude) " + " ".join(fields))
            self.assertTrue(csl.proc_is_live(77, "555", proc))
            self.assertTrue(csl.proc_is_live(77, None, proc))
            self.assertFalse(csl.proc_is_live(77, "556", proc))  # pid reused
            self.assertFalse(csl.proc_is_live(78, "555", proc))
            self.assertFalse(csl.proc_is_live("77", "555", proc))


class Filters(Base):
    def setUp(self):
        super().setUp()
        now = time.time()
        self.tree.session(SID_A, [msg(SID_A, cwd="/home/u/proj")], mtime=now - 10)
        self.tree.session(SID_B, [msg(SID_B, cwd="/home/u/proj-two")], project="-home-u-proj-two", mtime=now - 5 * 86400)
        self.tree.session(SID_C, [msg(SID_C, cwd="/home/u/proj/sub")], mtime=now - 3600)

    def test_sorted_newest_first(self):
        self.assertEqual([r["id"] for r in self.rows()], [SID_A, SID_C, SID_B])

    def test_cwd_prefix_respects_path_boundary(self):
        self.assertEqual({r["id"] for r in self.rows(cwd_prefix="/home/u/proj")}, {SID_A, SID_C})
        self.assertEqual({r["id"] for r in self.rows(cwd_prefix="/home/u/proj/")}, {SID_A, SID_C})
        self.assertEqual({r["id"] for r in self.rows(cwd_prefix="/home/u")}, {SID_A, SID_B, SID_C})

    def test_since(self):
        self.assertEqual({r["id"] for r in self.rows(since=csl.parse_since("2d"))}, {SID_A, SID_C})
        self.assertEqual({r["id"] for r in self.rows(since=csl.parse_since("30m"))}, {SID_A})
        iso = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() - 7200))
        self.assertEqual({r["id"] for r in self.rows(since=csl.parse_since(iso))}, {SID_A, SID_C})

    def run_main(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = csl.main(["--claude-dir", self.tree.root, *argv], is_live=never_live)
        return code, out.getvalue(), err.getvalue()

    def test_cli_json_limit(self):
        code, out, _ = self.run_main("--json", "--limit", "2")
        self.assertEqual(code, 0)
        data = json.loads(out)
        self.assertEqual([r["id"] for r in data], [SID_A, SID_C])
        self.assertEqual(
            set(data[0]),
            {"id", "title", "title_source", "cwd", "git_branch", "last_modified", "size_bytes", "live"},
        )

    def test_cli_id_prefix_unique_missing_ambiguous(self):
        code, out, _ = self.run_main("--json", "--id", "bbbb")
        self.assertEqual((code, [r["id"] for r in json.loads(out)]), (0, [SID_B]))
        code, _, err = self.run_main("--id", "zzzz")
        self.assertEqual(code, 1)
        self.assertIn("no session", err)
        self.tree.session("bbbbbbbb-9999-4999-8999-999999999999", [msg("x")])
        code, _, err = self.run_main("--id", "bbbb")
        self.assertEqual(code, 1)
        self.assertIn("ambiguous", err)

    def test_cli_table(self):
        code, out, _ = self.run_main()
        self.assertEqual(code, 0)
        lines = out.splitlines()
        self.assertTrue(lines[0].startswith("LAST_MODIFIED"))
        self.assertEqual(len(lines), 4)
        self.assertIn(SID_A, lines[1])

    def test_claude_config_dir_env(self):
        old = os.environ.get("CLAUDE_CONFIG_DIR")
        os.environ["CLAUDE_CONFIG_DIR"] = self.tree.root
        try:
            self.assertEqual(csl.default_claude_dir(), self.tree.root)
        finally:
            if old is None:
                del os.environ["CLAUDE_CONFIG_DIR"]
            else:
                os.environ["CLAUDE_CONFIG_DIR"] = old


if __name__ == "__main__":
    unittest.main()
