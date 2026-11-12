#!/usr/bin/env python3
"""ci-local: the projects' CI, run on Murph beside (or, on a private repository, instead of) GitHub Actions (RAL-144).

Watches the repositories in repos.toml. For each push to main, and each open pull request whose
branch lives in the same repository (never a fork's), it checks out the commit, runs the jobs its
`.ci/local.toml` lists, each capped in its own systemd scope, and posts one commit status per job
to GitHub, named `local/<job>`. See docs/ci-local.md.

  ci_local.py serve                       watch and run (what the systemd unit runs)
  ci_local.py run <repo> <sha> [--post]   run one commit's jobs by hand; posts only with --post
  ci_local.py check-config <checkout>     the drift check: every workflow job mapped or manual

Standard library only (Python 3.11+), so the runner needs nothing installed.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import fcntl
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import threading
import time
import tomllib
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

HERE = Path(__file__).resolve().parent
HOME = Path.home()
ROOT = Path(os.environ.get("CI_LOCAL_ROOT", HOME / ".cache" / "ci"))
SLOTS_FILE = Path(os.environ.get("CI_LOCAL_SLOTS_FILE", HOME / ".config" / "ci-local.slots"))
DEFAULT_SLOTS = 2
POLL_SECONDS = 60
# Below this many requests left in the hour, the poller sleeps until GitHub's reset time.
RATE_FLOOR = 300
# New jobs wait while the machine has less than this free (Murph has no swap, /tmp is RAM).
MEM_FLOOR_KB = 6 * 1024 * 1024
# While Josh's game window is open: one job at a time, at the lowest priority, on less CPU.
PLAY_CPU_QUOTA = "200%"
PLAY_PATTERN = re.compile(r"godot( [^ ]+)* --path \S*/Documents/GitHub/Discoteque(-play)?( |$)")
CONFIG_PATH = ".ci/local.toml"
API = "https://api.github.com"
# Triggers a workflow may keep (RAL-144: none of them spends minutes on its own).
ALLOWED_TRIGGERS = {"workflow_dispatch", "workflow_call"}
# Runner labels that bill even on a public repository: GitHub's larger runners, named with their
# size, such as `ubuntu-latest-4-cores` or `windows-2022-xlarge`.
PAID_RUNNER = re.compile(r"(^|-)(x*large|\d+-cores?|gpu)($|-)")


def log(*parts: object) -> None:
    stamp = time.strftime("%Y-%m-%d %H:%M:%S")
    print(stamp, *parts, flush=True)


# --- Configuration ---------------------------------------------------------------------------


@dataclass
class Job:
    name: str
    run: str = ""
    mem: str = "6G"
    cpu: str = "400%"
    timeout_min: float = 30
    env: dict[str, str] = field(default_factory=dict)
    postgres: bool = False      # a scratch Postgres cluster, its URL in $CI_POSTGRES_URL
    manual: str = ""            # non-empty: not run here, and why (macOS, Windows, ...)
    needs: list[str] = field(default_factory=list)  # an aggregate: passes when all of these pass


@dataclass
class RepoConfig:
    workflows: list[str]
    jobs: dict[str, Job]
    exempt: dict[str, str] = field(default_factory=dict)   # workflow -> why it may keep a trigger
    # Why GitHub's hosted runs may stay on (a public repository, where standard runners bill
    # nothing); empty when every workflow must be dispatch-only.
    hosted_runs: str = ""

    def runnable(self) -> list[Job]:
        """Jobs with commands to run here (not manual, not aggregates)."""
        return [j for j in self.jobs.values() if j.run]

    def aggregates(self) -> list[Job]:
        return [j for j in self.jobs.values() if j.needs]


def parse_config(text: str) -> RepoConfig:
    data = tomllib.loads(text)
    unknown = set(data) - {"workflows", "jobs", "exempt", "hosted_runs"}
    if unknown:
        raise ValueError(f"unknown keys in {CONFIG_PATH}: {sorted(unknown)}")
    jobs: dict[str, Job] = {}
    for name, spec in data.get("jobs", {}).items():
        extra = set(spec) - {"run", "mem", "cpu", "timeout_min", "env", "postgres", "manual",
                             "needs"}
        if extra:
            raise ValueError(f"job {name}: unknown keys {sorted(extra)}")
        job = Job(name=name, **spec)
        if [bool(job.run), bool(job.manual), bool(job.needs)].count(True) != 1:
            raise ValueError(f"job {name}: give exactly one of `run`, `manual` or `needs`")
        if not re.fullmatch(r"[A-Za-z0-9_.-]+", name):
            raise ValueError(f"job {name}: names are letters, digits, '.', '_' and '-'")
        jobs[name] = job
    if not jobs:
        raise ValueError(f"{CONFIG_PATH} lists no jobs")
    for job in jobs.values():
        for n in job.needs:
            if n not in jobs or not jobs[n].run:
                raise ValueError(f"job {job.name}: needs `{n}`, which isn't a job run here")
    exempt = dict(data.get("exempt", {}))
    for wf, why in exempt.items():
        if not isinstance(why, str) or not why.strip():
            raise ValueError(f"exempt workflow {wf}: say why")
    hosted_runs = data.get("hosted_runs", "")
    if "hosted_runs" in data and (not isinstance(hosted_runs, str) or not hosted_runs.strip()):
        raise ValueError("hosted_runs: say why GitHub's hosted runs may stay on")
    return RepoConfig(workflows=list(data.get("workflows", [])), jobs=jobs, exempt=exempt,
                      hosted_runs=hosted_runs)


def load_repos(path: Path = HERE / "repos.toml") -> dict[str, str]:
    """Name -> owner/repo."""
    data = tomllib.loads(path.read_text())
    return {name: spec["github"] for name, spec in data["repos"].items()}


# --- The drift check -------------------------------------------------------------------------


def workflow_triggers(doc: dict) -> set[str]:
    # YAML 1.1 reads a bare `on:` key as the boolean True.
    on = doc.get("on", doc.get(True))
    if isinstance(on, str):
        return {on}
    if isinstance(on, list):
        return set(on)
    if isinstance(on, dict):
        return set(on)
    return set()


def ungated_jobs(doc: dict) -> set[str]:
    """Jobs that could run here. A job is gated when its `if` names `github.repository ==`, or when
    every job it needs is gated and its `if` can't override a skipped need (no always(),
    failure() or cancelled())."""
    jobs = doc.get("jobs") or {}
    gated: dict[str, bool] = {}

    def is_gated(name: str, seen: frozenset = frozenset()) -> bool:
        if name in gated:
            return gated[name]
        spec = jobs.get(name) or {}
        cond = re.sub(r"\s+", " ", str(spec.get("if", "")))
        needs = spec.get("needs") or []
        needs = [needs] if isinstance(needs, str) else list(needs)
        if re.search(r"github\.repository ?==", cond):
            ok = True
        elif needs and name not in seen and not re.search(r"always\(\)|failure\(\)|cancelled\(\)", cond):
            ok = all(is_gated(n, seen | {name}) for n in needs)
        else:
            ok = False
        gated[name] = ok
        return ok

    return {j for j in jobs if not is_gated(j)}


def paid_runners(doc: dict) -> list[tuple[str, str]]:
    """(job, label) for each job whose `runs-on` names a larger runner, which bills even on a
    public repository."""
    found = []
    for name, spec in (doc.get("jobs") or {}).items():
        runs_on = (spec or {}).get("runs-on", [])
        if isinstance(runs_on, dict):
            runs_on = runs_on.get("labels", [])
        labels = [runs_on] if isinstance(runs_on, str) else list(runs_on or [])
        found += [(name, str(label)) for label in labels if PAID_RUNNER.search(str(label))]
    return found


def check_config(checkout: Path, public: bool | None = None) -> list[str]:
    """Problems with a checkout's local CI config; empty when it's in step with its workflows.

    `public` is GitHub's answer for the repository: True public, False private, None not
    confirmed. A checkout declares `hosted_runs` itself, so it's honoured only when GitHub says the
    repository is public; otherwise it's a problem, and the workflows are held to the
    dispatch-only rule as if it weren't there (a private repository's hosted runs bill)."""
    import yaml  # only the drift check needs it; the runner itself doesn't

    cfg_file = checkout / CONFIG_PATH
    if not cfg_file.exists():
        return [f"no {CONFIG_PATH}"]
    try:
        cfg = parse_config(cfg_file.read_text())
    except (ValueError, tomllib.TOMLDecodeError) as e:
        return [f"{CONFIG_PATH}: {e}"]
    problems = []
    hosted = bool(cfg.hosted_runs) and public is True
    if cfg.hosted_runs and public is False:
        # Not worded `.ci/local.toml: ...`: that prefix means the config can't be read at all, and
        # stops every job; this only holds the workflows to the dispatch-only rule.
        problems.append("hosted_runs set on a private repository, where GitHub's hosted runs bill "
                        "minutes")
    elif cfg.hosted_runs and public is None:
        problems.append("hosted_runs is set, but couldn't confirm with GitHub that the repository "
                        "is public")
    wf_dir = checkout / ".github" / "workflows"
    on_disk = sorted(p.name for p in wf_dir.glob("*.y*ml")) if wf_dir.is_dir() else []
    for name in on_disk:
        doc = yaml.safe_load((wf_dir / name).read_text()) or {}
        spend = workflow_triggers(doc) - ALLOWED_TRIGGERS
        if hosted:
            # A public repository's standard runners bill nothing, so its workflows keep their
            # triggers; only a larger runner would spend.
            for job, label in paid_runners(doc):
                problems.append(f"{name}: job `{job}` runs on `{label}`, a larger runner, which "
                                f"bills even on a public repository")
        elif spend and name in cfg.exempt:
            # Allowed only when no job can run here: each is gated to another repository (marshal's
            # upstream workflows, kept as upstream wrote them, run on upstream's runners only).
            for job in sorted(ungated_jobs(doc)):
                problems.append(f"{name}: exempt, but job `{job}` isn't gated to another "
                                f"repository (`if: github.repository == ...`), so it would run")
        elif spend:
            problems.append(f"{name}: triggers {sorted(spend)} would run on GitHub's runners; "
                            f"keep only {sorted(ALLOWED_TRIGGERS)}")
    for name in cfg.exempt:
        if name not in on_disk:
            problems.append(f"{CONFIG_PATH} exempts workflow {name}, which isn't there")
    for name in cfg.workflows:
        path = wf_dir / name
        if not path.exists():
            problems.append(f"{CONFIG_PATH} names workflow {name}, which isn't there")
            continue
        doc = yaml.safe_load(path.read_text()) or {}
        for job in doc.get("jobs", {}):
            if job not in cfg.jobs:
                problems.append(f"{name}: job `{job}` is neither run here nor marked manual "
                                f"in {CONFIG_PATH}")
    return problems


# --- GitHub ----------------------------------------------------------------------------------


class GitHub:
    """The few calls the runner needs, with ETags and a rate-limit floor."""

    def __init__(self, token: str | None, opener=urllib.request.urlopen):
        self.token = token
        self.opener = opener
        self.etags: dict[str, tuple[str, object]] = {}
        self.remaining: int | None = None
        self.reset_at: float = 0.0
        self.lock = threading.Lock()

    def _request(self, method: str, path: str, body: dict | None = None,
                 conditional: bool = False):
        url = API + path
        req = urllib.request.Request(url, method=method)
        if self.token:      # none: only public answers, as for anyone
            req.add_header("Authorization", f"Bearer {self.token}")
        req.add_header("Accept", "application/vnd.github+json")
        req.add_header("X-GitHub-Api-Version", "2022-11-28")
        req.add_header("User-Agent", "ci-local")
        cached = self.etags.get(url) if conditional else None
        if cached:
            req.add_header("If-None-Match", cached[0])
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            req.add_header("Content-Type", "application/json")
        try:
            with self.opener(req, data=data, timeout=30) as resp:
                self._note_limits(resp.headers)
                payload = json.loads(resp.read() or b"null")
                if conditional and resp.headers.get("ETag"):
                    self.etags[url] = (resp.headers["ETag"], payload)
                return payload
        except urllib.error.HTTPError as e:
            self._note_limits(e.headers)
            if e.code == 304 and cached:      # unchanged: free, and the cached answer stands
                return cached[1]
            raise

    def _note_limits(self, headers) -> None:
        if headers is None:
            return
        with self.lock:
            if headers.get("X-RateLimit-Remaining") is not None:
                self.remaining = int(headers["X-RateLimit-Remaining"])
            if headers.get("X-RateLimit-Reset") is not None:
                self.reset_at = float(headers["X-RateLimit-Reset"])

    def backoff_seconds(self, now: float | None = None) -> float:
        """How long to wait before polling again because of the rate limit (0: go ahead)."""
        now = time.time() if now is None else now
        if self.remaining is not None and self.remaining < RATE_FLOOR and self.reset_at > now:
            return self.reset_at - now + 5
        return 0.0

    def heads(self, full: str) -> list[dict]:
        """What to test: main's head and each open pull request's head, marked fork or not."""
        out = []
        main = self._request("GET", f"/repos/{full}/branches/main", conditional=True)
        out.append({"sha": main["commit"]["sha"], "ref": "main", "fork": False})
        pulls = self._request("GET", f"/repos/{full}/pulls?state=open&per_page=100",
                              conditional=True)
        for pr in pulls:
            head_repo = (pr.get("head") or {}).get("repo") or {}
            out.append({
                "sha": pr["head"]["sha"],
                "ref": f"pull/{pr['number']}",
                "fork": head_repo.get("full_name") != full,
            })
        return out

    def is_public(self, full: str) -> bool | None:
        """GitHub's word on whether the repository is public: True or False, or None when it
        couldn't be asked or didn't say (a private repository asked without a token is a 404)."""
        try:
            repo = self._request("GET", f"/repos/{full}", conditional=True)
        except (urllib.error.URLError, OSError, ValueError) as e:
            log(f"{full}: couldn't read the repository's visibility: {e}")
            return None
        if not isinstance(repo, dict) or not isinstance(repo.get("private"), bool):
            return None
        return repo["private"] is False and repo.get("visibility", "public") == "public"

    def post_status(self, full: str, sha: str, context: str, state: str, description: str):
        body = {"state": state, "context": context, "description": description[:140]}
        return self._request("POST", f"/repos/{full}/statuses/{sha}", body)


TOKEN_FILE_DEFAULT = HOME / ".config" / "ci-local" / "token"


def token_file() -> Path:
    return Path(os.environ.get("CI_LOCAL_TOKEN_FILE") or TOKEN_FILE_DEFAULT)


def token(posting: bool) -> str | None:
    """The token for GitHub's API. Posting (serve, run --post) needs the fine-grained token in its
    file and fails closed without it: never Josh's full-scope `gh` token. A run that doesn't post
    uses no token at all for the API."""
    path = token_file()
    if path.exists():
        if path.stat().st_mode & 0o077:
            raise SystemExit(f"ci-local: {path} must be readable by its owner only (chmod 600)")
        return path.read_text().strip()
    if posting:
        raise SystemExit(f"ci-local: posting statuses needs the fine-grained token in {path} "
                         "(docs/ci-local.md, setting it up); it won't fall back to gh's token")
    return None


# --- The machine -----------------------------------------------------------------------------


def mem_available_kb(meminfo: str | None = None) -> int:
    text = meminfo if meminfo is not None else Path("/proc/meminfo").read_text()
    m = re.search(r"^MemAvailable:\s+(\d+) kB", text, re.M)
    return int(m.group(1)) if m else 0


def playing(cmdlines: list[str] | None = None) -> bool:
    """Josh's DiscoSim window is open (a godot on his play or dev checkout that isn't headless)."""
    if cmdlines is None:
        cmdlines = []
        for p in Path("/proc").glob("[0-9]*/cmdline"):
            try:
                cmdlines.append(p.read_bytes().replace(b"\0", b" ").decode(errors="replace"))
            except OSError:
                continue
    for c in cmdlines:
        if PLAY_PATTERN.search(c) and "--headless" not in c and " --editor" not in c:
            return True
    return False


def slot_count(play: bool) -> int:
    if play:
        return 1
    try:
        return max(1, int(SLOTS_FILE.read_text().strip()))
    except (OSError, ValueError):
        return DEFAULT_SLOTS


class Slot:
    """One of N machine-wide job slots, as dsm-heavy's: a lock file each."""

    def __init__(self, stop: threading.Event | None = None):
        self.stop = stop or threading.Event()
        self.fd = None
        self.number = 0

    def __enter__(self):
        locks = ROOT / "locks"
        locks.mkdir(parents=True, exist_ok=True)
        announced = False
        while not self.stop.is_set():
            play = playing()
            if mem_available_kb() >= MEM_FLOOR_KB:
                for i in range(1, slot_count(play) + 1):
                    fd = os.open(locks / f"slot.{i}.lock", os.O_CREAT | os.O_RDWR, 0o600)
                    try:
                        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    except BlockingIOError:
                        os.close(fd)
                        continue
                    self.fd, self.number = fd, i
                    return self
                why = "all slots busy"
            else:
                why = "MemAvailable under 6 GB"
            if not announced:
                log(f"waiting for a slot: {why}")
                announced = True
            self.stop.wait(5)
        raise InterruptedError("stopping")

    def __exit__(self, *exc):
        if self.fd is not None:
            fcntl.flock(self.fd, fcntl.LOCK_UN)
            os.close(self.fd)


