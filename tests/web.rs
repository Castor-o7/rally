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
    start_after_with(projects_json, fake_gh, code_seconds, setup, |_| Vec::new())
}

/// Like `start_after`, with more arguments for `rally web`, made from the test's folder.
fn start_after_with(
    projects_json: &str,
    fake_gh: Option<&str>,
    code_seconds: u64,
    setup: impl FnOnce(&Database, &std::path::Path),
    web_args: impl FnOnce(&std::path::Path) -> Vec<String>,
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
        .args(web_args(dir.path()))
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
        // Never kept by the browser, so a reload always fetches the code the viewer serves now
        // (RAL-112).
        assert!(lower.contains("cache-control: no-store"), "{path}");
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

/// RAL-112: the page is stamped with a fingerprint of the page code the viewer serves, and every
/// status carries the same one, so a tab running other code can tell and reload.
#[test]
fn the_page_and_its_status_carry_the_same_page_code() {
    let viewer = start();
    let (status, text) = wait_for_ledger(&viewer);
    assert_eq!(status, 200);
    let value: Value = serde_json::from_str(body_of(&text)).unwrap();
    let code = value["page_code"]
        .as_str()
        .expect("the status names its page code");
    assert_eq!(code.len(), 16, "{code}");
    assert!(code.bytes().all(|b| b.is_ascii_hexdigit()), "{code}");
    let page = get(&viewer, "/").1;
    assert!(
        page.contains(&format!(
            r#"<meta name="rally-page-code" content="{code}">"#
        )),
        "{page}"
    );
    assert!(!page.contains("PAGE_CODE"), "the placeholder is replaced");
}

/// The Chrome command `RALLY_TEST_CHROME` names, or `None` to skip a browser test (RAL-84).
/// `RALLY_TEST_BROWSER_REQUIRED`, which CI sets, makes a missing browser a failure.
fn browser() -> Option<String> {
    let chrome = std::env::var("RALLY_TEST_CHROME")
        .ok()
        .filter(|command| !command.trim().is_empty());
    if chrome.is_none() {
        assert!(
            std::env::var_os("RALLY_TEST_BROWSER_REQUIRED").is_none(),
            "RALLY_TEST_BROWSER_REQUIRED is set, but RALLY_TEST_CHROME names no browser"
        );
        eprintln!("skipped: set RALLY_TEST_CHROME to sign in through a real browser (RAL-84)");
    }
    chrome
}

/// Signs in to `viewer` through the button in headless `chrome`, and with `probe`, a JavaScript
/// expression, evaluates it on the board afterwards. Returns what tests/browser/sign-in.mjs saw.
fn in_browser(viewer: &Viewer, chrome: &str, probe: Option<&str>) -> Value {
    in_browser_with(viewer, chrome, probe, None)
}

/// Like `in_browser`, and with `schemes` ("light,dark"), runs the probe again under each emulated
/// colour scheme, the results in `probes` by scheme (RAL-91).
fn in_browser_with(
    viewer: &Viewer,
    chrome: &str,
    probe: Option<&str>,
    schemes: Option<&str>,
) -> Value {
    in_browser_full(viewer, chrome, probe, schemes, None)
}

/// Like `in_browser`, in a window of `window` ("width,height") rather than the script's 1500x1000
/// (RAL-111).
fn in_browser_at(
    viewer: &Viewer,
    chrome: &str,
    probe: Option<&str>,
    window: Option<&str>,
) -> Value {
    in_browser_full(viewer, chrome, probe, None, window)
}

/// `in_browser` with each of its options: colour schemes (RAL-91) and a window size (RAL-111).
fn in_browser_full(
    viewer: &Viewer,
    chrome: &str,
    probe: Option<&str>,
    schemes: Option<&str>,
    window: Option<&str>,
) -> Value {
    let url = format!("http://{}{}", viewer.address, login(viewer));
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/browser/sign-in.mjs");
    let mut command = Command::new("node");
    command
        .arg(&script)
        .arg(&url)
        .args(chrome.split_whitespace());
    let probe_file = tempfile::NamedTempFile::new().unwrap();
    if let Some(probe) = probe {
        std::fs::write(probe_file.path(), probe).unwrap();
        command.env("RALLY_BROWSER_PROBE", probe_file.path());
    }
    if let Some(schemes) = schemes {
        command.env("RALLY_BROWSER_SCHEMES", schemes);
    }
    if let Some(window) = window {
        command.env("RALLY_BROWSER_WINDOW", window);
    }
    let output = command
        .output()
        .expect("node is needed to drive the browser");
    let printed = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{printed}\n{stderr}");
    let seen: Value = serde_json::from_str(printed.trim()).unwrap_or_else(|_| panic!("{printed}"));
    nothing_of_the_browser_survives(&seen);
    seen
}

/// RAL-97: once the script has ended, no profile it used is left, under the host's /tmp or a
/// flatpak sandbox's, and no browser still runs on one. Each run under flatpak once left a whole
/// headless Chrome and a 170 MB profile in $XDG_RUNTIME_DIR, until that tmpfs filled (2026-11-06).
fn nothing_of_the_browser_survives(seen: &Value) {
    let profiles = seen["profiles"]
        .as_array()
        .expect("the script names its profiles");
    assert!(!profiles.is_empty(), "{seen}");
    for profile in profiles {
        let profile = profile.as_str().unwrap();
        assert!(
            !std::path::Path::new(profile).exists(),
            "{profile} was left behind"
        );
        // A browser on it has the profile's own name in its command line, wherever it was put.
        let name = std::path::Path::new(profile)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let still = Command::new("pgrep")
            .args(["-f", "--", &name])
            .output()
            .unwrap();
        assert!(
            still.stdout.is_empty(),
            "a browser still runs on {name}: {}",
            String::from_utf8_lossy(&still.stdout)
        );
    }
}

/// RAL-112: in a real browser, the page knows the code it runs, doesn't reload while the viewer
/// serves the same code, reloads once when it serves other code, and never twice for one code.
#[test]
fn a_tab_reloads_once_when_the_viewer_serves_new_page_code() {
    let Some(chrome) = browser() else { return };
    let viewer = start();
    let probe = r##"(async () => {
      const status = await (await fetch("/api/status", { cache: "no-store" })).json();
      sessionStorage.removeItem("rally-reloaded-for");
      const same = reloadForNewCode(status);
      sessionStorage.setItem("rally-reloaded-for", "0123456789abcdef");
      const again = reloadForNewCode({ page_code: "0123456789abcdef" });
      sessionStorage.removeItem("rally-reloaded-for");
      let reloads = 0;
      const changed = reloadForNewCode({ page_code: "fedcba9876543210" }, () => { reloads += 1; });
      const twice = reloadForNewCode({ page_code: "fedcba9876543210" }, () => { reloads += 1; });
      return { pageCode: PAGE_CODE, serving: status.page_code, same, again, changed, twice, reloads,
               remembered: sessionStorage.getItem("rally-reloaded-for") };
    })()"##;
    let seen = in_browser(&viewer, &chrome, Some(probe));
    let probe = &seen["probe"];
    assert_eq!(probe["pageCode"], probe["serving"], "{seen}");
    assert_eq!(probe["same"], false, "the same code doesn't reload: {seen}");
    assert_eq!(probe["again"], false, "never twice for one code: {seen}");
    assert_eq!(probe["changed"], true, "new code reloads: {seen}");
    assert_eq!(probe["twice"], false, "and only once: {seen}");
    assert_eq!(probe["reloads"], 1, "{seen}");
    assert_eq!(probe["remembered"], "fedcba9876543210", "{seen}");
}

