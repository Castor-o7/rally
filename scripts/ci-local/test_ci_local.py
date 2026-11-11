"""Tests for ci-local (RAL-144). Run: python3 -m unittest discover -s scripts/ci-local"""

import io
import json
import os
import subprocess
import sys
import tempfile
import textwrap
import threading
import unittest
import urllib.error
from email.message import Message
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ci_local  # noqa: E402


def write(root: Path, rel: str, text: str) -> None:
    p = root / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(textwrap.dedent(text))


CONFIG = """
workflows = ["ci.yml"]

[jobs.check]
run = "cargo test"

[jobs.macos]
manual = "needs macOS"
"""

WORKFLOW = """
name: CI
on:
  workflow_dispatch:
jobs:
  check:
    runs-on: ubuntu-latest
  macos:
    runs-on: macos-15
"""


class ConfigTests(unittest.TestCase):
    def test_parses_jobs_and_manual(self):
        cfg = ci_local.parse_config(CONFIG)
        self.assertEqual(cfg.workflows, ["ci.yml"])
        self.assertEqual([j.name for j in cfg.runnable()], ["check"])
        self.assertEqual(cfg.jobs["macos"].manual, "needs macOS")

    def test_a_job_needs_run_or_manual_not_both(self):
        with self.assertRaisesRegex(ValueError, "exactly one"):
            ci_local.parse_config('[jobs.a]\nrun = "x"\nmanual = "y"\n')
        with self.assertRaisesRegex(ValueError, "exactly one"):
            ci_local.parse_config('[jobs.a]\nmem = "1G"\n')

    def test_an_aggregate_needs_jobs_run_here(self):
        cfg = ci_local.parse_config(CONFIG + '[jobs.all]\nneeds = ["check"]\n')
        self.assertEqual([a.name for a in cfg.aggregates()], ["all"])
        self.assertEqual([j.name for j in cfg.runnable()], ["check"])
        with self.assertRaisesRegex(ValueError, "isn't a job run here"):
            ci_local.parse_config(CONFIG + '[jobs.all]\nneeds = ["macos"]\n')
        with self.assertRaisesRegex(ValueError, "isn't a job run here"):
            ci_local.parse_config(CONFIG + '[jobs.all]\nneeds = ["nope"]\n')
        with self.assertRaisesRegex(ValueError, "exactly one"):
            ci_local.parse_config(CONFIG + '[jobs.all]\nneeds = ["check"]\nrun = "x"\n')

    def test_unknown_keys_are_refused(self):
        with self.assertRaisesRegex(ValueError, "unknown keys"):
            ci_local.parse_config('[jobs.a]\nrun = "x"\nruns_on = "y"\n')
        with self.assertRaisesRegex(ValueError, "unknown keys"):
            ci_local.parse_config('jobz = 1\n[jobs.a]\nrun = "x"\n')

    def test_no_jobs_is_refused(self):
        with self.assertRaisesRegex(ValueError, "no jobs"):
            ci_local.parse_config('workflows = []\n')

    def test_job_names_are_safe_for_paths_and_contexts(self):
        with self.assertRaisesRegex(ValueError, "names are"):
            ci_local.parse_config('[jobs."a/../b"]\nrun = "x"\n')

    def test_the_repos_file_lists_the_four_projects(self):
        repos = ci_local.load_repos()
        self.assertEqual(set(repos), {"rally", "marshal", "muster", "RootyRoopsDAW"})
        self.assertTrue(all(v.startswith("Castor-o7/") for v in repos.values()))


class DriftTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_in_step(self):
        write(self.root, ".ci/local.toml", CONFIG)
        write(self.root, ".github/workflows/ci.yml", WORKFLOW)
        self.assertEqual(ci_local.check_config(self.root), [])

    def test_a_workflow_job_missing_from_the_config_is_drift(self):
        write(self.root, ".ci/local.toml", CONFIG)
        write(self.root, ".github/workflows/ci.yml", WORKFLOW + "  audit:\n    runs-on: ubuntu-latest\n")
        problems = ci_local.check_config(self.root)
        self.assertEqual(len(problems), 1)
        self.assertIn("`audit`", problems[0])

    def test_a_push_or_pull_request_trigger_is_refused_in_any_workflow(self):
        write(self.root, ".ci/local.toml", CONFIG)
        write(self.root, ".github/workflows/ci.yml", WORKFLOW.replace(
            "  workflow_dispatch:", "  push:\n    branches: [main]\n  pull_request:"))
        write(self.root, ".github/workflows/other.yml", "on: push\njobs: {}\n")
        problems = ci_local.check_config(self.root)
        self.assertTrue(any("ci.yml" in p and "pull_request" in p and "push" in p for p in problems))
        self.assertTrue(any("other.yml" in p for p in problems))

    def test_an_exempt_workflow_may_keep_triggers_only_if_every_job_is_gated_elsewhere(self):
        write(self.root, ".ci/local.toml", CONFIG + '[exempt]\n"release.yml" = "upstream only"\n')
        write(self.root, ".github/workflows/ci.yml", WORKFLOW)
        write(self.root, ".github/workflows/release.yml", """
            on:
              push:
                branches: [main]
            jobs:
              release:
                if: github.repository == 'ignition-is-go/marshal'
              publish:
                if: github.repository == 'ignition-is-go/marshal' && needs.release.outputs.skip == 'false'
              notify:
                needs: [release, publish]
                if: needs.release.result == 'success'
        """)
        self.assertEqual(ci_local.check_config(self.root), [])
        write(self.root, ".github/workflows/release.yml", """
            on: push
            jobs:
              release:
                if: github.repository == 'ignition-is-go/marshal'
              notify:
                if: needs.release.result == 'success'
              report:
                needs: release
                if: always()
        """)
        problems = ci_local.check_config(self.root)
        self.assertEqual(len(problems), 2)
        self.assertIn("`notify`", problems[0])      # no needs and no gate
        self.assertIn("`report`", problems[1])      # always() runs it even when release is skipped

    def test_a_public_repository_keeps_its_hosted_triggers_but_not_larger_runners(self):
        public = 'hosted_runs = "public repository: standard runners bill nothing"\n' + CONFIG
        write(self.root, ".ci/local.toml", public)
        write(self.root, ".github/workflows/ci.yml", WORKFLOW.replace(
            "  workflow_dispatch:", "  push:\n    branches: [main]\n  pull_request:"))
        write(self.root, ".github/workflows/other.yml", "on: push\njobs: {}\n")
        self.assertEqual(ci_local.check_config(self.root), [])
        write(self.root, ".github/workflows/big.yml", """
            on: push
            jobs:
              fast:
                runs-on: ubuntu-latest-8-cores
              wide:
                runs-on: {group: x, labels: [windows-2022-xlarge]}
              arm:
                runs-on: ubuntu-24.04-arm
        """)
        problems = ci_local.check_config(self.root)
        self.assertEqual(len(problems), 2, problems)
        self.assertIn("`fast`", problems[0])
        self.assertIn("`wide`", problems[1])
        with self.assertRaisesRegex(ValueError, "say why"):
            ci_local.parse_config('hosted_runs = " "\n' + CONFIG)

    def test_an_exemption_needs_a_reason_and_a_workflow(self):
        with self.assertRaisesRegex(ValueError, "say why"):
            ci_local.parse_config(CONFIG + '[exempt]\n"release.yml" = ""\n')
        write(self.root, ".ci/local.toml", CONFIG + '[exempt]\n"gone.yml" = "x"\n')
        write(self.root, ".github/workflows/ci.yml", WORKFLOW)
        self.assertIn("gone.yml", ci_local.check_config(self.root)[0])

    def test_on_as_a_list(self):
        write(self.root, ".ci/local.toml", CONFIG)
        write(self.root, ".github/workflows/ci.yml",
              WORKFLOW.replace("on:\n  workflow_dispatch:", "on: [workflow_dispatch, schedule]"))
        problems = ci_local.check_config(self.root)
        self.assertTrue(any("schedule" in p for p in problems))

    def test_a_named_workflow_that_is_gone(self):
        write(self.root, ".ci/local.toml", CONFIG)
        self.assertIn("isn't there", ci_local.check_config(self.root)[0])

    def test_no_config(self):
        self.assertEqual(ci_local.check_config(self.root), [f"no {ci_local.CONFIG_PATH}"])


class FakeResponse(io.BytesIO):
    def __init__(self, body, headers):
        super().__init__(json.dumps(body).encode())
        self.headers = Message()
        for k, v in headers.items():
            self.headers[k] = v

    def __enter__(self):
        return self

    def __exit__(self, *a):
        self.close()