# --- Running a commit ------------------------------------------------------------------------


# git asks this for credentials: the fine-grained token's file when there is one (it has Contents:
# read on the four repositories), otherwise gh's (only for runs that don't post; the public
# repositories need none).
_FILE_HELPER = ('!f() { test "$1" = get || exit 0; echo username=x-access-token; '
                'printf "password=%s\\n" "$(cat "$CI_LOCAL_TOKEN_FILE")"; }; f')


def git(*args: str, cwd: Path | None = None, check: bool = True) -> str:
    env = dict(os.environ, GIT_TERMINAL_PROMPT="0")
    if token_file().exists():
        env["CI_LOCAL_TOKEN_FILE"] = str(token_file())
        helper = _FILE_HELPER
    else:
        helper = "!gh auth git-credential"
    return subprocess.run(["git", "-c", "credential.helper=", "-c", f"credential.helper={helper}",
                           *args], cwd=cwd, check=check, capture_output=True, text=True,
                          env=env).stdout


_repo_locks: dict[str, threading.Lock] = {}
_repo_locks_guard = threading.Lock()


def repo_lock(name: str) -> threading.Lock:
    """Serialises git operations on one mirror (fetches and worktree adds would race)."""
    with _repo_locks_guard:
        return _repo_locks.setdefault(name, threading.Lock())