/// RAL-84: sign in through the button in a real headless Chrome, as the operator does. RAL-83's
/// bug, Chrome's own `Origin: null` under a no-referrer page, passed every curl-driven test here.
/// It runs when `RALLY_TEST_CHROME` names a Chrome command (for example `google-chrome`, or
/// `flatpak run com.google.Chrome`), with Node 22 or later on the path; CI sets it, and sets
/// `RALLY_TEST_BROWSER_REQUIRED` so it can't be skipped there.
#[test]
fn a_real_browser_signs_in_through_the_button() {
    let Some(chrome) = browser() else { return };
    let viewer = start();
    let seen = in_browser(&viewer, &chrome, None);
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

/// RAL-90, in the browser: ages tick between polls, the live indicator shows, Verified is green
/// and In review amber, a commit in evidence is shortened with the whole of it in its title, and
/// IDs and ages don't wrap.
#[test]
fn the_board_keeps_its_ages_current_and_shows_commits_short() {
    let Some(chrome) = browser() else { return };
    let commit = "0123456789abcdef0123456789abcdef01234567";
    let viewer = start_after("[]", None, 60, |db, dir| {
        let write = |args: &[&str]| {
            let out = on(db, dir)
                .args(["--credential", "a1", "--as", "author"])
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        // An open rally fixes the board to RAL, whoever is on this machine's Muster roster.
        write(&[
            "open-rally",
            "--name",
            "test",
            "--project",
            "RAL",
            "--team",
            "rally",
        ]);
        write(&["ready", "RAL-1"]);
        write(&["claim", "RAL-1", "--team", "rally"]);
        let evidence = format!("Castor-o7/rally@{commit}");
        write(&[
            "submit",
            "RAL-1",
            "--team",
            "rally",
            "--evidence",
            &evidence,
            "--not-checkable",
        ]);
    });
    let probe = r##"(async () => {
      const ages = () => [...document.querySelectorAll(".age[data-ts]")].map((n) => n.textContent);
      const before = ages();
      await new Promise((r) => setTimeout(r, 2500));
      const after = ages();
      const commit = document.querySelector("code.commit");
      const colour = (name) => {
        const probe = document.createElement("i");
        probe.className = name;
        document.body.append(probe);
        const value = getComputedStyle(probe).backgroundColor;
        probe.remove();
        return value;
      };
      const firstCell = document.querySelector("#ledger td");
      const ageCell = document.querySelector("td.nowrap .age")?.parentElement;
      return {
        before, after,
        freshness: document.getElementById("freshness").textContent,
        freshnessClass: document.getElementById("freshness").className,
        commit: commit && { text: commit.textContent, title: commit.title },
        verified: colour("s-verified"),
        inReview: colour("s-in_review"),
        firstCellWraps: firstCell ? getComputedStyle(firstCell).whiteSpace : null,
        ageCellWraps: ageCell ? getComputedStyle(ageCell).whiteSpace : null,
        // RAL-96: a state shows once, as a dot and its name.
        stateCell: (() => {
          const pill = document.querySelector("#ledger .state-pill");
          const cell = pill?.closest("td");
          const header = cell && cell.closest("table").querySelectorAll("th")[cell.cellIndex];
          return pill && { text: cell.textContent.trim(), dot: Boolean(pill.querySelector(".dot.s-in_review")),
            header: header?.querySelector(".label")?.textContent || null };
        })(),
      };
    })()"##;
    let seen = in_browser(&viewer, &chrome, Some(probe));
    let probe = &seen["probe"];
    assert_eq!(seen["after"]["status"], 200, "{seen}");
    // Some age moved on by itself between polls, with nothing redrawn.
    let before = probe["before"].as_array().unwrap();
    let after = probe["after"].as_array().unwrap();
    assert!(!before.is_empty(), "{probe}");
    assert_eq!(before.len(), after.len(), "{probe}");
    assert_ne!(before, after, "the ages stood still: {probe}");
    let freshness = probe["freshness"].as_str().unwrap();
    assert!(
        freshness.contains("Live") && freshness.contains("updated"),
        "{probe}"
    );
    assert!(
        probe["freshnessClass"].as_str().unwrap().contains("live"),
        "{probe}"
    );
    // The commit is shortened, with the whole evidence in the title.
    assert_eq!(
        probe["commit"]["text"], "Castor-o7/rally@0123456",
        "{probe}"
    );
    assert_eq!(
        probe["commit"]["title"],
        format!("Castor-o7/rally@{commit}"),
        "{probe}"
    );
    // Verified is green and In review amber (light scheme).
    assert_eq!(probe["verified"], "rgb(27, 175, 122)", "{probe}");
    assert_eq!(probe["inReview"], "rgb(237, 161, 0)", "{probe}");
    assert_eq!(probe["firstCellWraps"], "nowrap", "{probe}");
    assert_eq!(probe["ageCellWraps"], "nowrap", "{probe}");
    // RAL-96: the state shows once, as its coloured dot and name, labelled declared while in review.
    // With only in-review stories, the State column is all declared, so it says so in its header
    // and not on the cell (RAL-93).
    assert_eq!(probe["stateCell"]["text"], "in review", "{probe}");
    assert_eq!(probe["stateCell"]["header"], "declared", "{probe}");
    assert_eq!(probe["stateCell"]["dot"], true, "{probe}");
}

