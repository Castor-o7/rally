//! Runs the real viewer and tries what a hostile web page in the same browser could.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};

use rally::scratch_pg::{self, Database};
use serde_json::{Value, json};

struct Viewer {
    child: Child,
    address: String,
    /// The long-lived viewer key, from its private file.
    key: String,
    key_file: std::path::PathBuf,
    /// A signed-in session, as the operator's browser holds after `rally web --login`.
    session: String,
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

/// `rally` on this test's own scratch ledger, with its pairing in the test's folder, so nothing
/// beside the operator's own pairing (such as `~/.config/rally/projects.json`) is read.
fn on(db: &Database, dir: &std::path::Path) -> Command {
    let mut command = rally();
    command.arg("--db").arg(&db.url);
    command.arg("--head-file").arg(dir.join("ledger.head"));
    command.env("RALLY_CONFIG", dir.join("client.json"));
    command
}

/// Starts on a ledger holding text written to attack whoever displays it, with no projects, so the
/// test doesn't need GitHub.
fn start() -> Viewer {
    start_with("[]", None)
}

/// Like `start`, with these projects, and `gh` replaced by the script in `fake_gh` when given.
fn start_with(projects_json: &str, fake_gh: Option<&str>) -> Viewer {
    start_with_codes(projects_json, fake_gh, 60)
}

/// Like `start_with`, with codes that last `code_seconds`.
fn start_with_codes(projects_json: &str, fake_gh: Option<&str>, code_seconds: u64) -> Viewer {
    start_after(projects_json, fake_gh, code_seconds, |_, _| {})
}

/// Like `start_with_codes`, after `setup` has written to the ledger.
fn start_after(
    projects_json: &str,
    fake_gh: Option<&str>,
    code_seconds: u64,
    setup: impl FnOnce(&Database, &std::path::Path),
) -> Viewer {
    let dir = tempfile::tempdir().unwrap();
    let db = scratch_pg::shared().database();
    setup(&db, dir.path());
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
    let key_file = dir.path().join("web-key");
    command
        .args(["web", "--bind", "127.0.0.1:0", "--projects"])
        .arg(&projects)
        .arg("--key-file")
        .arg(&key_file)
        .args(["--code-seconds", &code_seconds.to_string()])
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
    // The key is in the private file, never in what was printed.
    let key = std::fs::read_to_string(&key_file).unwrap();
    assert!(!line.contains(&key), "the key was printed");
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key_file).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "the key file isn't private");
    }
    let mut viewer = Viewer {
        child,
        address: started["listening"].as_str().unwrap().to_owned(),
        key,
        key_file,
        session: String::new(),
        _db: db,
        _dir: dir,
    };
    viewer.session = sign_in(&viewer);
    viewer
}

/// `rally web --login`'s address to open, with its one-time code.
fn login(viewer: &Viewer) -> String {
    let out = rally()
        .args(["web", "--bind", &viewer.address, "--login", "--key-file"])
        .arg(&viewer.key_file)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8(out.stdout).unwrap();
    assert!(!printed.contains(&viewer.key), "the key was printed");
    let value: Value = serde_json::from_str(&printed).unwrap();
    let open = value["open"].as_str().unwrap();
    open.split_once(&viewer.address).unwrap().1.to_owned()
}

/// Signs in as a browser would: opens the code's address, presses "Sign in", and keeps the
/// session cookie.
fn sign_in(viewer: &Viewer) -> String {
    let path = login(viewer);
    let (status, text) = bare(viewer, "GET", &path, &viewer.address, "");
    assert_eq!(status, 200, "{text}");
    let (status, text) = press_sign_in(viewer, &path, "");
    assert_eq!(status, 303, "{text}");
    session_from(&text)
}

/// The code in a `/?code=` path.
fn code_of(path: &str) -> &str {
    path.split_once("code=").unwrap().1
}

/// The sign-in page's button: a same-origin form POST of the path's code, with any extra headers.
fn press_sign_in(viewer: &Viewer, path: &str, extra: &str) -> (u16, String) {
    let body = format!("code={}", code_of(path));
    let headers = format!(
        "Origin: http://{}\r\nSec-Fetch-Site: same-origin\r\n\
         Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n{extra}",
        viewer.address,
        body.len()
    );
    bare_with_body(viewer, "POST", "/", &viewer.address, &headers, &body)
}