def mirror(name: str, full: str) -> Path:
    path = ROOT / "mirror" / f"{name}.git"
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        git("clone", "--bare", "--quiet", f"https://github.com/{full}.git", str(path))
        git("config", "remote.origin.fetch", "+refs/heads/*:refs/heads/*", cwd=path)
        git("config", "--add", "remote.origin.fetch", "+refs/pull/*/head:refs/pull/*/head", cwd=path)
    return path


def fetch(path: Path, sha: str) -> None:
    git("fetch", "--quiet", "--prune", "origin", cwd=path)
    git("cat-file", "-e", f"{sha}^{{commit}}", cwd=path)


def job_command(job: Job, play: bool) -> list[str]:
    """The job inside its systemd scope: capped memory and CPU, and `timeout` as the backstop."""
    cpu = job.cpu
    if play:
        cpu = PLAY_CPU_QUOTA if _pct(job.cpu) > _pct(PLAY_CPU_QUOTA) else job.cpu
    # --expand-environment=no: systemd-run would otherwise expand ${VAR} in the script itself,
    # from its own environment, before bash sees it (muster's ${BUN_VERSION} came out empty).
    cmd = ["systemd-run", "--user", "--scope", "--quiet", "--collect", "--expand-environment=no",
           "-p", f"MemoryMax={job.mem}", "-p", "MemorySwapMax=0", "-p", f"CPUQuota={cpu}"]
    if play:
        cmd.append("--nice=19")
    cmd += ["--", "timeout", "--kill-after=30s", f"{job.timeout_min}m",
            "bash", "-eo", "pipefail", "-c", job.run]
    return cmd