/// RAL-91, in the browser: every bar's fill is coloured by what it measures, in both schemes. The
/// ledger's progress is one bar of Verified green, In review amber and Sent back pink; roadmap ticks,
/// which aren't a ledger state, have a neutral colour of their own, which the legend names; and so
/// green only ever means Verified. Before RAL-91 every fill was one blue, whatever it measured.
#[cfg(unix)]
#[test]
fn every_bar_is_coloured_by_what_it_measures() {
    let Some(chrome) = browser() else { return };
    let commit = "0123456789abcdef0123456789abcdef01234567";
    let projects = r#"[{"name": "Rally", "prefix": "RAL", "repo": "Castor-o7/rally",
        "path": "docs/roadmap.md", "tick_meaning": "Verified by the watchman"}]"#;
    let viewer = start_after(projects, Some(FAKE_GH), 60, |db, dir| {
        let write = |name: &str, args: &[&str]| {
            let out = on(db, dir)
                .args(["--credential", "a1", "--as", name])
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        write(
            "author",
            &[
                "open-rally",
                "--name",
                "test",
                "--project",
                "RAL",
                "--team",
                "rally",
            ],
        );
        let evidence = format!("Castor-o7/rally@{commit}");
        for story in ["RAL-1", "RAL-2", "RAL-3"] {
            write("author", &["ready", story]);
            write("author", &["claim", story, "--team", "rally"]);
            write(
                "author",
                &[
                    "submit",
                    story,
                    "--team",
                    "rally",
                    "--evidence",
                    &evidence,
                    "--not-checkable",
                ],
            );
        }
        write(
            "reader",
            &["verify", "RAL-1", "--note", "read it", "--commit", commit],
        );
        write("reader", &["send-back", "RAL-3", "--note", "no test"]);
        write("author", &["ready", "RAL-4"]);
    });
    let probe = r##"(async () => {
      // The roadmap is read in the background, so wait for the project's card to have both figures.
      const ready = () => document.querySelector(".roadmap .bar > .ticked")
        && document.querySelector(".roadmap .bar > .s-verified");
      for (let i = 0; i < 40 && !ready(); i++) {
        await refresh();
        await new Promise((r) => setTimeout(r, 250));
      }
      const colour = (node) => getComputedStyle(node).backgroundColor;
      const card = document.querySelector(".roadmap");
      return {
        fills: [...document.querySelectorAll(".bar > div")]
          .map((node) => ({ class: node.className, colour: colour(node), title: node.title })),
        keys: [...document.querySelectorAll(".legend i")]
          .map((node) => ({ class: node.className, colour: colour(node), text: node.parentElement.textContent })),
        card: card && [...card.querySelectorAll(".figure > .bar")]
          .map((bar) => [...bar.children].map((part) => part.className)),
      };
    })()"##;
    let seen = in_browser_with(&viewer, &chrome, Some(probe), Some("light,dark"));
    assert_eq!(seen["after"]["status"], 200, "{seen}");
    // The colours app.css gives each meaning, as Chrome computes them.
    let expected = |scheme: &str, class: &str| -> &'static str {
        match (scheme, class) {
            ("light", "ticked") => "rgb(95, 94, 89)",
            ("light", "s-ready") => "rgb(42, 120, 214)",
            ("light", "s-claimed") => "rgb(235, 104, 52)",
            ("light", "s-in_review") => "rgb(237, 161, 0)",
            ("light", "s-verified") => "rgb(27, 175, 122)",
            ("light", "s-sent_back") => "rgb(232, 123, 164)",
            ("dark", "ticked") => "rgb(166, 165, 156)",
            ("dark", "s-ready") => "rgb(57, 135, 229)",
            ("dark", "s-claimed") => "rgb(217, 89, 38)",
            ("dark", "s-in_review") => "rgb(201, 133, 0)",
            ("dark", "s-verified") => "rgb(25, 158, 112)",
            ("dark", "s-sent_back") => "rgb(213, 81, 129)",
            _ => panic!("a fill with no meaning: {class:?} ({scheme})"),
        }
    };
    for scheme in ["light", "dark"] {
        let probe = &seen["probes"][scheme];
        let fills = probe["fills"].as_array().unwrap();
        assert!(!fills.is_empty(), "{scheme}: {probe}");
        // Every fill on the page has exactly one meaning, drawn in that meaning's colour.
        for fill in fills {
            let class = fill["class"].as_str().unwrap();
            assert_eq!(fill["colour"], expected(scheme, class), "{scheme}: {fill}");
        }
        // The project's card: its ticks in the ticked colour, then the ledger's progress as Verified,
        // In review and Sent back, one story each, each segment titled with its count.
        assert_eq!(
            probe["card"],
            json!([["ticked"], ["s-verified", "s-in_review", "s-sent_back"]]),
            "{scheme}: {probe}"
        );
        for (class, title) in [
            ("s-verified", "Verified: 1"),
            ("s-in_review", "In review: 1"),
            ("s-sent_back", "Sent back: 1"),
        ] {
            assert!(
                fills
                    .iter()
                    .any(|fill| fill["class"] == class && fill["title"] == title),
                "{scheme}: no {class} titled {title:?}: {probe}"
            );
        }
        // The legend says what the ticked colour means, and keys each state in its colour.
        let keys = probe["keys"].as_array().unwrap();
        let ticked = keys
            .iter()
            .find(|key| key["class"] == "ticked")
            .unwrap_or_else(|| panic!("{scheme}: no key for ticks: {probe}"));
        assert_eq!(ticked["colour"], expected(scheme, "ticked"), "{scheme}");
        assert!(
            ticked["text"]
                .as_str()
                .unwrap()
                .contains("not a state in the ledger"),
            "{scheme}: {ticked}"
        );
        for key in keys {
            let class = key["class"].as_str().unwrap();
            assert_eq!(key["colour"], expected(scheme, class), "{scheme}: {key}");
        }
    }
}