fn session_from(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| line.to_lowercase().starts_with("set-cookie:"))
        .expect("a cookie");
    let value = line.split_once("rally_view=").unwrap().1;
    value.split(';').next().unwrap().to_owned()
}

const ATTACK: &str = "<script>alert(1)</script>";

/// A request carrying the viewer's cookie, as the operator's browser would once it has the key.
fn raw(viewer: &Viewer, method: &str, path: &str, host: &str, extra: &str) -> (u16, String) {
    let extra = format!("Cookie: rally_view={}\r\n{extra}", viewer.session);
    bare(viewer, method, path, host, &extra)
}

/// A request with only the headers given.
fn bare(viewer: &Viewer, method: &str, path: &str, host: &str, extra: &str) -> (u16, String) {
    bare_with_body(viewer, method, path, host, extra, "")
}

/// Like `bare`, with a body after the headers.
fn bare_with_body(
    viewer: &Viewer,
    method: &str,
    path: &str,
    host: &str,
    extra: &str,
    body: &str,
) -> (u16, String) {
    let mut stream = TcpStream::connect(&viewer.address).unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n{extra}\r\n{body}"
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
    let key_file = dir.path().join("web-key");
    let mut child = on(&db, dir.path())
        .args(["web", "--bind", "127.0.0.1:0", "--projects"])
        .arg(&projects)
        .arg("--key-file")
        .arg(&key_file)
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
        key: std::fs::read_to_string(&key_file).unwrap(),
        key_file: key_file.clone(),
        session: String::new(),
        _db: db,
        _dir: dir,
    };
    let mut viewer = viewer;
    viewer.session = sign_in(&viewer);
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
    let key_file = dir.path().join("web-key");
    let output = on(&db, dir.path())
        .args(["web", "--bind", "0.0.0.0:0", "--key-file"])
        .arg(&key_file)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("loopback"));
    // Refused before any key is made.
    assert!(!key_file.exists());
}

#[test]
fn a_key_file_others_can_read_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let db = scratch_pg::shared().database();
    let key_file = dir.path().join("web-key");
    std::fs::write(&key_file, "a".repeat(64)).unwrap();
    std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let output = on(&db, dir.path())
        .args(["web", "--bind", "127.0.0.1:0", "--key-file"])
        .arg(&key_file)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("read by others"));
}

/// A `gh` that answers the viewer's reads for one project with text written to attack the page.
const FAKE_GH: &str = r#"#!/bin/sh
case "$*" in
  *commits/HEAD*) echo 0123456789abcdef0123456789abcdef01234567 ;;
  *graphql*blame*) printf '[{"startingLine":1,"endingLine":2,"commit":{"committedDate":"2026-10-29T08:00:00Z","message":"<script>alert(6)</script>"}},{"startingLine":3,"endingLine":3,"commit":{"committedDate":"2026-10-30T12:00:00Z"}},{"startingLine":4,"endingLine":4,"commit":{"committedDate":"<img src=x onerror=alert(7)>"}}]' ;;
  *graphql*docs/muster/watchman*) printf '2026-11-03.md\n' ;;
  *graphql*docs/muster*) printf '2026-11-02.md\n2026-11-03.md\nnotes.md\n' ;;
  *contents/docs/muster/watchman/2026-11-03.md*) printf '<script>alert(8)</script> the whole muster' ;;
  *contents/docs/muster/2026-11-03.md*) printf '<script>alert(3)</script> the day went well' ;;
  *contents/docs/roadmap.md*) printf '## Open\n- [ ] **RAL-1: <img src=x onerror=alert(4)>**\n- [x] **RAL-2: Done.**\n- [x] **RAL-3: Dated by a hostile blame.**\n' ;;
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
    // RAL-44: a ticked line gets the date of the commit that last changed it, as data only.
    assert_eq!(project["stories"][1]["last_changed"], "2026-10-30");
    assert!(
        project["stories"][2].get("last_changed").is_none(),
        "a date that isn't one is dropped"
    );
    assert!(
        project["stories"][0].get("last_changed").is_none(),
        "only ticked lines are dated"
    );
    assert!(
        !value.to_string().contains("alert(6)"),
        "commit messages aren't kept"
    );
    // The overview's writeup is the watchman's, from its own folder, never Rally's day summary.
    assert_eq!(
        value["muster_summary"]["file"],
        "docs/muster/watchman/2026-11-03.md"
    );
    assert_eq!(
        value["muster_summary"]["text"],
        "<script>alert(8)</script> the whole muster"
    );
    for path in ["/", "/app.js"] {
        let page = get(&viewer, path).1;
        for attack in ["alert(3)", "alert(4)", "alert(5)", "alert(6)", "alert(7)"] {
            assert!(!page.contains(attack), "{path} carries {attack}");
        }
    }
}