def _pct(q: str) -> int:
    return int(q.rstrip("%"))


# What a job inherits from the runner: nothing else. Jobs run as joshd and can read ~ anyway
# (docs/ci-local.md, "Trust"), but no token, agent socket or password rides in on the environment.
ENV_ALLOW = {"PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LANGUAGE", "TZ", "TERM",
             # systemd-run --user and flatpak reach the user's manager and bus through these
             "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS",
             "CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN"}
ENV_SECRET = re.compile(r"TOKEN|SECRET|PASSWORD|PASSWD|CREDENTIAL|_KEY$|^SSH_|^GH_|^GITHUB_", re.I)


def job_env(name: str, job: Job, tmp: Path, base: dict[str, str] | None = None) -> dict[str, str]:
    base = dict(os.environ) if base is None else base
    env = {k: v for k, v in base.items()
           if (k in ENV_ALLOW or k.startswith("LC_")) and not ENV_SECRET.search(k)}
    env.update({
        "CI": "true",
        "CI_LOCAL": "1",
        "CARGO_TERM_COLOR": "never",
        "CARGO_TARGET_DIR": str(ROOT / "target" / name / job.name),
        "TMPDIR": str(tmp),
    })
    for k, v in job.env.items():
        if ENV_SECRET.search(k):
            raise ValueError(f"job {job.name}: env {k} looks like a secret; jobs get none")
        env[k] = v
    return env