/// RAL-96: the 30-second refresh redraws every section, and a section someone opened stays open.
#[cfg(unix)]
#[test]
fn an_open_section_stays_open_across_a_refresh() {
    let Some(chrome) = browser() else { return };
    // A project the rally doesn't name puts a "Not in this rally" section on the board.
    let projects = r#"[{"name": "Rally", "prefix": "RAL", "repo": "Castor-o7/rally",
        "path": "docs/roadmap.md", "tick_meaning": "Verified by the watchman"},
        {"name": "Muster", "prefix": "MUS", "repo": "Castor-o7/muster",
        "path": "docs/roadmap.md", "tick_meaning": "Verified by the watchman"}]"#;
    let viewer = start_after(projects, Some(FAKE_GH), 60, |db, dir| {
        let out = on(db, dir)
            .args(["--credential", "a1", "--as", "author"])
            .args([
                "open-rally",
                "--name",
                "test",
                "--project",
                "RAL",
                "--team",
                "rally",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    });
    let probe = r##"(async () => {
      // The roadmaps are read in the background, so wait for a section to appear.
      for (let i = 0; i < 40 && !document.querySelector("details"); i++) {
        await refresh();
        await new Promise((r) => setTimeout(r, 250));
      }
      const before = document.querySelector("details");
      if (!before) return { found: false };
      before.open = true;
      const summary = before.querySelector("summary").textContent;
      await refresh();
      const after = [...document.querySelectorAll("details > summary")]
        .find((s) => s.textContent === summary)?.parentElement;
      return { found: true, summary, redrawn: !before.isConnected, open: Boolean(after && after.open) };
    })()"##;
    let seen = in_browser(&viewer, &chrome, Some(probe));
    let probe = &seen["probe"];
    assert_eq!(probe["found"], true, "{seen}");
    assert_eq!(
        probe["redrawn"], true,
        "the refresh should redraw the section: {probe}"
    );
    assert_eq!(probe["open"], true, "the open section closed: {probe}");
}

