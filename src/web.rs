//! The read-only viewer (RAL-25, RAL-27, RAL-29): a page on this machine's loopback address.
//!
//! It holds no write path: it reads the ledger with a read-only credential or from a file, roadmap
//! files through `gh`, and the roster through `muster-shim roster`. Those are read by a background
//! thread, so requests never wait on them and a page can always show how old its data is.
//!
//! A web page in the same browser is the threat here. Requests must name this address in `Host`,
//! which stops DNS rebinding, and any `Origin` must be this page's own. Nothing sends CORS headers,
//! and the content security policy allows no inline script. See `docs/web.md`.

use std::io::Read;
use std::net::{SocketAddr, TcpListener};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::views::UNTRUSTED_TEXT;

const INDEX: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const APP_CSS: &str = include_str!("../web/app.css");

/// The projects shown by default.
pub const DEFAULT_PROJECTS: &str = include_str!("../web/projects.json");

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; \
                   base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
const LIVE_EVERY: Duration = Duration::from_secs(30);
const ROADMAPS_EVERY: Duration = Duration::from_secs(300);
const COMMAND_LIMIT: Duration = Duration::from_secs(20);

/// A project's roadmap, and what its ticks mean there.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub name: String,
    pub prefix: String,
    pub repo: String,
    pub path: String,
    pub tick_meaning: String,
}

/// Counts of checkboxes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Boxes {
    pub ticked: u32,
    pub total: u32,
    pub percent: u32,
}

/// The checkboxes under one heading.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Section {
    pub name: String,
    pub ticked: u32,
    pub total: u32,
}

/// Counts a roadmap's checkboxes overall and by `##` heading, ignoring examples in code fences.
/// A fence line's character, its length and what follows it: three or more backticks or tildes,
/// indented by at most three spaces. A backtick fence's info string can't hold a backtick.
fn fence_marker(line: &str) -> Option<(char, usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let kind = rest.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let length = rest.len() - rest.trim_start_matches(kind).len();
    if length < 3 {
        return None;
    }
    let after = &rest[length..];
    if kind == '`' && after.contains('`') {
        return None;
    }
    Some((kind, length, after))
}

pub fn parse_roadmap(text: &str) -> (Boxes, Vec<Section>) {
    let mut sections: Vec<Section> = Vec::new();
    let mut current = "Before the first heading".to_owned();
    // The open fence's character and length, as CommonMark has them.
    let mut fence: Option<(char, usize)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let marker = fence_marker(line);
        match (fence, marker) {
            (None, Some((kind, length, _))) => {
                fence = Some((kind, length));
                continue;
            }
            (Some((open, open_length)), Some((kind, length, rest)))
                if kind == open && length >= open_length && rest.trim().is_empty() =>
            {
                fence = None;
                continue;
            }
            (Some(_), _) => continue,
            (None, None) => {}
        }
        if let Some(heading) = trimmed.strip_prefix("## ") {
            current = heading.trim().to_owned();
            continue;
        }
        let item = trimmed
            .strip_prefix("- [")
            .or_else(|| trimmed.strip_prefix("* ["));
        let ticked = match item.and_then(|rest| rest.get(..2)) {
            Some("x]" | "X]") => true,
            Some(" ]") => false,
            _ => continue,
        };
        if sections
            .last()
            .is_none_or(|section| section.name != current)
        {
            sections.push(Section {
                name: current.clone(),
                ticked: 0,
                total: 0,
            });
        }
        let section = sections.last_mut().expect("just added");
        section.total += 1;
        section.ticked += u32::from(ticked);
    }
    let total: u32 = sections.iter().map(|section| section.total).sum();
    let ticked: u32 = sections.iter().map(|section| section.ticked).sum();
    // Round down, so an unfinished roadmap never shows as 100%.
    let percent = (ticked * 100).checked_div(total).unwrap_or(0);
    (
        Boxes {
            ticked,
            total,
            percent,
        },
        sections,
    )
}