PG_START = r"""
ci_pg_dir="$TMPDIR/pg"
initdb -D "$ci_pg_dir" -U postgres --auth=trust -E UTF8 >/dev/null
ci_pg_port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
pg_ctl -D "$ci_pg_dir" -o "-p $ci_pg_port -k $ci_pg_dir -c listen_addresses=127.0.0.1" -l "$ci_pg_dir.log" -w start >/dev/null
trap 'pg_ctl -D "$ci_pg_dir" -m fast -w stop >/dev/null 2>&1 || true' EXIT
export CI_POSTGRES_PORT=$ci_pg_port
export CI_POSTGRES_URL="postgres://postgres@127.0.0.1:$ci_pg_port/postgres"
export CI_PG_BIN="$(dirname "$(command -v pg_ctl)")"
"""


def short_tmp(name: str, sha: str, job: str) -> Path:
    """The job's TMPDIR: short, because tests put Unix sockets in it and a socket's path must fit
    in 108 bytes (hosted runners' /tmp is 4). ~/.cache/ci/t/<10 hex> is about 32."""
    import hashlib
    return ROOT / "t" / hashlib.sha256(f"{name}@{sha}:{job}".encode()).hexdigest()[:10]


class RunLock:
    """One run at a time of a repository's job at a commit, which share a checkout, a TMPDIR (with
    the job's scratch Postgres in it) and a log. Taken before anything is cleaned, and handed to
    the job's scope (`fd`, passed to it) so it's held until the job really ends, even when
    ci-local itself stops: a restart reruns commits whose results it never recorded, while their
    first runs go on in their scopes. Without it, the rerun deleted the first run's Postgres
    directory, and the first run's exit stopped the rerun's Postgres (2026-11-11, rally#229's
    false red: 47 store tests "can't reach test Postgres")."""

    def __init__(self, name: str, sha: str, job: str, stop: threading.Event | None = None):
        locks = ROOT / "locks"
        locks.mkdir(parents=True, exist_ok=True)
        self.path = locks / f"run-{name}-{sha[:12]}-{job}.lock"
        self.stop = stop or threading.Event()
        self.fd: int | None = None

    def __enter__(self):
        announced = False
        while True:
            fd = os.open(self.path, os.O_CREAT | os.O_RDWR, 0o600)
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                os.close(fd)
            else:
                self.fd = fd
                return self
            if self.stop.is_set():
                raise InterruptedError("stopping")
            if not announced:
                log(f"waiting: an earlier run of {self.path.stem[4:]} is still going")
                announced = True
            self.stop.wait(5)

    def __exit__(self, *exc):
        # Closed, never unlocked: unlocking would free it for the job's processes too, which hold
        # the same open file; closing leaves it held until the last of them ends.
        if self.fd is not None:
            os.close(self.fd)