/// RAL-104: the report box belongs to the open rally. A second rally opened the same day doesn't
/// take the first one's writeup, and its events are its own, newest first, while the first keeps
/// its events in its past-rally view.
#[test]
fn the_report_box_belongs_to_the_open_rally_and_lists_its_events() {
    let projects = r#"[{"name": "Rally", "prefix": "RAL", "repo": "Castor-o7/rally",
        "path": "docs/roadmap.md", "tick_meaning": "Verified by the watchman"}]"#;
    let writeups = std::cell::RefCell::new(None);
    let viewer = start_after_with(
        projects,
        // GitHub is never reached: the writeups are read locally.
        Some("#!/bin/sh\necho 'gh: offline' >&2\nexit 1\n"),
        60,
        |db, dir| {
            let write = |args: &[&str]| {
                let out = on(db, dir)
                    .args(["--credential", "a1", "--as", "mellow-weevil"])
                    .args(args)
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "{args:?}: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            };
            write(&["open-rally", "--name", "first", "--project", "RAL"]);
            write(&[
                "event",
                "--headline",
                "the first rally's incident",
                "--severity",
                "incident",
                "--backing",
                "record 2",
            ]);
            // The first rally's writeup, written today, before the second rally opens.
            let folder = dir.join("watchman");
            std::fs::create_dir(&folder).unwrap();
            std::fs::write(folder.join("2026-11-06.md"), "the first rally's day").unwrap();
            *writeups.borrow_mut() = Some(folder);
            std::thread::sleep(std::time::Duration::from_millis(20));
            write(&["close-rally", "--note", "done"]);
            write(&["open-rally", "--name", "second", "--project", "RAL"]);
            write(&[
                "event",
                "--headline",
                "a sha that didn't exist",
                "--severity",
                "incident",
                "--backing",
                "Castor-o7/marshal#11",
            ]);
            write(&[
                "event",
                "--headline",
                "the tmpfs filled",
                "--severity",
                "notice",
            ]);
        },
        |_| {
            let folder = writeups.borrow().clone().unwrap();
            vec!["--watchman-summaries".into(), folder.display().to_string()]
        },
    );
    let mut status = Value::Null;
    for _ in 0..200 {
        let (_, text) = get(&viewer, "/api/status");
        status = serde_json::from_str(body_of(&text)).unwrap_or(Value::Null);
        if status["muster_summary"]["for_rally"].is_string() && status["rally"]["events"].is_array()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // Dated today, but written before this rally opened: not this rally's.
    assert_eq!(
        status["muster_summary"]["for_rally"], "before_it_opened",
        "{status}"
    );
    assert!(
        status["muster_summary"]["written_at_ms"].is_i64(),
        "{status}"
    );
    let rally = &status["rally"];
    assert_eq!(rally["name"], "second");
    let headlines: Vec<&str> = rally["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["headline"].as_str())
        .collect();
    // Newest first, and none of the first rally's.
    assert_eq!(
        headlines,
        ["the tmpfs filled", "a sha that didn't exist"],
        "{rally}"
    );
    let bare = &rally["events"][0];
    assert_eq!(bare["severity"], "notice");
    assert_eq!(bare["kind"], "declared");
    assert_eq!(bare["by"], "mellow-weevil");
    assert_eq!(
        bare["backing"],
        json!([]),
        "nothing backs it, and that's shown"
    );
    // The first rally keeps its events in its past-rally view.
    let opened = rally["past"][0]["opened_seq"].as_u64().unwrap();
    let (code, text) = get(&viewer, &format!("/api/past?rally={opened}"));
    assert_eq!(code, 200, "{text}");
    let past: Value = serde_json::from_str(body_of(&text)).unwrap();
    assert_eq!(
        past["rally"]["events"][0]["headline"],
        "the first rally's incident"
    );
    assert_eq!(past["rally"]["events"].as_array().unwrap().len(), 1);

    // In the browser, the report box says so, and lists the events above it.
    let Some(chrome) = browser() else { return };
    let probe = r##"(async () => {
      for (let i = 0; i < 100 && !document.querySelector("#muster ul.events"); i++) {
        await new Promise((r) => setTimeout(r, 100));
      }
      const box = document.getElementById("muster");
      const incident = box.querySelector("ul.events li.incident > .label");
      return {
        text: box.textContent,
        incident: incident && getComputedStyle(incident).color,
        notice: getComputedStyle(box.querySelector("ul.events li.notice > .label")).color,
      };
    })()"##;
    let seen = in_browser(&viewer, &chrome, Some(probe));
    let text = seen["probe"]["text"].as_str().unwrap_or_default();
    assert!(text.contains("This rally's events (2)"), "{seen}");
    assert!(
        text.contains("No writeup for this rally yet") && text.contains("written before it opened"),
        "{seen}"
    );
    assert!(!text.contains("the first rally's"), "{seen}");
    let (newer, older) = (
        text.find("the tmpfs filled").unwrap(),
        text.find("a sha that didn't exist").unwrap(),
    );
    assert!(newer < older, "newest first: {seen}");
    assert!(text.contains("nothing named as backing it"), "{seen}");
    assert!(text.contains("backed by Castor-o7/marshal#11"), "{seen}");
    assert_ne!(seen["probe"]["incident"], seen["probe"]["notice"], "{seen}");
}

