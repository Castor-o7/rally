//! RAL-120, RAL-126: `rally stalls`, `wake`, `brief` and `digest` read every shim's roster and
//! show their union, and send through the first shim that knows the recipient, so Rally keeps
//! working while sessions run on both daemons. Fake shims stand in for both and log what they
//! were asked.

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
    /// A daemon with no sessions: an empty roster, as Marshal's live one gives, and `tell` says
    /// nothing was sent.
    Empty,
    /// A daemon that can't be reached: `roster` fails, and `tell` says nothing was sent.
    Down,
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
        // Each fake's session is its own, so two populated rosters have two sessions.
        let roster = json!({"read_at": 1, "sessions": [{
            "session_id": format!("{case}-{name}"),
            "nickname": "rally",
            "wake": "waiting",
            "declared": {"project": "rally", "activity": "idle", "current_task": null},
        }]})
        .to_string();
        let (on_roster, on_tell) = match fake {
            Fake::Has => (format!("printf '%s' '{roster}'"), "exit 0".to_owned()),
            Fake::Empty => (
                "printf '%s' '{\"read_at\": 1, \"sessions\": []}'".to_owned(),
                "echo \"Error: no team or session named \\`$2\\` on the roster\" >&2; exit 1".to_owned(),
            ),
            Fake::Down => (
                "echo 'Error: no roster within 10s: the daemon is unreachable or has no sessions' >&2; exit 1"
                    .to_owned(),
                "echo \"Error: couldn't register with the daemon within 10s; nothing was sent\" >&2; exit 1"
                    .to_owned(),
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

/// A case: its name, the shims listed, the rosters it should read, the rosters its sessions come
/// from, and the shim a message should go through.
struct Case {
    name: &'static str,
    shims: Vec<String>,
    read: Vec<&'static str>,
    rows_from: Vec<&'static str>,
    told_by: &'static str,
}

/// The ways a machine can be during the move: one daemon installed, Marshal's empty beside Muster's,
/// both with sessions, and Marshal's down.
fn cases(rig: &Rig) -> Vec<Case> {
    vec![
        Case {
            name: "only marshal-shim",
            shims: vec![
                rig.fake("a", "marshal-shim", Fake::Has),
                rig.missing("muster-shim"),
            ],
            read: vec!["marshal-shim"],
            rows_from: vec!["marshal-shim"],
            told_by: "marshal-shim",
        },
        Case {
            name: "only muster-shim",
            shims: vec![
                rig.missing("marshal-shim"),
                rig.fake("b", "muster-shim", Fake::Has),
            ],
            read: vec!["muster-shim"],
            rows_from: vec!["muster-shim"],
            told_by: "muster-shim",
        },
        Case {
            name: "both, Marshal empty",
            shims: vec![
                rig.fake("c", "marshal-shim", Fake::Empty),
                rig.fake("c", "muster-shim", Fake::Has),
            ],
            read: vec!["marshal-shim", "muster-shim"],
            rows_from: vec!["muster-shim"],
            told_by: "muster-shim",
        },
        Case {
            name: "both populated",
            shims: vec![
                rig.fake("d", "marshal-shim", Fake::Has),
                rig.fake("d", "muster-shim", Fake::Has),
            ],
            read: vec!["marshal-shim", "muster-shim"],
            rows_from: vec!["marshal-shim", "muster-shim"],
            told_by: "marshal-shim",
        },
        Case {
            name: "Marshal down",
            shims: vec![
                rig.fake("f", "marshal-shim", Fake::Down),
                rig.fake("f", "muster-shim", Fake::Has),
            ],
            read: vec!["muster-shim"],
            rows_from: vec!["muster-shim"],
            told_by: "muster-shim",
        },
    ]
}

/// The last part of each path in a list, so `/tmp/x/a/marshal-shim` reads `marshal-shim`.
fn names(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| {
            let v = v.as_str().or_else(|| v["command"].as_str()).unwrap_or("");
            v.rsplit('/').next().unwrap_or("").to_owned()
        })
        .collect()
}

fn ends_with(value: &Value, name: &str) -> bool {
    value.as_str().is_some_and(|s| s.ends_with(name))
}

#[test]
fn stalls_shows_every_rosters_sessions_naming_each_source() {
    let rig = Rig::new();
    rig.write(&["ready", "RAL-1"]);
    let projects = rig.projects();
    for case in cases(&rig) {
        rig.clear();
        let report = rig.run(&case.shims, &["stalls", "--projects", path(&projects)]);
        let name = case.name;
        assert_eq!(report["roster"]["available"], true, "{name}: {report}");
        assert_eq!(
            names(&report["roster"]["read_with"]),
            case.read,
            "{name}: {report}"
        );
        // Every roster was asked, whichever answered.
        for shim in ["marshal-shim", "muster-shim"] {
            let installed = case
                .shims
                .iter()
                .any(|s| s.ends_with(shim) && !s.contains("not-installed"));
            assert_eq!(
                rig.asked().contains(&format!("{shim} roster")),
                installed,
                "{name}: {:?}",
                rig.asked()
            );
        }
        // A source that failed is listed as passed over.
        let failed = names(&report["roster"]["passed_over"]);
        assert_eq!(
            failed.len(),
            case.shims.len() - case.read.len(),
            "{name}: {report}"
        );
        // The team's rows name the rosters they came from.
        let text = report.to_string();
        for source in &case.rows_from {
            assert!(
                text.contains(&format!("{source}\"")),
                "{name}: {source} in {report}"
            );
        }
    }
    // Neither has a roster: said, and only declared blockers are shown.
    let none = vec![
        rig.fake("g", "marshal-shim", Fake::Down),
        rig.missing("muster-shim"),
    ];
    let report = rig.run(&none, &["stalls"]);
    assert_eq!(report["roster"]["available"], false, "{report}");
    assert_eq!(names(&report["roster"]["passed_over"]).len(), 2, "{report}");
}

#[test]
fn brief_reads_every_roster() {
    let rig = Rig::new();
    rig.write(&["ready", "RAL-1"]);
    for case in cases(&rig) {
        rig.clear();
        let report = rig.run(&case.shims, &["brief"]);
        let name = case.name;
        assert_eq!(
            names(&report["roster_read"]["read_with"]),
            case.read,
            "{name}: {report}"
        );
        assert_eq!(
            names(&report["roster_read"]["passed_over"]).len(),
            case.shims.len() - case.read.len(),
            "{name}: {report}"
        );
    }
}

#[test]
fn wake_sees_every_rosters_sessions_and_tells_through_the_shim_that_has_the_team() {
    let rig = Rig::new();
    rig.write(&["ready", "RAL-1"]);
    let projects = rig.projects();
    for (n, case) in cases(&rig).into_iter().enumerate() {
        rig.clear();
        let name = case.name;
        let state = rig.dir.path().join(format!("wake-{n}.json"));
        let report = rig.run(
            &case.shims,
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
        assert_eq!(
            names(&report["roster"]["read_with"]),
            case.read,
            "{name}: {report}"
        );
        let team = &report["teams"].as_array().unwrap()[0];
        assert!(
            ends_with(&team["sent_by"], case.told_by),
            "{name}: {report}"
        );
        let asked = rig.asked();
        let tells: Vec<&String> = asked.iter().filter(|a| a.ends_with(" tell")).collect();
        // Sent once: through the shim that has the team, after any that said it hadn't.
        assert_eq!(
            tells.last().map(|t| t.as_str()),
            Some(format!("{} tell", case.told_by).as_str()),
            "{name}: {asked:?}"
        );
        assert!(tells.len() <= 2, "{name}: {asked:?}");
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
    for (n, case) in cases(&rig).into_iter().enumerate() {
        rig.clear();
        let name = case.name;
        // A new record after the last digest.
        let state = rig.dir.path().join(format!("digest-{n}.json"));
        std::fs::write(&state, r#"{"w": {"head": 0, "seen_to": 0}}"#).unwrap();
        rig.write(&["ready", &format!("RAL-{}", 10 + n)]);
        let pass = || {
            rig.run(
                &case.shims,
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
            "{name}: {first}"
        );
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let report = pass();
        let row = &report["watchmen"][0];
        assert_eq!(row["decision"], "sent", "{name}: {report}");
        assert!(ends_with(&row["sent_by"], case.told_by), "{name}: {report}");
        assert!(
            rig.asked().contains(&format!("{} tell", case.told_by)),
            "{name}: {:?}",
            rig.asked()
        );
    }
}