class FakeGitHub:
    """Answers like GitHub: ETags, 304 for an unchanged conditional request, rate headers."""

    def __init__(self):
        self.calls = []
        self.remaining = 4999
        self.pulls = [
            {"number": 7, "head": {"sha": "b" * 40, "repo": {"full_name": "Castor-o7/rally"}}},
            {"number": 8, "head": {"sha": "c" * 40, "repo": {"full_name": "someone/rally"}}},
            {"number": 9, "head": {"sha": "d" * 40, "repo": None}},  # the fork was deleted
        ]

    def __call__(self, req, data=None, timeout=None):
        self.calls.append((req.get_method(), req.full_url, req.headers.get("If-none-match"),
                           json.loads(data) if data else None))
        headers = {"X-RateLimit-Remaining": str(self.remaining), "X-RateLimit-Reset": "2000000000"}
        if req.get_method() == "POST":
            return FakeResponse({"state": "ok"}, headers)
        body = ({"commit": {"sha": "a" * 40}} if "/branches/" in req.full_url else self.pulls)
        etag = f'"{hash(json.dumps(body))}"'
        if req.headers.get("If-none-match") == etag:
            raise urllib.error.HTTPError(req.full_url, 304, "Not Modified", _headers(headers), None)
        return FakeResponse(body, {**headers, "ETag": etag})


def _headers(d):
    m = Message()
    for k, v in d.items():
        m[k] = v
    return m


class GitHubTests(unittest.TestCase):
    def test_heads_mark_forks_and_deleted_forks(self):
        gh = ci_local.GitHub("t", opener=FakeGitHub())
        heads = gh.heads("Castor-o7/rally")
        self.assertEqual([(h["ref"], h["fork"]) for h in heads],
                         [("main", False), ("pull/7", False), ("pull/8", True), ("pull/9", True)])

    def test_a_second_poll_is_conditional_and_reuses_the_answer_on_304(self):
        fake = FakeGitHub()
        gh = ci_local.GitHub("t", opener=fake)
        first = gh.heads("Castor-o7/rally")
        second = gh.heads("Castor-o7/rally")
        self.assertEqual(first, second)
        self.assertIsNone(fake.calls[0][2])
        self.assertIsNotNone(fake.calls[2][2])      # If-None-Match sent the second time

    def test_backs_off_below_the_floor_until_reset(self):
        fake = FakeGitHub()
        gh = ci_local.GitHub("t", opener=fake)
        gh.heads("Castor-o7/rally")
        self.assertEqual(gh.backoff_seconds(now=1_999_999_000), 0.0)
        fake.remaining = ci_local.RATE_FLOOR - 1
        gh.heads("Castor-o7/rally")
        self.assertEqual(gh.backoff_seconds(now=1_999_999_000), 1005.0)
        self.assertEqual(gh.backoff_seconds(now=2_000_000_100), 0.0)   # past the reset

    def test_status_body(self):
        fake = FakeGitHub()
        gh = ci_local.GitHub("t", opener=fake)
        gh.post_status("Castor-o7/rally", "a" * 40, "local/check", "success", "x" * 200)
        method, url, _, body = fake.calls[-1]
        self.assertEqual((method, url), ("POST", f"https://api.github.com/repos/Castor-o7/rally/statuses/{'a' * 40}"))
        self.assertEqual(body["context"], "local/check")
        self.assertEqual(len(body["description"]), 140)
        self.assertNotIn("target_url", body)


class MachineTests(unittest.TestCase):
    def test_mem_available(self):
        self.assertEqual(ci_local.mem_available_kb("MemTotal: 1 kB\nMemAvailable:   22156000 kB\n"), 22156000)

    def test_playing_sees_the_game_window_not_tests_or_the_editor(self):
        play = "/usr/bin/godot --path /home/joshd/Documents/GitHub/Discoteque-play "
        dev = "/usr/bin/godot --path /home/joshd/Documents/GitHub/Discoteque"
        self.assertTrue(ci_local.playing([play]))
        self.assertTrue(ci_local.playing([dev]))
        self.assertFalse(ci_local.playing(["godot --headless --path /home/joshd/Documents/GitHub/Discoteque --script t.gd"]))
        self.assertFalse(ci_local.playing(["/usr/bin/godot --path /home/joshd/Documents/GitHub/Discoteque --editor"]))
        self.assertFalse(ci_local.playing(["godot --path /home/joshd/.cache/vibrant-gull/rc/new"]))

    def test_slots_one_while_playing_else_the_dial(self):
        with tempfile.TemporaryDirectory() as d:
            f = Path(d) / "slots"
            old = ci_local.SLOTS_FILE
            ci_local.SLOTS_FILE = f
            try:
                self.assertEqual(ci_local.slot_count(False), ci_local.DEFAULT_SLOTS)
                f.write_text("3\n")
                self.assertEqual(ci_local.slot_count(False), 3)
                self.assertEqual(ci_local.slot_count(True), 1)
                f.write_text("junk")
                self.assertEqual(ci_local.slot_count(False), ci_local.DEFAULT_SLOTS)
            finally:
                ci_local.SLOTS_FILE = old


