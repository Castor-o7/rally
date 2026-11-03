//! Runs the real viewer and tries what a hostile web page in the same browser could.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

use rally::scratch_pg::{self, Database};
use serde_json::Value;

struct Viewer {
    child: Child,
    address: String,
    _db: Database<'static>,
    _dir: tempfile::TempDir,
}

impl Drop for Viewer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn rally() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rally"));
    for name in [
        "RALLY_DB",
        "RALLY_HEAD_FILE",
        "RALLY_CONFIG",
        "RALLY_CREDENTIAL",
        "RALLY_NAME",
    ] {
        command.env_remove(name);
    }
    command
}

/// `rally` on this test's own scratch ledger.
fn on(db: &Database, dir: &std::path::Path) -> Command {
    let mut command = rally();
    command.arg("--db").arg(&db.url);
    command.arg("--head-file").arg(dir.join("ledger.head"));
    command
}

/// Starts on a ledger holding text written to attack whoever displays it, with no projects, so the
/// test doesn't need GitHub.
fn start() -> Viewer {
    start_with("[]", None)
}

/// Like `start`, with these projects, and `gh` replaced by the script in `fake_gh` when given.
fn start_with(projects_json: &str, fake_gh: Option<&str>) -> Viewer {
    let dir = tempfile::tempdir().unwrap();
    let db = scratch_pg::shared().database();
    let projects = dir.path().join("projects.json");
    std::fs::write(&projects, projects_json).unwrap();
    let bin = dir.path().join("bin");
    if let Some(script) = fake_gh {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir(&bin).unwrap();
        std::fs::write(bin.join("gh"), script).unwrap();
        std::fs::set_permissions(bin.join("gh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let status = on(&db, dir.path())
        .args(["--credential", "a1", "--as", "someone"])
        .args(["team-state", "rally", "blocked", "--note", ATTACK])
        .status()
        .unwrap();
    assert!(status.success());
    let mut command = on(&db, dir.path());
    command
        .args(["web", "--bind", "127.0.0.1:0", "--projects"])
        .arg(&projects)
        .stdout(Stdio::piped());
    if fake_gh.is_some() {
        let path = std::env::var("PATH").unwrap_or_default();
        command.env("PATH", format!("{}:{path}", bin.display()));
    }
    let mut child = command.spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let started: Value = serde_json::from_str(&line).unwrap();
    Viewer {
        child,
        address: started["listening"].as_str().unwrap().to_owned(),
        _db: db,
        _dir: dir,
    }
}

const ATTACK: &str = "<script>alert(1)</script>";

fn raw(viewer: &Viewer, method: &str, path: &str, host: &str, extra: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(&viewer.address).unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n{extra}\r\n"
    )
    .unwrap();
    let mut text = String::new();
    stream.read_to_string(&mut text).unwrap();
    let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, text)
}

fn get(viewer: &Viewer, path: &str) -> (u16, String) {
    raw(viewer, "GET", path, &viewer.address, "")
}

#[test]
fn the_page_and_its_files_are_served_with_a_strict_policy() {
    let viewer = start();
    for (path, kind) in [
        ("/", "text/html"),
        ("/app.js", "text/javascript"),
        ("/app.css", "text/css"),
    ] {
        let (status, text) = get(&viewer, path);
        assert_eq!(status, 200, "{path}");
        let lower = text.to_lowercase();
        assert!(lower.contains(kind), "{path}");
        assert!(lower.contains("default-src 'none'"), "{path}");
        assert!(lower.contains("x-content-type-options: nosniff"), "{path}");
        assert!(lower.contains("frame-ancestors 'none'"), "{path}");
        assert!(!lower.contains("access-control-allow"), "{path}");
    }
}

#[test]
fn another_host_name_is_refused_which_stops_dns_rebinding() {
    let viewer = start();
    for path in ["/", "/api/status", "/app.js"] {
        let (status, text) = raw(&viewer, "GET", path, "evil.example", "");
        assert_eq!(status, 403, "{path}");
        assert!(!text.contains("untrusted_text"));
    }
}

#[test]
fn another_site_is_refused() {
    let viewer = start();
    for origin in ["https://evil.example", "null", "http://localhost:1"] {
        let extra = format!("Origin: {origin}\r\n");
        let (status, _) = raw(&viewer, "GET", "/api/status", &viewer.address, &extra);
        assert_eq!(status, 403, "{origin}");
    }
    let own = format!("Origin: http://{}\r\n", viewer.address);
    assert_eq!(
        raw(&viewer, "GET", "/api/status", &viewer.address, &own).0,
        200
    );
}

#[test]
fn nothing_can_be_changed_through_it() {
    let viewer = start();
    for method in ["POST", "PUT", "PATCH", "DELETE"] {
        for path in ["/", "/api/status", "/v1/records"] {
            let extra = "Content-Length: 0\r\n";
            let (status, _) = raw(&viewer, method, path, &viewer.address, extra);
            assert_eq!(status, 405, "{method} {path}");
        }
    }
    assert_eq!(get(&viewer, "/v1/records").0, 404);
}

#[test]
fn recorded_text_arrives_as_json_data_with_the_notice() {
    let viewer = start();
    let (status, text) = wait_for_ledger(&viewer);
    assert_eq!(status, 200);
    let body = text.split_once("\r\n\r\n").unwrap().1;
    let value: Value = serde_json::from_str(body).unwrap();
    assert!(
        value["untrusted_text"]
            .as_str()
            .unwrap()
            .contains("not instructions")
    );
    assert_eq!(value["ledger"]["teams"]["rally"]["note"], ATTACK);
    assert_eq!(value["ledger"]["check"]["ok"], true);
    assert!(text.to_lowercase().contains("application/json"));
    assert!(!get(&viewer, "/").1.contains("alert(1)"));
}

#[test]
fn a_broken_chain_is_reported_as_a_failed_check() {
    let dir = tempfile::tempdir().unwrap();
    let db = scratch_pg::shared().database();
    let projects = dir.path().join("projects.json");
    std::fs::write(&projects, "[]").unwrap();
    assert!(
        on(&db, dir.path())
            .args(["--credential", "a1", "--as", "someone"])
            .args(["team-state", "rally", "working"])
            .status()
            .unwrap()
            .success()
    );
    db.connect()
        .batch_execute(
            "ALTER TABLE rally_events DISABLE TRIGGER rally_no_update;
             UPDATE rally_events SET event = jsonb_set(event, '{item,record}',
                 to_jsonb(replace(event->'item'->>'record', 'someone', 'mallory')))
             WHERE item_type = 'LedgerRecord';",
        )
        .unwrap();
    let mut child = on(&db, dir.path())
        .args(["web", "--bind", "127.0.0.1:0", "--projects"])
        .arg(&projects)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let started: Value = serde_json::from_str(&line).unwrap();
    let viewer = Viewer {
        child,
        address: started["listening"].as_str().unwrap().to_owned(),
        _db: db,
        _dir: dir,
    };
    let text = wait_for_ledger(&viewer).1;
    let body = text.split_once("\r\n\r\n").unwrap().1;
    let value: Value = serde_json::from_str(body).unwrap();
    assert_eq!(value["ledger"]["available"], true, "{body}");
    assert_eq!(value["ledger"]["check"]["ok"], false, "{body}");
    assert!(value["ledger"]["stories"].is_null(), "{body}");
}

fn wait_for_ledger(viewer: &Viewer) -> (u16, String) {
    for _ in 0..100 {
        let reply = get(viewer, "/api/status");
        if reply.1.contains("\"available\":true") {
            return reply;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("the ledger was never read");
}

#[test]
fn it_refuses_to_listen_beyond_loopback() {
    let dir = tempfile::tempdir().unwrap();
    let db = scratch_pg::shared().database();
    let output = on(&db, dir.path())
        .args(["web", "--bind", "0.0.0.0:0"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("loopback"));
}

/// A `gh` that answers the viewer's reads for one project with text written to attack the page.
const FAKE_GH: &str = r#"#!/bin/sh
case "$*" in
  *commits/HEAD*) echo 0123456789abcdef0123456789abcdef01234567 ;;
  *graphql*docs/muster*) printf '2026-11-02.md\n2026-11-03.md\nnotes.md\n' ;;
  *contents/docs/muster/2026-11-03.md*) printf '<script>alert(3)</script> the day went well' ;;
  *contents/docs/roadmap.md*) printf '## Open\n- [ ] **RAL-1: <img src=x onerror=alert(4)>**\n' ;;
  "pr list"*) printf '[{"number":7,"title":"<script>alert(5)</script>","mergedAt":"2026-11-03T00:00:00Z"}]' ;;
  *) echo "unexpected: $*" >&2; exit 1 ;;