def run_job(name: str, sha: str, job: Job, src: Path, stop: threading.Event) -> tuple[bool, str]:
    """Runs one job in its own checkout. Returns (passed, one-line summary)."""
    logdir = ROOT / name / sha
    logdir.mkdir(parents=True, exist_ok=True)
    logfile = logdir / f"{job.name}.log"
    work = ROOT / "work" / name / f"{sha[:12]}-{job.name}"
    tmp = short_tmp(name, sha, job.name)
    with RunLock(name, sha, job.name, stop) as run_lock, Slot(stop) as slot:
        play = playing()
        for d in (work, tmp):
            if d.exists():
                shutil.rmtree(d, ignore_errors=True)
        tmp.mkdir(parents=True)
        with repo_lock(name):
            git("worktree", "prune", cwd=src)
            git("worktree", "add", "--detach", "--force", str(work), sha, cwd=src)
        script = (PG_START if job.postgres else "") + job.run
        run_spec = Job(**{**job.__dict__, "run": script})
        cmd = job_command(run_spec, play)
        started = time.time()
        with open(logfile, "w") as out:
            out.write(f"# ci-local {name}@{sha} job {job.name}, slot {slot.number}"
                      f"{', throttled (play window open)' if play else ''}\n"
                      f"# MemoryMax={job.mem} CPUQuota={job.cpu} timeout={job.timeout_min}m\n"
                      f"# {shlex.join(cmd[:cmd.index('--')])}\n\n")
            out.flush()
            rc = subprocess.run(cmd, cwd=work, env=job_env(name, job, tmp), stdout=out,
                                stderr=subprocess.STDOUT, pass_fds=(run_lock.fd,)).returncode
            secs = int(time.time() - started)
            out.write(f"\n# exit {rc} after {secs}s\n")
        shutil.rmtree(tmp, ignore_errors=True)
        shutil.rmtree(work, ignore_errors=True)
        with repo_lock(name):
            git("worktree", "prune", cwd=src)
    timed_out = rc == 124
    verdict = "passed" if rc == 0 else ("timed out" if timed_out else f"failed (exit {rc})")
    return rc == 0, f"{verdict} in {secs // 60}m{secs % 60:02d}s on Murph; log {logfile}"


def read_config_at(src: Path, sha: str) -> RepoConfig:
    text = git("show", f"{sha}:{CONFIG_PATH}", cwd=src, check=False)
    if not text:
        raise ValueError(f"no {CONFIG_PATH} at this commit")
    return parse_config(text)


def drift_at(src: Path, sha: str, public: bool | None = None) -> list[str]:
    """The drift check against a commit, in a throwaway checkout of just the files it reads."""
    tmp = ROOT / "tmp" / f"config-{sha[:12]}"
    shutil.rmtree(tmp, ignore_errors=True)
    tmp.mkdir(parents=True)
    try:
        paths = [CONFIG_PATH, ".github/workflows"]
        archive = subprocess.run(["git", "archive", sha, "--", *paths], cwd=src,
                                 capture_output=True)
        if archive.returncode != 0:      # one of the paths is missing; try them one at a time
            for p in paths:
                one = subprocess.run(["git", "archive", sha, "--", p], cwd=src, capture_output=True)
                if one.returncode == 0:
                    subprocess.run(["tar", "-x", "-C", str(tmp)], input=one.stdout, check=True)
        else:
            subprocess.run(["tar", "-x", "-C", str(tmp)], input=archive.stdout, check=True)
        return check_config(tmp, public)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def _has_config(src: Path, rev: str) -> bool:
    try:
        git("cat-file", "-e", f"{rev}:{CONFIG_PATH}", cwd=src)
        return True
    except subprocess.CalledProcessError:
        return False