class CommandTests(unittest.TestCase):
    def test_the_job_runs_capped_in_a_scope(self):
        job = ci_local.Job(name="check", run="cargo test", mem="8G", cpu="600%", timeout_min=20)
        cmd = ci_local.job_command(job, play=False)
        self.assertEqual(cmd[:4], ["systemd-run", "--user", "--scope", "--quiet"])
        self.assertIn("--expand-environment=no", cmd)
        self.assertIn("MemoryMax=8G", cmd)
        self.assertIn("MemorySwapMax=0", cmd)
        self.assertIn("CPUQuota=600%", cmd)
        self.assertNotIn("--nice=19", cmd)
        self.assertEqual(cmd[cmd.index("--") + 1:], ["timeout", "--kill-after=30s", "20m",
                                                      "bash", "-eo", "pipefail", "-c", "cargo test"])

    def test_play_mode_lowers_cpu_and_priority_but_never_raises_cpu(self):
        hi = ci_local.job_command(ci_local.Job(name="a", run="x", cpu="600%"), play=True)
        self.assertIn("CPUQuota=200%", hi)
        self.assertIn("--nice=19", hi)
        lo = ci_local.job_command(ci_local.Job(name="a", run="x", cpu="100%"), play=True)
        self.assertIn("CPUQuota=100%", lo)

    def test_a_job_inherits_no_secrets_and_only_the_allow_list(self):
        base = {"PATH": "/usr/bin", "HOME": "/home/x", "LANG": "C.UTF-8", "LC_ALL": "C",
                "XDG_RUNTIME_DIR": "/run/user/1000", "GH_TOKEN": "t", "GITHUB_TOKEN": "t",
                "CI_LOCAL_TOKEN": "t", "CI_LOCAL_TOKEN_FILE": "/f", "SSH_AUTH_SOCK": "/s",
                "RALLY_DB_PASSWORD_FILE": "/p", "MY_SECRET": "s", "AWS_ACCESS_KEY": "k",
                "PGPASSWORD": "p", "EDITOR": "vim", "LC_TOKEN": "sneaky"}
        env = ci_local.job_env("rally", ci_local.Job(name="check", run="x"), Path("/t"), base=base)
        for k in ("PATH", "HOME", "LANG", "LC_ALL", "XDG_RUNTIME_DIR"):
            self.assertIn(k, env)
        for k in ("GH_TOKEN", "GITHUB_TOKEN", "CI_LOCAL_TOKEN", "CI_LOCAL_TOKEN_FILE", "SSH_AUTH_SOCK",
                  "RALLY_DB_PASSWORD_FILE", "MY_SECRET", "AWS_ACCESS_KEY", "PGPASSWORD", "EDITOR",
                  "LC_TOKEN"):
            self.assertNotIn(k, env)

    def test_a_job_cant_ask_for_a_secret_by_name(self):
        with self.assertRaisesRegex(ValueError, "looks like a secret"):
            ci_local.job_env("rally", ci_local.Job(name="c", run="x", env={"GH_TOKEN": "x"}), Path("/t"), base={})

    def test_env_keeps_builds_out_of_tmp(self):
        job = ci_local.Job(name="check", run="x", env={"FOO": "1"})
        env = ci_local.job_env("rally", job, Path("/home/x/.cache/ci/tmp/t"))
        self.assertEqual(env["TMPDIR"], "/home/x/.cache/ci/tmp/t")
        self.assertTrue(env["CARGO_TARGET_DIR"].endswith("/target/rally/check"))
        self.assertFalse(env["CARGO_TARGET_DIR"].startswith("/tmp"))
        self.assertEqual((env["CI"], env["FOO"]), ("true", "1"))


class TokenTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.file = Path(self.tmp.name) / "token"
        self.old = os.environ.get("CI_LOCAL_TOKEN_FILE")
        os.environ["CI_LOCAL_TOKEN_FILE"] = str(self.file)

    def tearDown(self):
        if self.old is None:
            os.environ.pop("CI_LOCAL_TOKEN_FILE", None)
        else:
            os.environ["CI_LOCAL_TOKEN_FILE"] = self.old
        self.tmp.cleanup()

    def test_posting_fails_closed_without_the_token_file(self):
        with self.assertRaises(SystemExit) as e:
            ci_local.token(posting=True)
        self.assertIn("won't fall back", str(e.exception))

    def test_a_run_that_doesnt_post_needs_no_token(self):
        self.assertIsNone(ci_local.token(posting=False))

    def test_the_file_must_be_private(self):
        self.file.write_text("tok\n")
        self.file.chmod(0o644)
        with self.assertRaisesRegex(SystemExit, "chmod 600"):
            ci_local.token(posting=True)
        self.file.chmod(0o600)
        self.assertEqual(ci_local.token(posting=True), "tok")

    def test_no_gh_fallback_anywhere(self):
        src = (Path(__file__).parent / "ci_local.py").read_text()
        self.assertNotIn('"gh", "auth", "token"', src)