/// The verified figure for a project's stories. It comes only from the ledger, never from ticks.
pub fn verified_for(prefix: &str, ledger: &Value) -> Value {
    let wanted = format!("{prefix}-");
    let stories = ledger["stories"].as_object();
    let matching: Vec<&Value> = stories
        .map(|stories| {
            stories
                .iter()
                .filter(|(id, _)| id.starts_with(&wanted))
                .map(|(_, story)| story)
                .collect()
        })
        .unwrap_or_default();
    if matching.is_empty() {
        return json!({"available": false});
    }
    let verified = matching
        .iter()
        .filter(|story| story["state"] == "verified")
        .count();
    json!({"available": true, "verified": verified, "tracked": matching.len()})
}

/// Where the viewer reads the ledger.
pub type LedgerReader = Box<dyn Fn() -> Result<Value, String> + Send>;

#[derive(Default)]
struct Snapshot {
    /// Each project's latest roadmap reading, kept after a failed read so that its age shows.
    roadmaps: Vec<Value>,
    ledger: Option<Value>,
    roster: Option<Value>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Runs a command, giving up after `limit`, so that a hung program can't freeze the page's data.
/// Returns its standard output if it succeeded.
/// The most output a command may give: a roadmap or the roster, with plenty of room.
const MAX_COMMAND_OUTPUT: u64 = 8 * 1024 * 1024;

fn run_with_timeout(mut command: Command, limit: Duration) -> Result<Vec<u8>, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("can't run it: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no output")?;
    // Read on another thread, so that a full pipe can't block the child while we wait for it.
    let (sender, output) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        // One byte past the limit says it was exceeded; the rest is never read, and the child
        // is killed below if that leaves it blocked.
        let read = (&mut stdout)
            .take(MAX_COMMAND_OUTPUT + 1)
            .read_to_end(&mut bytes);
        let _ = sender.send(read.map(|_| bytes));
    });
    let started = Instant::now();
    // Output the reader finished with before the child was seen to exit.
    let mut early: Option<std::io::Result<Vec<u8>>> = None;
    let collect = |early: &mut Option<std::io::Result<Vec<u8>>>, wait: Duration| {
        let read = match early.take() {
            Some(read) => read,
            None => output
                .recv_timeout(wait)
                .map_err(|_| "its output didn't finish".to_owned())?,
        };
        read.map_err(|e| e.to_string()).and_then(too_much)
    };
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) if status.success() => {
                // Something it left running could hold its output open, so this needs a limit too.
                let left = limit.saturating_sub(started.elapsed());
                return collect(&mut early, left.max(Duration::from_millis(100)));
            }
            Some(_) => {
                // Stopping at the limit ends a command still writing with a broken pipe, which it
                // reports as a failure; say what really happened.
                collect(&mut early, Duration::from_millis(100))?;
                return Err("it reported a failure".to_owned());
            }
            None if started.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("it didn't finish within {}s", limit.as_secs()));
            }
            None => {
                if early.is_none()
                    && let Ok(read) = output.try_recv()
                {
                    early = Some(read);
                }
                // Past the limit the reader stops, and a child still writing would block on a
                // full pipe until its time ran out, so end it now.
                if early.as_ref().is_some_and(|read| {
                    read.as_ref()
                        .is_ok_and(|bytes| bytes.len() as u64 > MAX_COMMAND_OUTPUT)
                }) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return collect(&mut early, Duration::ZERO);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Refuses output over [`MAX_COMMAND_OUTPUT`].
fn too_much(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if bytes.len() as u64 > MAX_COMMAND_OUTPUT {
        Err(format!(
            "it printed too much, over {} MiB",
            MAX_COMMAND_OUTPUT / 1024 / 1024
        ))
    } else {
        Ok(bytes)
    }
}

fn run_gh(args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("gh");
    command.args(args);
    let output = run_with_timeout(command, COMMAND_LIMIT).map_err(|e| format!("gh: {e}"))?;
    String::from_utf8(output).map_err(|_| "gh returned something unreadable".to_owned())
}

fn read_roadmap(project: &Project) -> Result<(String, String), String> {
    let commit = run_gh(&[
        "api",
        &format!("repos/{}/commits/HEAD", project.repo),
        "--jq",
        ".sha",
    ])?;
    let commit = commit.trim().to_owned();
    let contents = format!(
        "repos/{}/contents/{}?ref={commit}",
        project.repo, project.path
    );
    let text = run_gh(&["api", &contents, "-H", "Accept: application/vnd.github.raw"])?;
    Ok((commit, text))
}

fn refresh_roadmaps(projects: &[Project], previous: &[Value]) -> Vec<Value> {
    projects
        .iter()
        .enumerate()
        .map(|(index, project)| match read_roadmap(project) {
            Ok((commit, text)) => {
                let (boxes, sections) = parse_roadmap(&text);
                json!({
                    "boxes": boxes,
                    "sections": sections,
                    "commit": commit,
                    "read_at_ms": now_ms(),
                })
            }
            Err(error) => {
                let mut kept = previous.get(index).cloned().unwrap_or_else(|| json!({}));
                kept["error"] = json!(error);
                kept
            }
        })
        .collect()
}

/// Reads the roster, keeping only fields the page shows and Muster's own labels for them.
fn read_roster() -> Value {
    let unavailable = |reason: &str| json!({"available": false, "reason": reason});
    let mut command = Command::new("muster-shim");
    command.arg("roster");
    let output = match run_with_timeout(command, COMMAND_LIMIT) {
        Ok(output) => output,
        Err(error) => {
            return unavailable(&format!(
                "muster-shim couldn't read the roster ({error}). This machine may need the \
                 rollout of Muster's MUS-44."
            ));
        }
    };
    match serde_json::from_slice::<Value>(&output) {
        Ok(value) => shape_roster(&value, now_ms()),
        Err(_) => unavailable("muster-shim's roster couldn't be read."),
    }
}

/// Keeps only the fields the page shows, in Muster's categories. A later format that drops a field
/// leaves it blank rather than guessed.
pub fn shape_roster(value: &Value, read_at_ms: u64) -> Value {
    let list = value["sessions"].as_array().or_else(|| value.as_array());
    let Some(list) = list else {
        return json!({"available": false, "reason": "muster-shim's roster has an unknown format."});
    };
    let sessions: Vec<Value> = list
        .iter()
        .map(|session| {
            let declared = &session["declared"];
            let attested = &session["attested"];
            json!({
                "project": declared["project"],
                "nickname": session["nickname"],
                "machine": attested["machine"],
                "wake": session["wake"],
                // Only `waiting` comes from the daemon. The other wake values are declared.
                "wake_observed": session["wake"] == "waiting",
                "current_task": declared["current_task"],
                "activity": declared["activity"],
                "last_activity_at": declared["last_activity_at"],
            })
        })
        .collect();
    json!({"available": true, "sessions": sessions, "read_at_ms": read_at_ms})
}

struct App {
    projects: Vec<Project>,
    snapshot: Mutex<Snapshot>,
    allowed_hosts: Vec<String>,
}

/// Runs the viewer until the process ends. Refuses anything but a loopback address.
pub async fn serve(
    listener: TcpListener,
    projects: Vec<Project>,
    ledger: LedgerReader,
) -> Result<(), Box<dyn std::error::Error>> {
    let address = listener.local_addr()?;
    let app = Arc::new(App {
        projects,
        snapshot: Mutex::new(Snapshot::default()),
        allowed_hosts: allowed_hosts(address)?,
    });
    let refresher = app.clone();
    std::thread::spawn(move || refresh_forever(&refresher, &ledger));

    let router = Router::new()
        .route(
            "/",
            get(|| async { page("text/html; charset=utf-8", INDEX) }),
        )
        .route("/app.js", get(|| async { page("text/javascript", APP_JS) }))
        .route("/app.css", get(|| async { page("text/css", APP_CSS) }))
        .route("/api/status", get(status))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app);
    listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(listener)?;
    axum::serve(listener, router).await?;
    Ok(())
}