#[test]
fn a_viewer_that_isnt_signed_in_is_refused() {
    let viewer = start();
    let host = viewer.address.clone();
    for path in ["/", "/api/status", "/app.js"] {
        let (status, text) = bare(&viewer, "GET", path, &host, "");
        assert_eq!(status, 401, "{path}");
        assert!(!text.contains("untrusted_text"), "{path}");
        let lower = text.to_lowercase();
        assert!(lower.contains("referrer-policy: no-referrer"), "{path}");
        assert!(lower.contains("cache-control: no-store"), "{path}");
        // Neither a made-up session nor the long-lived key itself is a session.
        for cookie in ["0".repeat(64), viewer.key.clone()] {
            let extra = format!("Cookie: rally_view={cookie}\r\n");
            assert_eq!(bare(&viewer, "GET", path, &host, &extra).0, 401, "{path}");
        }
    }
    // The key in the address is no longer a way in.
    assert_eq!(
        bare(&viewer, "GET", &format!("/?key={}", viewer.key), &host, "").0,
        401
    );
}

#[test]
fn a_one_time_code_becomes_a_strict_session_cookie_once() {
    let viewer = start();
    let host = viewer.address.clone();
    let path = login(&viewer);
    let (status, text) = press_sign_in(&viewer, &path, "");
    assert_eq!(status, 303);
    let lower = text.to_lowercase();
    assert!(lower.contains("location: /\r\n"), "{text}");
    let cookie = lower
        .lines()
        .find(|line| line.starts_with("set-cookie:"))
        .unwrap()
        .to_owned();
    assert!(
        cookie.contains("httponly") && cookie.contains("samesite=strict"),
        "{cookie}"
    );
    // The cookie is a session, not the long-lived key, and no response carries the key.
    assert!(!text.contains(&viewer.key));
    let session = session_from(&text);
    assert_ne!(session, viewer.key.trim());
    let extra = format!("Cookie: rally_view={session}\r\n");
    let (status, page) = bare(&viewer, "GET", "/api/status", &host, &extra);
    assert_eq!(status, 200);
    assert!(!page.contains(&viewer.key));
    // A code works once.
    assert_eq!(press_sign_in(&viewer, &path, "").0, 401);
}

#[test]
fn an_expired_code_is_refused() {
    let viewer = start_with_codes("[]", None, 1);
    let path = login(&viewer);
    std::thread::sleep(std::time::Duration::from_millis(1_200));
    assert_eq!(press_sign_in(&viewer, &path, "").0, 401);
}