class TmpTests(unittest.TestCase):
    def test_tmpdir_leaves_room_for_unix_sockets(self):
        t = ci_local.short_tmp("RootyRoopsDAW", "f" * 40, "windows-cross-check")
        # marshal's codex_bridge test binds a socket some 60 bytes deep in TMPDIR; 108 is the limit.
        self.assertLess(len(str(t)) + 60, 108)
        self.assertNotEqual(t, ci_local.short_tmp("RootyRoopsDAW", "f" * 40, "rust"))
        self.assertFalse(str(t).startswith("/tmp"))


class StateTests(unittest.TestCase):
    def test_survives_a_restart(self):
        with tempfile.TemporaryDirectory() as d:
            s = ci_local.State(Path(d) / "state.json")
            s.mark("rally@" + "a" * 40, "passed")
            again = ci_local.State(Path(d) / "state.json")
            self.assertTrue(again.done("rally@" + "a" * 40))
            self.assertFalse(again.done("rally@" + "b" * 40))


@unittest.skipUnless(os.environ.get("CI_LOCAL_SYSTEMD_TEST") == "1",
                     "set CI_LOCAL_SYSTEMD_TEST=1 to run a real job in a user scope")
class EndToEndTests(unittest.TestCase):
    """A real repository, a real scope: a passing job, a failing one and a timed-out one."""

    def test_run_commit_against_a_local_repository(self):
        with tempfile.TemporaryDirectory(dir=Path.home() / ".cache") as d:
            d = Path(d)
            repo = d / "src"
            repo.mkdir()
            write(repo, ".ci/local.toml", """
                workflows = ["ci.yml"]
                [jobs.good]
                run = "X=fine; test -f README && echo ${X}"
                mem = "256M"
                cpu = "50%"
                [jobs.bad]
                run = "echo broken >&2; exit 3"
                mem = "256M"
                [jobs.slow]
                run = "sleep 600"
                mem = "256M"
                timeout_min = 0.05
                [jobs.mac]
                manual = "needs macOS"
                [jobs.all-good]
                needs = ["good"]
                [jobs.all]
                needs = ["good", "bad"]
            """)
            write(repo, ".github/workflows/ci.yml", """
                on: workflow_dispatch
                jobs:
                  good: {}
                  bad: {}
                  slow: {}
                  mac: {}
                  all-good: {}
                  all: {}
            """)
            write(repo, "README", "x")
            g = ["git", "-C", str(repo)]
            subprocess.run(g + ["init", "-q", "-b", "main"], check=True)
            subprocess.run(g + ["-c", "user.name=t", "-c", "user.email=t@t", "add", "."], check=True)
            subprocess.run(g + ["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "x"], check=True)
            sha = subprocess.run(g + ["rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()

            old_root, old_mirror, old_fetch = ci_local.ROOT, ci_local.mirror, ci_local.fetch
            ci_local.ROOT = d / "ci"
            ci_local.mirror = lambda name, full: repo
            ci_local.fetch = lambda path, sha: None
            posted = []

            class Recorder:
                def post_status(self, full, sha, context, state, description):
                    posted.append((context, state))
            try:
                ok = ci_local.run_commit("demo", "x/demo", sha, Recorder(), threading.Event())
            finally:
                ci_local.ROOT, ci_local.mirror, ci_local.fetch = old_root, old_mirror, old_fetch
            self.assertFalse(ok)
            final = {c: s for c, s in posted}
            self.assertEqual(final["local/ci-config"], "success")
            self.assertEqual(final["local/good"], "success")
            self.assertEqual(final["local/bad"], "failure")
            self.assertEqual(final["local/slow"], "failure")
            self.assertNotIn("local/mac", final)
            self.assertEqual(final["local/all-good"], "success")
            self.assertEqual(final["local/all"], "failure")
            # every status was pending before its final answer
            self.assertEqual(posted.index(("local/all", "pending")) < posted.index(("local/all", "failure")), True)
            logs = d / "ci" / "demo" / sha
            self.assertIn("fine", (logs / "good.log").read_text())
            self.assertIn("broken", (logs / "bad.log").read_text())
            self.assertIn("# exit 124", (logs / "slow.log").read_text())


if __name__ == "__main__":
    unittest.main()