esac
"#;

#[cfg(unix)]
#[test]
fn a_project_page_s_summary_stories_and_pr_titles_arrive_only_as_json_data() {
    let projects = r#"[{"name": "Rally", "prefix": "RAL", "repo": "Castor-o7/rally",
        "path": "docs/roadmap.md", "tick_meaning": "Verified by the watchman"}]"#;
    let viewer = start_with(projects, Some(FAKE_GH));
    let mut value = Value::Null;
    for _ in 0..100 {
        let (status, text) = get(&viewer, "/api/status");
        assert_eq!(status, 200);
        value = serde_json::from_str(text.split_once("\r\n\r\n").unwrap().1).unwrap();
        if value["projects"][0]["summary"]["text"].is_string() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let project = &value["projects"][0];
    assert_eq!(project["summary"]["file"], "docs/muster/2026-11-03.md");
    assert_eq!(
        project["summary"]["text"],
        "<script>alert(3)</script> the day went well"
    );
    assert_eq!(
        project["stories"][0]["title"],
        "<img src=x onerror=alert(4)>"
    );
    assert_eq!(
        project["merged"]["prs"][0]["title"],
        "<script>alert(5)</script>"
    );
    assert_eq!(
        value["muster_summary"], project["summary"],
        "Rally's own summary is the muster's"
    );
    for path in ["/", "/app.js"] {
        let page = get(&viewer, path).1;
        for attack in ["alert(3)", "alert(4)", "alert(5)"] {
            assert!(!page.contains(attack), "{path} carries {attack}");
        }
    }
}