def predates_local_ci(src: Path, sha: str) -> bool:
    """Whether a commit is on a branch from before local CI: neither it nor the commit where it
    left main has the config, so there's nothing to check and a red `local/ci-config` would be
    noise. A branch that removed the file from a base that had it doesn't count, and neither does
    one with no history in common with main: those are still checked, and fail."""
    if _has_config(src, sha):
        return False
    base = git("merge-base", sha, "refs/heads/main", cwd=src, check=False).strip()
    return bool(base) and not _has_config(src, base)


def run_commit(name: str, full: str, sha: str, gh: GitHub | None,
               stop: threading.Event | None = None, only: list[str] | None = None) -> bool | None:
    """Runs every job for one commit, posting statuses when `gh` is given. True when all passed;
    None when the commit predates local CI, and nothing was run or posted."""
    stop = stop or threading.Event()

    def post(context: str, state: str, desc: str) -> None:
        if gh is None:
            return
        try:
            gh.post_status(full, sha, context, state, desc)
        except urllib.error.URLError as e:
            log(f"{name}@{sha[:12]}: couldn't post {context} {state}: {e}")

    with repo_lock(name):
        src = mirror(name, full)
        fetch(src, sha)
    if predates_local_ci(src, sha):
        log(f"{name}@{sha[:12]}: no {CONFIG_PATH} here or where it left main, so it predates "
            f"local CI; nothing run or posted (it runs once the branch has main's config)")
        return None
    # Only a commit that declares hosted_runs needs GitHub's word that the repository is public:
    # asked with the runner's token when posting, otherwise as anyone, which a public repo answers.
    try:
        declared = bool(read_config_at(src, sha).hosted_runs)
    except Exception:      # no config, or one that won't parse: the drift check says which
        declared = False
    public = (gh or GitHub(None)).is_public(full) if declared else None
    problems = drift_at(src, sha, public)
    if problems:
        log(f"{name}@{sha[:12]}: config: " + "; ".join(problems))
        post("local/ci-config", "failure", problems[0])
        if any(p.startswith(f"no {CONFIG_PATH}") or p.startswith(f"{CONFIG_PATH}:") for p in problems):
            return False
    else:
        post("local/ci-config", "success", "jobs and workflows in step; no workflow spends minutes")
    cfg = read_config_at(src, sha)
    jobs = [j for j in cfg.runnable() if only is None or j.name in only]
    aggregates = [a for a in cfg.aggregates() if only is None or a.name in only]
    for a in aggregates:      # an aggregate asked for by name brings its jobs with it
        jobs += [cfg.jobs[n] for n in a.needs if cfg.jobs[n] not in jobs]
    for j in jobs + aggregates:
        post(f"local/{j.name}", "pending", "queued on Murph")
    results: dict[str, bool] = {}

    def one(job: Job) -> None:
        post(f"local/{job.name}", "pending", "running on Murph")
        try:
            ok, summary = run_job(name, sha, job, src, stop)
        except InterruptedError:
            post(f"local/{job.name}", "error", "ci-local stopped before it ran; it reruns on start")
            results[job.name] = False
            return
        except Exception as e:  # a runner fault, not the job's: say so rather than "failed"
            log(f"{name}@{sha[:12]} {job.name}: runner error: {e!r}")
            post(f"local/{job.name}", "error", f"ci-local error: {e}"[:140])
            results[job.name] = False
            return
        results[job.name] = ok
        log(f"{name}@{sha[:12]} {job.name}: {summary}")
        post(f"local/{job.name}", "success" if ok else "failure", summary)

    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, len(jobs))) as pool:
        list(pool.map(one, jobs))
    for a in aggregates:
        failed = [n for n in a.needs if not results.get(n)]
        results[a.name] = not failed
        if failed:
            post(f"local/{a.name}", "failure", f"needs {', '.join(a.needs)}; failed: {', '.join(failed)}")
        else:
            post(f"local/{a.name}", "success", f"{', '.join(a.needs)} all passed")
        log(f"{name}@{sha[:12]} {a.name}: {'passed' if not failed else 'failed: ' + ', '.join(failed)}")
    return all(results.get(j.name) for j in jobs + aggregates) and not problems


# --- The watcher -----------------------------------------------------------------------------