fn allowed_hosts(address: SocketAddr) -> Result<Vec<String>, String> {
    if !address.ip().is_loopback() {
        return Err(format!(
            "the viewer only runs on a loopback address, not {address}: remote viewing needs its \
             own review (RAL-34)"
        ));
    }
    // The address it listens on, and `localhost` only for the addresses that name usually means.
    let mut hosts = vec![address.to_string()];
    let usual = address.ip() == std::net::Ipv4Addr::LOCALHOST
        || address.ip() == std::net::Ipv6Addr::LOCALHOST;
    if usual {
        hosts.push(format!("localhost:{}", address.port()));
    }
    Ok(hosts)
}

fn refresh_forever(app: &App, ledger: &LedgerReader) {
    let mut roadmaps_due = Instant::now();
    loop {
        if Instant::now() >= roadmaps_due {
            let previous = app.snapshot.lock().expect("no panics").roadmaps.clone();
            let roadmaps = refresh_roadmaps(&app.projects, &previous);
            app.snapshot.lock().expect("no panics").roadmaps = roadmaps;
            roadmaps_due = Instant::now() + ROADMAPS_EVERY;
        }
        let ledger_value = match ledger() {
            Ok(mut value) => {
                value["available"] = json!(true);
                value["read_at_ms"] = json!(now_ms());
                value
            }
            Err(reason) => json!({"available": false, "reason": reason}),
        };
        let roster = read_roster();
        {
            let mut snapshot = app.snapshot.lock().expect("no panics");
            snapshot.ledger = Some(ledger_value);
            snapshot.roster = Some(roster);
        }
        std::thread::sleep(LIVE_EVERY);
    }
}

