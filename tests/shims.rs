//! RAL-120: `rally stalls`, `wake`, `brief` and `digest` read the roster and send through
//! Marshal's shim or Muster's, the first that has one, so Rally keeps working on either daemon
//! while the sessions move. Fake shims stand in for both and log what they were asked.

use std::path::{Path, PathBuf};
use std::process::Command;

use rally::scratch_pg::{self, Database};
use serde_json::{Value, json};

/// A scratch ledger, and a folder for the fakes and their log.
struct Rig {
    db: Database<'static>,
    dir: tempfile::TempDir,
}

/// How a fake shim answers `roster` and `tell`.
#[derive(Clone, Copy)]
enum Fake {
    /// A daemon with the team's session: a roster, and `tell` delivers.
    Has,
    /// A daemon with no sessions: `roster` fails, and `tell` says nothing was sent.
    Empty,
    /// A roster, but `tell` delivers and then exits non-zero.
    DeliversThenFails,
}

impl Rig {
    fn new() -> Self {
        Self {
            db: scratch_pg::shared().database(),
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn log(&self) -> PathBuf {
        self.dir.path().join("asked.log")
    }

    /// What the fakes were asked, one `<name> <subcommand>` per line.
    fn asked(&self) -> Vec<String> {
        std::fs::read_to_string(self.log())
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn clear(&self) {
        let _ = std::fs::remove_file(self.log());
    }

    /// Writes an executable fake named `name` that answers as `fake`, and returns its path.
    fn fake(&self, case: &str, name: &str, fake: Fake) -> String {
        use std::os::unix::fs::PermissionsExt;
        let roster = json!({"read_at": 1, "sessions": [{
            "session_id": "0f0e0d0c-0000-0000-0000-000000000001",
            "nickname": "rally",
            "wake": "waiting",
            "declared": {"project": "rally", "activity": "idle", "current_task": null},
        }]})
        .to_string();
        let (on_roster, on_tell) = match fake {
            Fake::Has => (format!("printf '%s' '{roster}'"), "exit 0".to_owned()),
            Fake::Empty => (
                "echo 'Error: no roster within 10s: the daemon is unreachable or has no sessions' >&2; exit 1"
                    .to_owned(),
                "echo \"Error: no team or session named \\`$2\\` on the roster\" >&2; exit 1".to_owned(),
            ),
            Fake::DeliversThenFails => (
                format!("printf '%s' '{roster}'"),
                "echo 'Error: the connection dropped after sending' >&2; exit 1".to_owned(),
            ),
        };
        // Each case's fakes in a folder of their own, so one case's can't replace another's.
        let folder = self.dir.path().join(case);
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(name);
        let log = self.log();
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"{name} $1\" >> '{}'\ncase \"$1\" in\n  roster) {on_roster} ;;\n  tell) {on_tell} ;;\n  *) exit 2 ;;\nesac\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    /// A path where no shim is, for a machine without one of them.
    fn missing(&self, name: &str) -> String {
        self.dir
            .path()
            .join("not-installed")
            .join(name)
            .display()
            .to_string()
    }

    /// Runs `rally` with the shims listed, failing the test if it fails.
    fn run(&self, shims: &[String], args: &[&str]) -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_rally"))
            .arg("--db")
            .arg(&self.db.url)
            .arg("--head-file")
            .arg(self.dir.path().join("ledger.head"))
            .args(["--credential", "a1", "--as", "tester"])
            .args(args)
            .env("RALLY_ROSTER_FROM", shims.join(","))
            .env("PATH", self.path_with_gh())
            .env("RALLY_CONFIG", self.dir.path().join("client.json"))
            .env_remove("RALLY_DB")
            .env_remove("RALLY_HEAD_FILE")
            .env_remove("RALLY_CREDENTIAL")
            .env_remove("RALLY_NAME")
            .env_remove("RALLY_TEAM")
            .env_remove("RALLY_PROJECTS")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// PATH with a fake `gh` first, serving a roadmap with RAL-1 on it, for `rally wake`.
    fn path_with_gh(&self) -> String {
        use std::os::unix::fs::PermissionsExt;
        let bin = self.dir.path().join("bin");
        if !bin.exists() {
            std::fs::create_dir(&bin).unwrap();
            let gh = "#!/bin/sh\ncase \"$*\" in\n  *commits/HEAD*) echo 0123456789abcdef0123456789abcdef01234567 ;;\n  *contents*) printf '%s\\n' '# Roadmap' '' '- [ ] **RAL-1: Something to do.** Done when: done.' ;;\n  *) echo \"unexpected: $*\" >&2; exit 1 ;;\nesac\n";
            std::fs::write(bin.join("gh"), gh).unwrap();
            std::fs::set_permissions(bin.join("gh"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }

    fn write(&self, args: &[&str]) {
        self.run(&[], args);
    }

    fn projects(&self) -> PathBuf {
        let path = self.dir.path().join("projects.json");
        std::fs::write(
            &path,
            json!([{"name": "rally", "prefix": "RAL", "repo": "Castor-o7/rally",
                    "path": "docs/roadmap.md", "tick_meaning": "done",
                    "team": "rally", "sessions": ["rally"]}])
            .to_string(),
        )
        .unwrap();
        path
    }
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// The three ways a machine can be during the move: Marshal has the sessions, only Muster is
/// installed, or both are and Marshal's daemon has none yet. The command each is read or sent
/// through.
fn cases(rig: &Rig) -> Vec<(&'static str, Vec<String>, &'static str)> {
    vec![
        (
            "only marshal-shim",
            vec![
                rig.fake("a", "marshal-shim", Fake::Has),
                rig.missing("muster-shim"),
            ],
            "marshal-shim",
        ),
        (
            "only muster-shim",
            vec![
                rig.missing("marshal-shim"),
                rig.fake("b", "muster-shim", Fake::Has),
            ],
            "muster-shim",
        ),
        (
            "both, Marshal empty",
            vec![
                rig.fake("c", "marshal-shim", Fake::Empty),
                rig.fake("c", "muster-shim", Fake::Has),
            ],
            "muster-shim",
        ),
    ]
}

fn ends_with(value: &Value, name: &str) -> bool {
    value.as_str().is_some_and(|s| s.ends_with(name))
}

#[test]
fn stalls_reads_the_roster_through_whichever_shim_has_one() {
    let rig = Rig::new();
    rig.write(&["ready", "RAL-1"]);
    for (case, shims, source) in cases(&rig) {
        rig.clear();
        let report = rig.run(&shims, &["stalls"]);
        assert_eq!(report["roster"]["available"], true, "{case}: {report}");
        assert!(
            ends_with(&report["roster"]["read_with"], source),
            "{case}: {report}"
        );
        assert!(
            rig.asked().contains(&format!("{source} roster")),
            "{case}: {:?}",
            rig.asked()
        );
    }
    // Neither has a roster: said, and only declared blockers are shown.
    let none = vec![
        rig.fake("d", "marshal-shim", Fake::Empty),
        rig.missing("muster-shim"),
    ];
    let report = rig.run(&none, &["stalls"]);
    assert_eq!(report["roster"]["available"], false, "{report}");
    assert_eq!(
        report["roster"]["passed_over"].as_array().unwrap().len(),
        2,
        "{report}"
    );
}

#[test]
fn brief_reads_the_roster_through_whichever_shim_has_one() {
    let rig = Rig::new();
    rig.write(&["ready", "RAL-1"]);
    for (case, shims, source) in cases(&rig) {
        rig.clear();
        let report = rig.run(&shims, &["brief"]);
        assert!(
            ends_with(&report["roster_read"]["read_with"], source),
            "{case}: {report}"
        );
        assert!(
            rig.asked().contains(&format!("{source} roster")),
            "{case}: {:?}",
            rig.asked()
        );
    }
}

#[test]
fn wake_reads_and_tells_through_whichever_shim_has_the_team() {
    let rig = Rig::new();
    rig.write(&["ready", "RAL-1"]);
    let projects = rig.projects();
    for (case, shims, source) in cases(&rig) {
        rig.clear();
        let state = rig
            .dir
            .path()
            .join(format!("wake-{source}-{}.json", case.len()));
        let report = rig.run(
            &shims,
            &[
                "wake",
                "--projects",
                path(&projects),
                "--interval",
                "1m",
                "--state-file",
                path(&state),
            ],
        );
        assert!(
            ends_with(&report["roster"]["read_with"], source),
            "{case}: {report}"
        );
        let team = &report["teams"].as_array().unwrap()[0];
        assert!(ends_with(&team["sent_by"], source), "{case}: {report}");
        let asked = rig.asked();
        assert!(
            asked.contains(&format!("{source} tell")),
            "{case}: {asked:?}"
        );
        // Told once, through one daemon.
        assert_eq!(
            asked.iter().filter(|a| a.ends_with(" tell")).count(),
            if case.starts_with("both") { 2 } else { 1 },
            "{case}: {asked:?}"
        );
        assert!(
            !asked.contains(&"muster-shim tell".to_owned()) || source == "muster-shim",
            "{case}: {asked:?}"
        );
    }
}

#[test]
fn a_shim_that_may_have_delivered_isnt_followed_by_a_second_send() {
    let rig = Rig::new();
    rig.write(&["ready", "RAL-1"]);
    let projects = rig.projects();
    let shims = vec![
        rig.fake("e", "marshal-shim", Fake::DeliversThenFails),
        rig.fake("e", "muster-shim", Fake::Has),
    ];
    let state = rig.dir.path().join("wake.json");
    let report = rig.run(
        &shims,
        &[
            "wake",
            "--projects",
            path(&projects),
            "--interval",
            "1m",
            "--state-file",
            path(&state),
        ],
    );
    let team = &report["teams"].as_array().unwrap()[0];
    let error = team["error"].as_str().unwrap_or_default();
    assert!(error.contains("marshal-shim tell failed"), "{report}");
    assert!(error.contains("the connection dropped"), "{report}");
    assert!(team["sent_by"].is_null(), "{report}");
    let asked = rig.asked();
    assert!(asked.contains(&"marshal-shim tell".to_owned()), "{asked:?}");
    assert!(
        !asked.contains(&"muster-shim tell".to_owned()),
        "a second copy was sent: {asked:?}"
    );
}

#[test]
fn digest_tells_the_watchman_through_whichever_shim_has_it() {
    let rig = Rig::new();
    for (case, shims, source) in cases(&rig) {
        rig.clear();
        // A new record, older than the quiet period, after the last digest.
        let state = rig
            .dir
            .path()
            .join(format!("digest-{source}-{}.json", case.len()));
        std::fs::write(&state, r#"{"w": {"head": 0, "seen_to": 0}}"#).unwrap();
        rig.write(&["ready", &format!("RAL-{}", 10 + case.len())]);
        let pass = || {
            rig.run(
                &shims,
                &[
                    "digest",
                    "--quiet",
                    "1s",
                    "--max",
                    "1s",
                    "--watchman",
                    "w",
                    "--state-file",
                    path(&state),
                ],
            )
        };
        // The first pass sees the record and batches; one after the quiet period sends it.
        let first = pass();
        assert_eq!(
            first["watchmen"][0]["decision"], "batching",
            "{case}: {first}"
        );
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let report = pass();
        let row = &report["watchmen"][0];
        assert_eq!(row["decision"], "sent", "{case}: {report}");
        assert!(ends_with(&row["sent_by"], source), "{case}: {report}");
        assert!(
            rig.asked().contains(&format!("{source} tell")),
            "{case}: {:?}",
            rig.asked()
        );
    }
}