class State:
    """Which commits have been run, so a restart doesn't run them again."""

    def __init__(self, path: Path = ROOT / "state.json"):
        self.path = path
        self.lock = threading.Lock()
        try:
            self.data = json.loads(path.read_text())
        except (OSError, ValueError):
            self.data = {"done": {}}

    def done(self, key: str) -> bool:
        with self.lock:
            return key in self.data["done"]

    def mark(self, key: str, result: str) -> None:
        with self.lock:
            self.data["done"][key] = {"result": result, "at": int(time.time())}
            self.path.parent.mkdir(parents=True, exist_ok=True)
            tmp = self.path.with_suffix(".tmp")
            tmp.write_text(json.dumps(self.data, indent=1, sort_keys=True))
            tmp.replace(self.path)


def serve(repos: dict[str, str], gh: GitHub, state: State, once: bool = False) -> None:
    stop = threading.Event()
    running: set[str] = set()
    pool = concurrent.futures.ThreadPoolExecutor(max_workers=8)
    log(f"ci-local watching {', '.join(repos.values())} every {POLL_SECONDS}s; "
        f"slots from {SLOTS_FILE} (default {DEFAULT_SLOTS}); logs under {ROOT}")

    def work(name: str, full: str, sha: str, key: str) -> None:
        try:
            ok = run_commit(name, full, sha, gh, stop)
            state.mark(key, "predates-local-ci" if ok is None else "passed" if ok else "failed")
        except InterruptedError:
            pass
        except Exception as e:
            log(f"{key}: {e!r}")
            state.mark(key, f"error: {e}")
        finally:
            running.discard(key)

    try:
        while True:
            wait = gh.backoff_seconds()
            if wait:
                log(f"rate limit: {gh.remaining} requests left; backing off {int(wait)}s "
                    f"until GitHub's reset")
                time.sleep(wait)
                log("rate limit: resuming")
            for name, full in repos.items():
                try:
                    heads = gh.heads(full)
                except (urllib.error.URLError, KeyError, ValueError) as e:
                    log(f"{full}: couldn't list heads: {e}")
                    continue
                for h in heads:
                    key = f"{name}@{h['sha']}"
                    if key in running or state.done(key):
                        continue
                    if h["fork"]:
                        try:
                            gh.post_status(full, h["sha"], "local/skipped-fork", "failure",
                                           "a fork's branch isn't run on Murph; a maintainer "
                                           "pushes it to this repository to test it")
                        except urllib.error.URLError as e:
                            log(f"{key}: couldn't post skipped-fork: {e}")
                            continue
                        state.mark(key, "skipped-fork")
                        log(f"{key} ({h['ref']}): a fork's, skipped")
                        continue
                    log(f"{key} ({h['ref']}): queued")
                    running.add(key)
                    pool.submit(work, name, full, h["sha"], key)
            if once:
                break
            time.sleep(POLL_SECONDS)
    except KeyboardInterrupt:
        pass
    finally:
        stop.set()
        pool.shutdown(wait=True)


# --- Command line ----------------------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="ci-local", description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("serve", help="watch the repositories and run what's new")
    s.add_argument("--once", action="store_true", help="one pass, then wait for its jobs and exit")
    r = sub.add_parser("run", help="run one commit's jobs by hand")
    r.add_argument("repo")
    r.add_argument("sha")
    r.add_argument("--post", action="store_true", help="also post the statuses to GitHub")
    r.add_argument("--job", action="append", help="only this job (repeatable)")
    c = sub.add_parser("check-config", help="the drift check for a checkout")
    c.add_argument("checkout", type=Path)
    c.add_argument("--repo", help="owner/repo, asked on GitHub whether it's public (for hosted_runs)")
    args = ap.parse_args(argv)

    if args.cmd == "check-config":
        public = GitHub(token(posting=False)).is_public(args.repo) if args.repo else None
        problems = check_config(args.checkout, public)
        for p in problems:
            print(f"ci-local: {p}", file=sys.stderr)
        if not problems:
            print("ci-local: config in step with the workflows")
        return 1 if problems else 0
    repos = load_repos()
    if args.cmd == "run":
        if args.repo not in repos:
            ap.error(f"unknown repo {args.repo}; known: {', '.join(repos)}")
        if not re.fullmatch(r"[0-9a-f]{40}", args.sha):
            ap.error("give the full 40-character commit")
        gh = GitHub(token(posting=True)) if args.post else None
        ok = run_commit(args.repo, repos[args.repo], args.sha, gh, only=args.job)
        if ok is None:
            print(f"ci-local: {args.repo}@{args.sha}: predates local CI ({CONFIG_PATH} isn't here "
                  f"or where it left main); nothing run")
            return 0
        print(f"ci-local: {args.repo}@{args.sha}: {'passed' if ok else 'FAILED'}; "
              f"logs in {ROOT / args.repo / args.sha}")
        return 0 if ok else 1
    serve(repos, GitHub(token(posting=True)), State(), once=args.once)
    return 0


if __name__ == "__main__":
    sys.exit(main())