/// RAL-93, in the browser: a column whose cells are all known the same way says so once, in its
/// header, and a column that mixes kinds keeps a label on each cell that has one; a legend says what
/// each kind means.
#[test]
fn provenance_goes_in_the_header_unless_a_column_mixes_kinds() {
    let Some(chrome) = browser() else { return };
    let commit = "0123456789abcdef0123456789abcdef01234567";
    let viewer = start_after("[]", None, 60, |db, dir| {
        let write = |name: &str, args: &[&str]| {
            let out = on(db, dir)
                .args(["--credential", "a1", "--as", name])
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        write(
            "author",
            &[
                "open-rally",
                "--name",
                "test",
                "--project",
                "RAL",
                "--team",
                "rally",
            ],
        );
        let evidence = format!("Castor-o7/rally@{commit}");
        for story in ["RAL-1", "RAL-2"] {
            write("author", &["ready", story]);
            write("author", &["claim", story, "--team", "rally"]);
            write(
                "author",
                &[
                    "submit",
                    story,
                    "--team",
                    "rally",
                    "--evidence",
                    &evidence,
                    "--not-checkable",
                ],
            );
        }
        // RAL-2 is verified, so the State column mixes kinds; RAL-1 stays in review.
        write(
            "reader",
            &["verify", "RAL-2", "--note", "read it", "--commit", commit],
        );
    });
    let probe = r##"(async () => {
      for (let i = 0; i < 100 && document.querySelectorAll("#ledger tbody tr").length < 2; i++) {
        await new Promise((r) => setTimeout(r, 100));
      }
      const table = [...document.querySelectorAll("#ledger table")]
        .find((t) => [...t.querySelectorAll("th")].some((th) => th.textContent.startsWith("Evidence")));
      const headers = [...table.querySelectorAll("th")].map((th) => ({
        name: th.firstChild.textContent, label: th.querySelector(".label")?.textContent || null,
      }));
      const column = (name) => {
        const i = headers.findIndex((h) => h.name === name);
        return [...table.querySelectorAll("tbody tr")].map((tr) => ({
          story: tr.children[0].textContent,
          labels: [...tr.children[i].querySelectorAll(".label")].map((l) => l.textContent),
        }));
      };
      const legend = [...document.querySelectorAll("#provenance dt")].map((dt) => dt.textContent);
      return { headers, state: column("State"), evidence: column("Evidence"), legend };
    })()"##;
    let seen = in_browser(&viewer, &chrome, Some(probe));
    let probe = &seen["probe"];
    let header = |name: &str| {
        probe["headers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["name"] == name)
            .unwrap_or_else(|| panic!("no {name} column: {probe}"))["label"]
            .clone()
    };
    // Every evidence cell is declared: said once, in the header, and on no cell.
    assert_eq!(header("Evidence"), "declared", "{probe}");
    for row in probe["evidence"].as_array().unwrap() {
        assert_eq!(row["labels"], json!([]), "{probe}");
    }
    // The State column mixes a declared state with a verified one: no header label, and the
    // in-review story's cell keeps its own.
    assert_eq!(header("State"), Value::Null, "{probe}");
    let state = |story: &str| {
        probe["state"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["story"] == story)
            .unwrap_or_else(|| panic!("no {story}: {probe}"))["labels"]
            .clone()
    };
    assert_eq!(state("RAL-1"), json!(["declared"]), "{probe}");
    assert_eq!(state("RAL-2"), json!([]), "{probe}");
    assert_eq!(
        probe["legend"],
        json!(["observed", "declared", "verified", "attested"]),
        "{probe}"
    );
}