/// RAL-73: Chrome fetched the code's address while it was pasted, and that spent the code before
/// the operator's visit. Opening the address, prefetched or not, now only shows the button.
#[test]
fn opening_a_codes_address_spends_nothing_and_only_the_button_signs_in() {
    let viewer = start();
    let path = login(&viewer);
    for extra in [
        "Sec-Purpose: prefetch\r\nSec-Fetch-Site: none\r\n",
        "Purpose: prefetch\r\n",
        "Sec-Purpose: prefetch;prerender\r\nSec-Fetch-Site: none\r\n",
        "Sec-Fetch-Site: none\r\n",
        "",
    ] {
        let (status, text) = bare(&viewer, "GET", &path, &viewer.address, extra);
        assert_eq!(status, 200, "{extra}: {text}");
        let lower = text.to_lowercase();
        assert!(!lower.contains("set-cookie"), "{extra}: {text}");
        assert!(lower.contains("text/html"), "{text}");
        assert!(
            text.contains(r#"<form method="post" action="/">"#),
            "{text}"
        );
        assert!(text.contains(code_of(&path)), "{text}");
        // Its policy lets only its own form post, back to this page.
        assert!(lower.contains("form-action 'self'"), "{text}");
        // And its form's POST carries this page's origin: under no-referrer, Chrome sends
        // `Origin: null`, which is refused, so the button never worked (RAL-83).
        assert!(lower.contains("referrer-policy: same-origin\r\n"), "{text}");
        assert!(
            text.contains(r#"<meta name="referrer" content="same-origin">"#),
            "{text}"
        );
        assert!(!lower.contains("no-referrer"), "{text}");
        assert!(lower.contains("default-src 'none'"), "{text}");
        assert!(lower.contains("frame-ancestors 'none'"), "{text}");
        assert!(!lower.contains("script-src"), "{text}");
    }
    // After all that, the code still works, once.
    let (status, text) = press_sign_in(&viewer, &path, "");
    assert_eq!(status, 303, "{text}");
    assert_eq!(press_sign_in(&viewer, &path, "").0, 401);
    // Every other page still allows no form at all.
    let (_, page) = get(&viewer, "/");
    assert!(page.to_lowercase().contains("form-action 'none'"), "{page}");
}

#[test]
fn the_sign_in_page_shows_nothing_but_a_code() {
    let viewer = start();
    // A request line can't hold a raw `<`, so attacks arrive percent-encoded; none is decoded.
    for offered in [
        "%3Cscript%3Ealert(1)%3C/script%3E".to_owned(),
        format!("{}%22%3E%3Cb%3E", "a".repeat(64)),
        "a".repeat(63),
        "g".repeat(64),
    ] {
        let path = format!("/?code={offered}");
        let (status, text) = bare(&viewer, "GET", &path, &viewer.address, "");
        assert_eq!(status, 401, "{offered}: {text}");
        assert!(
            !text.contains("<form") && !text.contains(&offered),
            "{text}"
        );
    }
}

#[test]
fn another_site_cant_press_the_button() {
    let viewer = start();
    let path = login(&viewer);
    let body = format!("code={}", code_of(&path));
    for from in [
        "Origin: http://evil.example\r\nSec-Fetch-Site: cross-site\r\n",
        // What a browser sends under no-referrer, or from a sandboxed or opaque page: still
        // refused, which is why the sign-in page mustn't be no-referrer (RAL-83).
        "Origin: null\r\n",
        "Origin: null\r\nSec-Fetch-Site: same-origin\r\n",
        "Sec-Fetch-Site: same-site\r\n",
    ] {
        let extra = format!(
            "{from}Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n",
            body.len()
        );
        let (status, text) = bare_with_body(&viewer, "POST", "/", &viewer.address, &extra, &body);
        assert_eq!(status, 403, "{from}: {text}");
        assert!(!text.to_lowercase().contains("set-cookie"), "{text}");
    }
    // Refused before it was looked at, the code still works.
    assert_eq!(press_sign_in(&viewer, &path, "").0, 303);
}

#[test]
fn a_huge_sign_in_body_is_refused() {
    let viewer = start();
    let body = format!("code={}", "a".repeat(4096));
    let extra = format!(
        "Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n",
        body.len()
    );
    let (status, text) = bare_with_body(&viewer, "POST", "/", &viewer.address, &extra, &body);
    assert_eq!(status, 413, "{text}");
}

#[test]
fn only_rally_web_login_can_ask_for_a_code() {
    let viewer = start();
    let host = viewer.address.clone();
    let key = format!(
        "Authorization: Bearer {}\r\nContent-Length: 0\r\n",
        viewer.key
    );
    // A browser always says where a request came from, so a page can't ask for one.
    for from in [
        format!("Origin: http://{host}\r\n"),
        "Sec-Fetch-Site: same-origin\r\n".to_owned(),
    ] {
        let (status, text) = bare(&viewer, "POST", "/api/code", &host, &format!("{key}{from}"));
        assert_eq!(status, 403, "{from}");
        assert!(!text.contains("\"code\""));
    }
    let wrong = format!(
        "Authorization: Bearer {}\r\nContent-Length: 0\r\n",
        "0".repeat(64)
    );
    assert_eq!(bare(&viewer, "POST", "/api/code", &host, &wrong).0, 401);
    assert_eq!(
        bare(&viewer, "POST", "/api/code", &host, "Content-Length: 0\r\n").0,
        401
    );
}

#[test]
fn a_new_key_signs_every_browser_out() {
    let viewer = start();
    assert_eq!(get(&viewer, "/api/status").0, 200);
    let out = rally()
        .args(["web", "--new-key", "--key-file"])
        .arg(&viewer.key_file)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        !String::from_utf8_lossy(&out.stdout)
            .contains(&std::fs::read_to_string(&viewer.key_file).unwrap())
    );
    assert_eq!(get(&viewer, "/api/status").0, 401);
    // The old key can't ask for a code either.
    let old = format!(
        "Authorization: Bearer {}\r\nContent-Length: 0\r\n",
        viewer.key
    );
    assert_eq!(
        bare(&viewer, "POST", "/api/code", &viewer.address, &old).0,
        401
    );
}

/// Stops the viewer and starts it again on the same ledger and key file, as a restart or deploy
/// does, and points `viewer` at the new one.
fn restart(viewer: &mut Viewer) {
    let _ = viewer.child.kill();
    let _ = viewer.child.wait();
    let projects = viewer._dir.path().join("projects.json");
    let mut child = on(&viewer._db, viewer._dir.path())
        .args(["web", "--bind", "127.0.0.1:0", "--projects"])
        .arg(&projects)
        .arg("--key-file")
        .arg(&viewer.key_file)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let started: Value = serde_json::from_str(&line).unwrap();
    viewer.child = child;
    viewer.address = started["listening"].as_str().unwrap().to_owned();
}

/// RAL-80: a restart signs no one out. Sessions are kept beside the key as hashes, in a private
/// file, and `--new-key` still signs everyone out, across a restart too.
#[test]
fn a_restart_keeps_browsers_signed_in_and_a_new_key_still_signs_them_out() {
    use std::os::unix::fs::PermissionsExt;
    let mut viewer = start();
    let second = sign_in(&viewer);
    let kept = viewer.key_file.with_file_name("web-key.sessions");
    let text = std::fs::read_to_string(&kept).unwrap();
    // Neither a session nor the key is in the file, only their hashes.
    for secret in [&viewer.session, &second, &viewer.key] {
        assert!(!text.contains(secret.trim()), "{text}");
    }
    let mode = std::fs::metadata(&kept).unwrap().permissions().mode();
    assert_eq!(mode & 0o077, 0, "the sessions file isn't private");

    restart(&mut viewer);
    assert_eq!(get(&viewer, "/api/status").0, 200);
    let other = format!("Cookie: rally_view={second}\r\n");
    assert_eq!(
        bare(&viewer, "GET", "/api/status", &viewer.address, &other).0,
        200
    );
    // A cookie that was never a session still isn't one.
    let made_up = format!("Cookie: rally_view={}\r\n", "ab".repeat(32));
    assert_eq!(
        bare(&viewer, "GET", "/api/status", &viewer.address, &made_up).0,
        401
    );

    let out = rally()
        .args(["web", "--new-key", "--key-file"])
        .arg(&viewer.key_file)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(get(&viewer, "/api/status").0, 401);
    restart(&mut viewer);
    assert_eq!(get(&viewer, "/api/status").0, 401);
    // A sign-in under the new key holds across a restart.
    viewer.key = std::fs::read_to_string(&viewer.key_file).unwrap();
    viewer.session = sign_in(&viewer);
    restart(&mut viewer);
    assert_eq!(get(&viewer, "/api/status").0, 200);

    // A sessions file that isn't one signs no one in, and the viewer still starts.
    std::fs::write(&kept, "not json").unwrap();
    restart(&mut viewer);
    assert_eq!(get(&viewer, "/api/status").0, 401);
}

/// RAL-80: a kept session older than 14 days signs no one in after a restart, and only the newest
/// 32 sessions are kept.
#[test]
fn old_sessions_end_and_only_the_newest_are_kept() {
    let mut viewer = start();
    let second = sign_in(&viewer);
    let kept = viewer.key_file.with_file_name("web-key.sessions");
    let mut file: Value = serde_json::from_str(&std::fs::read_to_string(&kept).unwrap()).unwrap();
    let rows = file["sessions"].as_array_mut().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row["made_at_ms"].as_u64().is_some()));
    // The first sign-in, 15 days ago.
    let made = rows[0]["made_at_ms"].as_u64().unwrap();
    rows[0]["made_at_ms"] = json!(made - 15 * 86_400_000);
    std::fs::write(&kept, file.to_string()).unwrap();
    restart(&mut viewer);
    assert_eq!(get(&viewer, "/api/status").0, 401, "15 days old");
    let newer = format!("Cookie: rally_view={second}\r\n");
    assert_eq!(
        bare(&viewer, "GET", "/api/status", &viewer.address, &newer).0,
        200
    );

    // 32 more sign-ins push the second out, and the file holds 32.
    let later: Vec<String> = (0..32).map(|_| sign_in(&viewer)).collect();
    assert_eq!(
        bare(&viewer, "GET", "/api/status", &viewer.address, &newer).0,
        401
    );
    for session in [&later[0], &later[31]] {
        let cookie = format!("Cookie: rally_view={session}\r\n");
        assert_eq!(
            bare(&viewer, "GET", "/api/status", &viewer.address, &cookie).0,
            200
        );
    }
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&kept).unwrap()).unwrap();
    assert_eq!(file["sessions"].as_array().unwrap().len(), 32);
}