fn page(content_type: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

async fn status(State(app): State<Arc<App>>) -> Response {
    let snapshot = app.snapshot.lock().expect("no panics");
    let ledger = snapshot
        .ledger
        .clone()
        .unwrap_or_else(|| json!({"available": false, "reason": "still reading it."}));
    let roster = snapshot
        .roster
        .clone()
        .unwrap_or_else(|| json!({"available": false, "reason": "still reading it."}));
    let projects: Vec<Value> = app
        .projects
        .iter()
        .enumerate()
        .map(|(index, project)| {
            let mut row = snapshot
                .roadmaps
                .get(index)
                .cloned()
                .unwrap_or_else(|| json!({}));
            row["name"] = json!(project.name);
            row["prefix"] = json!(project.prefix);
            row["repo"] = json!(project.repo);
            row["path"] = json!(project.path);
            row["tick_meaning"] = json!(project.tick_meaning);
            row["verified"] = verified_for(&project.prefix, &ledger);
            row
        })
        .collect();
    Json(json!({
        "untrusted_text": UNTRUSTED_TEXT,
        "ledger": ledger,
        "roster": roster,
        "projects": projects,
    }))
    .into_response()
}

async fn guard(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    let refused = |status: StatusCode, message: &'static str| (status, message).into_response();
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let mut response = if !app.allowed_hosts.iter().any(|allowed| allowed == host) {
        refused(
            StatusCode::FORBIDDEN,
            "this page is only served to its own address",
        )
    } else if !origin_allowed(request.headers().get(header::ORIGIN), host) {
        refused(StatusCode::FORBIDDEN, "other sites can't read this page")
    } else if !matches!(*request.method(), Method::GET | Method::HEAD) {
        refused(StatusCode::METHOD_NOT_ALLOWED, "this page is read-only")
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn origin_allowed(origin: Option<&HeaderValue>, host: &str) -> bool {
    origin.is_none_or(|origin| origin.as_bytes() == format!("http://{host}").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_ticks_overall_and_by_section() {
        let text = "# Title\n\n## One\n- [x] a\n- [ ] b\n  - [X] nested\n\n## Two\n* [ ] c\n";
        let (boxes, sections) = parse_roadmap(text);
        assert_eq!(
            boxes,
            Boxes {
                ticked: 2,
                total: 4,
                percent: 50
            }
        );
        assert_eq!(sections.len(), 2);
        assert_eq!((sections[0].ticked, sections[0].total), (2, 3));
        assert_eq!(sections[1].name, "Two");
    }

    #[test]
    fn ignores_examples_in_code_fences_and_things_that_only_look_like_boxes() {
        let text = "```markdown\n- [x] example\n```\n- [link](url)\n- [] no\nnot - [x] a box\n- [ ] real\n";
        let (boxes, _) = parse_roadmap(text);
        assert_eq!((boxes.ticked, boxes.total), (0, 1));
    }

    #[test]
    fn an_unfinished_roadmap_never_rounds_up_to_100() {
        let text = format!("{}- [ ] last\n", "- [x] done\n".repeat(999));
        assert_eq!(parse_roadmap(&text).0.percent, 99);
    }

    #[test]
    fn an_empty_roadmap_has_no_percentage_to_divide() {
        assert_eq!(parse_roadmap("# nothing").0, Boxes::default());
    }

    #[test]
    fn verified_comes_only_from_the_ledger() {
        let none = json!({"available": false});
        assert_eq!(verified_for("RAL", &none)["available"], false);

        let ledger = json!({"stories": {
            "RAL-1": {"state": "verified"},
            "RAL-2": {"state": "in_review"},
            "RALLY-3": {"state": "verified"},
            "MUS-1": {"state": "verified"},
        }});
        let figure = verified_for("RAL", &ledger);
        assert_eq!(figure["verified"], 1);
        assert_eq!(figure["tracked"], 2);
        assert_eq!(verified_for("CAT", &ledger)["available"], false);
    }

    #[test]
    fn the_roster_keeps_only_what_the_page_shows() {
        let roster = json!({"sessions": [{
            "wake": "waiting",
            "nickname": "x",
            "observed": {"waiting": true},
            "attested": {"machine": "a1", "operator": "someone@example.com"},
            "declared": {"project": "rally", "current_task": "RAL-1",
                         "activity": "busy", "last_activity_at": 5, "cwd": "/secret"},
        }]});
        let shaped = shape_roster(&roster, 9);
        let text = shaped.to_string();
        assert!(!text.contains("/secret"), "{text}");
        assert!(!text.contains("example.com"), "{text}");
        assert_eq!(shaped["sessions"][0]["machine"], "a1");
        assert_eq!(shaped["sessions"][0]["nickname"], "x");
        assert_eq!(shaped["sessions"][0]["wake_observed"], true);
        let declared_wake = json!({"sessions": [{"wake": "channel", "declared": {}}]});
        assert_eq!(
            shape_roster(&declared_wake, 9)["sessions"][0]["wake_observed"],
            false
        );
        assert_eq!(shaped["sessions"][0]["current_task"], "RAL-1");
        assert_eq!(shape_roster(&json!("nope"), 9)["available"], false);
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_command_is_given_up_on() {
        let mut command = Command::new("sleep");
        command.arg("30");
        let started = Instant::now();
        let error = run_with_timeout(command, Duration::from_millis(100)).unwrap_err();
        assert!(error.contains("didn't finish"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn a_command_with_a_lot_of_output_finishes() {
        let mut command = Command::new("sh");
        command.args(["-c", "head -c 1000000 /dev/zero"]);
        let output = run_with_timeout(command, Duration::from_secs(10)).unwrap();
        assert_eq!(output.len(), 1_000_000);
    }

    #[cfg(unix)]
    #[test]
    fn something_left_holding_the_output_open_is_given_up_on() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & exit 0"]);
        let started = Instant::now();
        let error = run_with_timeout(command, Duration::from_millis(300)).unwrap_err();
        assert!(error.contains("output"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_command_is_an_error() {
        let mut command = Command::new("sh");
        command.args(["-c", "exit 3"]);
        assert!(run_with_timeout(command, Duration::from_secs(5)).is_err());
    }

    #[test]
    fn a_command_that_prints_too_much_is_refused() {
        let mut command = Command::new("sh");
        command.args(["-c", "head -c 9000000 /dev/zero"]);
        let error = run_with_timeout(command, Duration::from_secs(10)).unwrap_err();
        assert!(error.contains("too much"), "{error}");
    }

    #[test]
    fn fences_open_and_close_as_markdown_says() {
        let count = |text: &str| parse_roadmap(text).0;
        // A longer fence isn't closed by a shorter one.
        let longer = "````\n```\n- [x] inside\n````\n- [ ] outside\n";
        assert_eq!((count(longer).ticked, count(longer).total), (0, 1));
        // Nor by the other kind.
        let other = "```\n~~~\n- [x] inside\n```\n- [ ] outside\n";
        assert_eq!((count(other).ticked, count(other).total), (0, 1));
        // A closing fence can't have text after it.
        let text_after = "```\n``` not a close\n- [x] inside\n```\n- [ ] outside\n";
        assert_eq!((count(text_after).ticked, count(text_after).total), (0, 1));
        // Four spaces of indent is code, not a fence, so what follows still counts.
        let indented = "    ```\n- [x] counted\n";
        assert_eq!((count(indented).ticked, count(indented).total), (1, 1));
        // An info string is fine on an opening fence.
        let info = "```rust\n- [x] inside\n```\n- [ ] outside\n";
        assert_eq!((count(info).ticked, count(info).total), (0, 1));
    }

    #[test]
    fn each_loopback_address_allows_its_own_host() {
        let hosts = allowed_hosts("127.0.0.2:7300".parse().unwrap()).unwrap();
        assert!(hosts.contains(&"127.0.0.2:7300".to_owned()), "{hosts:?}");
        assert!(!hosts.contains(&"localhost:7300".to_owned()), "{hosts:?}");
        assert!(!hosts.contains(&"127.0.0.1:7300".to_owned()), "{hosts:?}");
        let hosts = allowed_hosts("127.0.0.1:7300".parse().unwrap()).unwrap();
        assert_eq!(hosts, ["127.0.0.1:7300", "localhost:7300"]);
        let hosts = allowed_hosts("[::1]:7300".parse().unwrap()).unwrap();
        assert_eq!(hosts, ["[::1]:7300", "localhost:7300"]);
    }

    #[test]
    fn only_loopback_addresses_are_allowed() {
        assert!(allowed_hosts("127.0.0.1:7300".parse().unwrap()).is_ok());
        assert!(allowed_hosts("[::1]:7300".parse().unwrap()).is_ok());
        assert!(allowed_hosts("192.168.1.5:7300".parse().unwrap()).is_err());
        assert!(allowed_hosts("0.0.0.0:7300".parse().unwrap()).is_err());
    }

    #[test]
    fn the_page_never_builds_markup_from_text() {
        for banned in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "eval(",
        ] {
            assert!(!APP_JS.contains(banned), "{banned}");
        }
        assert!(!INDEX.contains("<script>"), "no inline script");
        assert!(!INDEX.contains("style="), "no inline style");
        assert!(!INDEX.contains("http://") && !INDEX.contains("https://"));
    }

    #[test]
    fn the_default_projects_can_be_read() {
        let projects: Vec<Project> = serde_json::from_str(DEFAULT_PROJECTS).unwrap();
        assert!(projects.iter().any(|project| project.prefix == "RAL"));
    }
}