/// RAL-92, in the browser: on a 1500-pixel window the page uses up to 1200 of it, rather than 960
/// with 40% empty, and it never runs wider than the window.
#[test]
fn the_page_uses_a_wide_window() {
    let Some(chrome) = browser() else { return };
    let viewer = start_after("[]", None, 60, |_, _| {});
    let probe = r##"(async () => {
      return {
        window: window.innerWidth,
        body: document.body.getBoundingClientRect().width,
        maxWidth: getComputedStyle(document.body).maxWidth,
        overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
      };
    })()"##;
    let seen = in_browser(&viewer, &chrome, Some(probe));
    let probe = &seen["probe"];
    assert_eq!(probe["window"], 1500, "{seen}");
    assert_eq!(probe["maxWidth"], "1200px", "{probe}");
    assert_eq!(probe["body"], 1200, "{probe}");
    assert_eq!(probe["overflow"], false, "{probe}");
}

/// RAL-111, in the browser: on an 800-pixel window, as on a half screen, the page fits it. Wide
/// tables scroll inside their own box, and the page itself never scrolls sideways.
#[test]
fn the_page_fits_a_narrow_window() {
    let Some(chrome) = browser() else { return };
    let commit = "0123456789abcdef0123456789abcdef01234567";
    let viewer = start_after("[]", None, 60, |db, dir| {
        let write = |args: &[&str]| {
            let out = on(db, dir)
                .args(["--credential", "a1", "--as", "author"])
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        write(&[
            "open-rally",
            "--name",
            "test",
            "--project",
            "RAL",
            "--team",
            "rally",
        ]);
        // A story in review, so the evidence table, the widest, is on the page.
        write(&["ready", "RAL-1"]);
        write(&["claim", "RAL-1", "--team", "rally"]);
        let evidence = format!("Castor-o7/rally@{commit}");
        write(&[
            "submit",
            "RAL-1",
            "--team",
            "rally",
            "--evidence",
            &evidence,
            "--not-checkable",
        ]);
    });
    let probe = r##"(async () => {
      for (let i = 0; i < 100 && !document.querySelector("#ledger table"); i++) {
        await new Promise((r) => setTimeout(r, 100));
      }
      const root = document.documentElement;
      // Anything wider than the window that isn't inside a box that scrolls on its own.
      const sticking = [...document.querySelectorAll("body *")]
        .filter((node) => node.getBoundingClientRect().right > root.clientWidth + 1)
        .filter((node) => !node.closest(".table-wrap"))
        .map((node) => node.tagName + (node.id ? "#" + node.id : "") + "." + node.className)
        .slice(0, 5);
      return {
        window: window.innerWidth,
        tables: document.querySelectorAll("table").length,
        overflow: root.scrollWidth > root.clientWidth,
        sticking,
      };
    })()"##;
    let seen = in_browser_at(&viewer, &chrome, Some(probe), Some("800,1000"));
    let probe = &seen["probe"];
    assert_eq!(probe["window"], 800, "{seen}");
    assert!(probe["tables"].as_u64().unwrap() > 0, "{probe}");
    assert_eq!(probe["sticking"], json!([]), "{probe}");
    assert_eq!(probe["overflow"], false, "{probe}");
}