#[test]
fn a_request_the_browser_marks_as_from_another_site_is_refused_even_with_the_key() {
    let viewer = start();
    for site in ["cross-site", "same-site"] {
        let extra = format!("Sec-Fetch-Site: {site}\r\n");
        let (status, text) = raw(&viewer, "GET", "/api/status", &viewer.address, &extra);
        assert_eq!(status, 403, "{site}");
        assert!(!text.contains("untrusted_text"));
        // The refusal says how to sign in instead (RAL-88).
        assert!(text.contains("other sites can't read this page"), "{text}");
        assert!(
            text.contains("run `rally web --login` in a terminal"),
            "{text}"
        );
        assert!(text.contains("address bar, not from another page or a browser extension"));
    }
    for site in ["same-origin", "none"] {
        let extra = format!("Sec-Fetch-Site: {site}\r\n");
        assert_eq!(
            raw(&viewer, "GET", "/api/status", &viewer.address, &extra).0,
            200,
            "{site}"
        );
    }
}

/// The viewer's first line, from `rally web` started on `dir` with `args`, and the child to stop.
fn first_line(db: &Database, dir: &std::path::Path, args: &[&str]) -> (std::process::Child, Value) {
    let mut child = on(db, dir)
        .args(["web", "--bind", "127.0.0.1:0", "--key-file"])
        .arg(dir.join("web-key"))
        .args(args)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    (child, serde_json::from_str(&line).unwrap())
}

/// RAL-74: a whole rally ran with no DiscoSim page, because the viewer only read its built-in list
/// unless `--projects` was given. Now `projects.json` beside the pairing is read by default.
#[test]
fn the_projects_beside_the_pairing_are_read_by_default() {
    let db = scratch_pg::shared().database();
    // With none there, the built-in list.
    let dir = tempfile::tempdir().unwrap();
    let (mut child, started) = first_line(&db, dir.path(), &[]);
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(started["projects"], "built-in", "{started}");

    // With one there, that one.
    let dir = tempfile::tempdir().unwrap();
    let own = dir.path().join("projects.json");
    std::fs::write(&own, "[]").unwrap();
    let (mut child, started) = first_line(&db, dir.path(), &[]);
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(started["projects"], json!(own), "{started}");

    // --projects still wins over it.
    let other = dir.path().join("other.json");
    std::fs::write(&other, "[]").unwrap();
    let (mut child, started) =
        first_line(&db, dir.path(), &["--projects", other.to_str().unwrap()]);
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(started["projects"], json!(other), "{started}");
}

#[test]
fn a_broken_default_projects_file_is_an_error_not_the_built_in_list() {
    let db = scratch_pg::shared().database();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("projects.json"), "{not a list").unwrap();
    let output = on(&db, dir.path())
        .args(["web", "--bind", "127.0.0.1:0", "--key-file"])
        .arg(dir.path().join("web-key"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("projects.json") && stderr.contains("isn't a projects list"),
        "{stderr}"
    );
}

/// The body of a response, after its headers.
fn body_of(text: &str) -> &str {
    text.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// RAL-77: a past rally opens as its own board, as the ledger stood when it closed, rather than
/// as the whole ledger at that record.
#[test]
fn a_past_rally_opens_as_its_own_board() {
    let viewer = start_after("[]", None, 60, |db, dir| {
        let write = |args: &[&str]| {
            let out = on(db, dir)
                .args(["--credential", "a1", "--as", "frank-crocodile"])
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        write(&["ready", "DSM-1"]);
        write(&["claim", "--team", "crafty-wolverine", "DSM-1"]);
        write(&["ready", "RAL-1"]);
        write(&["claim", "--team", "rally", "RAL-1"]);
        write(&[
            "open-rally",
            "--name",
            "DiscoSim",
            "--project",
            "DSM",
            "--team",
            "crafty-wolverine",
        ]);
        write(&["ready", "DSM-2"]);
        write(&["claim", "--team", "silken-birch", "DSM-2"]);
        write(&["close-rally", "--note", "stood down"]);
        // After the close: not on its board.
        write(&["ready", "DSM-3"]);
    });
    let mut opened = None;
    for _ in 0..200 {
        let (_, text) = get(&viewer, "/api/status");
        if let Ok(status) = serde_json::from_str::<Value>(body_of(&text))
            && let Some(seq) = status["rally"]["past"][0]["opened_seq"].as_u64()
        {
            opened = Some(seq);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let opened = opened.expect("the rallies were never read");
    let (status, text) = get(&viewer, &format!("/api/past?rally={opened}"));
    assert_eq!(status, 200, "{text}");
    let past: Value = serde_json::from_str(body_of(&text)).unwrap();
    let rally = &past["rally"];
    assert_eq!(rally["name"], "DiscoSim", "{past}");
    assert_eq!(rally["closed"], true);
    assert_eq!(rally["note"], "stood down");
    assert_eq!(past["as_of"], rally["closed_seq"], "it stands at the close");
    assert!(
        past["stories"].get("DSM-3").is_none(),
        "nothing after the close"
    );
    let mut stories: Vec<&str> = rally["board"]["stories"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    stories.sort_unstable();
    // DSM-1 was in flight when it opened; RAL-1 isn't its project's.
    assert_eq!(stories, ["DSM-1", "DSM-2"], "{rally}");
    // The open rally, or one that never was, has no past board.
    assert_eq!(
        get(&viewer, &format!("/api/past?rally={}", opened + 1)).0,
        404
    );
    assert_eq!(get(&viewer, "/api/past?rally=1&as_of=1").0, 400);
    assert_eq!(get(&viewer, "/api/past").0, 400);
    // A record number still shows the whole ledger then, with no board.
    let (status, text) = get(&viewer, &format!("/api/past?as_of={opened}"));
    assert_eq!(status, 200, "{text}");
    assert!(
        serde_json::from_str::<Value>(body_of(&text))
            .unwrap()
            .get("rally")
            .is_none()
    );
}

/// RAL-84: sign in through the button in a real headless Chrome, as the operator does. RAL-83's
/// bug, Chrome's own `Origin: null` under a no-referrer page, passed every curl-driven test here.
/// It runs when `RALLY_TEST_CHROME` names a Chrome command (for example `google-chrome`, or
/// `flatpak run com.google.Chrome`), with Node 22 or later on the path; CI sets it, and sets
/// `RALLY_TEST_BROWSER_REQUIRED` so it can't be skipped there.
#[test]
fn a_real_browser_signs_in_through_the_button() {
    let Some(chrome) = std::env::var("RALLY_TEST_CHROME")
        .ok()
        .filter(|command| !command.trim().is_empty())
    else {
        assert!(
            std::env::var_os("RALLY_TEST_BROWSER_REQUIRED").is_none(),
            "RALLY_TEST_BROWSER_REQUIRED is set, but RALLY_TEST_CHROME names no browser"
        );
        eprintln!("skipped: set RALLY_TEST_CHROME to sign in through a real browser (RAL-84)");
        return;
    };
    let viewer = start();
    let url = format!("http://{}{}", viewer.address, login(&viewer));
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/browser/sign-in.mjs");
    let output = Command::new("node")
        .arg(&script)
        .arg(&url)
        .args(chrome.split_whitespace())
        .output()
        .expect("node is needed to drive the browser");
    let printed = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{printed}\n{stderr}");
    let seen: Value = serde_json::from_str(printed.trim()).unwrap_or_else(|_| panic!("{printed}"));
    // Opening the address showed the button and spent nothing.
    assert_eq!(seen["codePage"]["button"], true, "{seen}");
    assert_eq!(seen["codePage"]["referrer"], "same-origin", "{seen}");
    // Pressing it signed the browser in: the board, and its data, are served.
    assert_eq!(seen["after"]["path"], "/", "{seen}");
    assert_eq!(seen["after"]["status"], 200, "{seen}");
    assert!(
        !seen["after"]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("other sites can't read this page"),
        "{seen}"
    );
}
