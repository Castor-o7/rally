//! Runs the real service over TLS and tries each route with and without access.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rally::access::Pairing;
use rally::client::{Reply, request};
use rally::scratch_pg::{self, Cluster, Database};
use serde_json::{Value, json};

const ROUTES: [(&str, &str); 13] = [
    ("POST", "/v1/records"),
    ("GET", "/v1/stories"),
    ("GET", "/v1/stories/RAL-13"),
    ("GET", "/v1/teams"),
    ("GET", "/v1/teams/rally"),
    ("GET", "/v1/check"),
    ("GET", "/v1/whoami"),
    ("GET", "/v1/designation"),
    ("GET", "/v1/verifications"),
    ("GET", "/v1/waiting"),
    ("GET", "/v1/handover"),
    ("GET", "/v1/brief"),
    ("GET", "/v1/teams/rally/record"),
];

struct Service<'a> {
    child: Child,
    // Dropped after the child is killed, in `Drop`.
    db: Database<'a>,
    dir: tempfile::TempDir,
    address: String,
    fingerprint: String,
    limits: Vec<String>,
}

impl Drop for Service<'_> {
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
        "RALLY_STATE_DIR",
        "RALLY_TEAM",
        "RALLY_HARNESS",
        "RALLY_OPERATOR_WINDOW",
        "RALLY_OPERATOR_AWAY_AFTER",
        "RALLY_OPERATOR_AWAY",
    ] {
        command.env_remove(name);
    }
    command
}

/// Starts the service, and returns it with the address it's listening on and its fingerprint.
fn spawn(db: &Database, dir: &Path, limits: &[String]) -> (Child, String, String) {
    let mut child = rally()
        .arg("--db")
        .arg(&db.url)
        .arg("--head-file")
        .arg(dir.join("ledger.head"))
        .args(["serve", "--bind", "127.0.0.1:0"])
        .args(limits)
        .arg("--state-dir")
        .arg(dir.join("state"))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let started: Value = serde_json::from_str(&line).unwrap();
    // It prints its address before replaying, so wait until it answers.
    let address = started["listening"].as_str().unwrap().to_owned();
    for _ in 0..600 {
        if std::net::TcpStream::connect(&address).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    (
        child,
        address,
        started["fingerprint"].as_str().unwrap().to_owned(),
    )
}

fn start_on<'a>(cluster: &'a Cluster, limits: &[&str]) -> Service<'a> {
    let dir = tempfile::tempdir().unwrap();
    let db = cluster.database();
    let limits: Vec<String> = limits.iter().map(|s| (*s).to_owned()).collect();
    let (child, address, fingerprint) = spawn(&db, dir.path(), &limits);
    Service {
        child,
        db,
        dir,
        address,
        fingerprint,
        limits,
    }
}

fn start_with(limits: &[&str]) -> Service<'static> {
    start_on(scratch_pg::shared(), limits)
}

const GENEROUS: [&str; 8] = [
    "--burst",
    "1000",
    "--write-burst",
    "1000",
    "--heavy-burst",
    "1000",
    "--min-free-bytes",
    "0",
];

fn start() -> Service<'static> {
    start_with(&GENEROUS)
}

impl Service<'_> {
    /// A real restart: the process ends, and the new one replays the ledger from Postgres.
    fn restart(&mut self, signal: &str) {
        let pid = self.child.id().to_string();
        assert!(
            Command::new("kill")
                .args([signal, &pid])
                .status()
                .unwrap()
                .success()
        );
        self.child.wait().unwrap();
        let (child, address, fingerprint) = spawn(&self.db, self.dir.path(), &self.limits);
        assert_eq!(fingerprint, self.fingerprint, "the certificate changed");
        self.child = child;
        self.address = address;
    }

    /// Tries to start it, and returns why it wouldn't.
    fn refuses_to_start(&self) -> String {
        let output = rally()
            .arg("--db")
            .arg(&self.db.url)
            .arg("--head-file")
            .arg(self.head_file())
            .args(["serve", "--bind", "127.0.0.1:0"])
            .arg("--state-dir")
            .arg(self.dir.path().join("state"))
            .output()
            .unwrap();
        assert!(!output.status.success(), "it started");
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    /// Starts it again after it was stopped.
    fn restart_after_stop(&mut self) {
        let (child, address, _) = spawn(&self.db, self.dir.path(), &self.limits);
        self.child = child;
        self.address = address;
    }

    fn head_file(&self) -> PathBuf {
        self.dir.path().join("ledger.head")
    }

    fn state_dir(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    fn add(&self, name: &str, read_only: bool) -> Pairing {
        let out = self.dir.path().join(format!("{name}.pair"));
        let mut command = rally();
        command.args(["add-machine", name, "--address", &self.address]);
        command.arg("--state-dir").arg(self.state_dir());
        command.arg("--out").arg(&out);
        if read_only {
            command.arg("--read-only");
        }
        let output = command.output().unwrap();
        assert!(output.status.success());
        let code = std::fs::read_to_string(&out).unwrap();
        let pairing = Pairing::parse(&code).unwrap();
        let printed = String::from_utf8_lossy(&output.stdout);
        assert!(!printed.contains(&pairing.token), "the token was printed");
        pairing
    }

    fn wrong_token(&self) -> Pairing {
        Pairing {
            address: self.address.clone(),
            fingerprint: self.fingerprint.clone(),
            token: "not-a-real-token".into(),
        }
    }
}

fn call(pairing: &Pairing, method: &str, path: &str, body: Option<&str>) -> Reply {
    request(pairing, method, path, body, &[]).unwrap()
}

fn write(pairing: &Pairing, name: &str, kind: &str, body: Value) -> Reply {
    let text = json!({"kind": kind, "declared_name": name, "body": body}).to_string();
    call(pairing, "POST", "/v1/records", Some(&text))
}

fn ready(pairing: &Pairing) -> Reply {
    write(
        pairing,
        "someone",
        "declared",
        json!({"change": "ready", "story": "RAL-13"}),
    )
}

#[test]
fn a_paired_machine_writes_and_the_service_sets_credential_and_time() {
    let service = start();
    let a1 = service.add("a1", false);
    assert_eq!(ready(&a1).status, 200);

    let reply = call(&a1, "GET", "/v1/stories/RAL-13", None);
    assert_eq!(reply.status, 200);
    let shown: Value = serde_json::from_str(&reply.body).unwrap();
    let entry = &shown["detail"]["history"][0];
    assert_eq!(entry["source"]["credential"], "a1");
    assert_eq!(entry["source"]["declared_name"], "someone");
    assert_eq!(entry["kind"], "declared");
    assert!(entry["at_ms"].as_i64().unwrap() > 1_000_000_000_000);
}

#[test]
fn a_client_cannot_choose_its_credential_or_time() {
    let service = start();
    let a1 = service.add("a1", false);
    let text = json!({
        "kind": "declared", "declared_name": "x",
        "body": {"change": "ready", "story": "RAL-13"},
        "credential": "murph", "at_ms": 1
    })
    .to_string();
    let reply = call(&a1, "POST", "/v1/records", Some(&text));
    assert_eq!(reply.status, 400, "{}", reply.body);
}

#[test]
fn every_route_refuses_no_token_or_a_wrong_token_and_changes_nothing() {
    // Two wrong tokens per route would pass the default failed-token burst of 20.
    let mut limits = GENEROUS.to_vec();
    limits.extend(["--failed-token-burst", "100"]);
    let service = start_with(&limits);
    let a1 = service.add("a1", false);
    let body = json!({"kind": "declared", "declared_name": "x",
                      "body": {"change": "ready", "story": "RAL-13"}})
    .to_string();
    let mut wrong = service.wrong_token();
    for (method, path) in ROUTES {
        wrong.token = "not-a-real-token".into();
        assert_eq!(
            call(&wrong, method, path, Some(&body)).status,
            401,
            "{path}"
        );
        wrong.token = String::new();
        assert_eq!(
            call(&wrong, method, path, Some(&body)).status,
            401,
            "{path}"
        );
    }
    // Routes that don't exist are refused the same way, so they reveal nothing.
    wrong.token = "not-a-real-token".into();
    assert_eq!(call(&wrong, "GET", "/nope", None).status, 401);

    let all: Value = serde_json::from_str(&call(&a1, "GET", "/v1/stories", None).body).unwrap();
    assert_eq!(all["as_of"], 0);
}

#[test]
fn every_route_refuses_a_web_page() {
    let service = start();
    let a1 = service.add("a1", false);
    let origin = [("Origin", "https://evil.example")];
    for (method, path) in ROUTES {
        let reply = request(&a1, method, path, Some("{}"), &origin).unwrap();
        assert_eq!(reply.status, 403, "{path}");
    }
}

#[test]
fn a_read_only_credential_reads_but_cannot_write() {
    let service = start();
    let a1 = service.add("a1", false);
    let viewer = service.add("viewer", true);
    assert_eq!(ready(&a1).status, 200);

    assert_eq!(ready(&viewer).status, 403);
    assert_eq!(call(&viewer, "GET", "/v1/stories", None).status, 200);
    let all: Value = serde_json::from_str(&call(&a1, "GET", "/v1/stories", None).body).unwrap();
    assert_eq!(all["as_of"], 1);
}

#[test]
fn a_read_only_credential_is_refused_on_any_method_but_get() {
    let service = start();
    let viewer = service.add("viewer", true);
    for (method, path) in [
        ("DELETE", "/v1/stories"),
        ("PUT", "/v1/teams"),
        ("POST", "/nope"),
    ] {
        assert_eq!(
            call(&viewer, method, path, Some("{}")).status,
            403,
            "{method} {path}"
        );
    }
}

#[test]
fn a_credential_can_tell_whether_it_is_read_only() {
    let service = start();
    let viewer = service.add("viewer", true);
    let a1 = service.add("a1", false);
    let who = |pairing| -> Value {
        serde_json::from_str(&call(pairing, "GET", "/v1/whoami", None).body).unwrap()
    };
    assert_eq!(who(&viewer)["read_only"], true);
    assert_eq!(who(&a1)["read_only"], false);
    assert_eq!(who(&a1)["credential"], "a1");
}

#[test]
fn only_rally_records_observed_facts() {
    let service = start();
    let a1 = service.add("a1", false);
    let reply = write(
        &a1,
        "x",
        "observed",
        json!({"change": "ready", "story": "RAL-13"}),
    );
    assert_eq!(reply.status, 403);
}

#[test]
fn a_refused_change_says_why_and_a_bad_request_is_refused() {
    let service = start();
    let a1 = service.add("a1", false);
    let claim = json!({"change": "claim", "story": "RAL-13", "team": "rally"});
    let reply = write(&a1, "x", "declared", claim);
    assert_eq!(reply.status, 409);
    assert!(reply.body.contains("isn't in the ledger"), "{}", reply.body);
    assert_eq!(
        call(&a1, "POST", "/v1/records", Some("not json")).status,
        400
    );
    assert_eq!(call(&a1, "DELETE", "/v1/stories", None).status, 405);
    assert_eq!(call(&a1, "GET", "/nope", None).status, 404);
}

#[test]
fn a_revoked_machine_is_refused_at_once() {
    let service = start();
    let a1 = service.add("a1", false);
    assert_eq!(call(&a1, "GET", "/v1/stories", None).status, 200);
    let status = rally()
        .args(["revoke", "a1", "--state-dir"])
        .arg(service.state_dir())
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(call(&a1, "GET", "/v1/stories", None).status, 401);
}

#[test]
fn an_oversized_body_is_refused() {
    let service = start();
    let a1 = service.add("a1", false);
    let big = "x".repeat(rally::server::MAX_BODY_BYTES + 1);
    let reply = request(&a1, "POST", "/v1/records", Some(&big), &[]);
    // The service may close the connection while the rest is still being sent.
    if let Ok(reply) = reply {
        assert_eq!(reply.status, 413);
    }
    let all: Value = serde_json::from_str(&call(&a1, "GET", "/v1/stories", None).body).unwrap();
    assert_eq!(all["as_of"], 0);
}

#[test]
fn a_flooding_credential_is_limited_without_affecting_another() {
    let service = start_with(&[
        "--burst",
        "5",
        "--per-second",
        "0.001",
        "--min-free-bytes",
        "0",
    ]);
    let a1 = service.add("a1", false);
    let murph = service.add("murph", false);
    let statuses: Vec<u16> = (0..7)
        .map(|_| call(&a1, "GET", "/v1/teams", None).status)
        .collect();
    assert_eq!(statuses[..5], [200; 5]);
    assert_eq!(statuses[5..], [429; 2]);
    assert_eq!(call(&murph, "GET", "/v1/teams", None).status, 200);
}

#[test]
fn writes_have_their_own_lower_limit_and_reads_keep_working() {
    let service = start_with(&[
        "--write-burst",
        "2",
        "--write-per-second",
        "0.001",
        "--min-free-bytes",
        "0",
    ]);
    let a1 = service.add("a1", false);
    let mark = |story: &str| {
        write(
            &a1,
            "x",
            "declared",
            json!({"change": "ready", "story": story}),
        )
        .status
    };
    assert_eq!([mark("RAL-1"), mark("RAL-2")], [200, 200]);
    assert_eq!(mark("RAL-3"), 429);
    assert_eq!(call(&a1, "GET", "/v1/stories", None).status, 200);
    let murph = service.add("murph", false);
    assert_eq!(ready(&murph).status, 200);
}

#[test]
fn a_full_ledger_refuses_writes_but_can_be_read() {
    let service = start_with(&["--max-ledger-bytes", "1", "--min-free-bytes", "0"]);
    let a1 = service.add("a1", false);
    assert_eq!(ready(&a1).status, 507);
    assert_eq!(call(&a1, "GET", "/v1/stories", None).status, 200);
}

#[test]
fn add_machine_leaves_nothing_behind_when_it_fails() {
    let service = start();
    let taken = service.dir.path().join("taken.pair");
    std::fs::write(&taken, "keep me").unwrap();
    let add = |out: &Path, address: &str| {
        rally()
            .args(["add-machine", "a1", "--address", address, "--state-dir"])
            .arg(service.state_dir())
            .arg("--out")
            .arg(out)
            .output()
            .unwrap()
    };
    assert!(!add(&taken, "x:1").status.success());
    assert_eq!(std::fs::read_to_string(&taken).unwrap(), "keep me");

    let bad = service.dir.path().join("bad.pair");
    assert!(!add(&bad, "a/b:1").status.success());
    assert!(!bad.exists());

    let missing = service.dir.path().join("no/such/dir/a1.pair");
    assert!(!add(&missing, "x:1").status.success());
    // The name wasn't taken by any of those attempts.
    let good = service.dir.path().join("good.pair");
    assert!(add(&good, "x:1").status.success());
}

#[test]
fn low_disk_space_refuses_writes_and_says_why_but_reads_carry_on() {
    let service = start_with(&["--min-free-bytes", "18446744073709551615"]);
    let a1 = service.add("a1", false);
    let reply = ready(&a1);
    assert_eq!(reply.status, 507);
    assert!(reply.body.contains("low on free space"), "{}", reply.body);
    let all = call(&a1, "GET", "/v1/stories", None);
    assert_eq!(all.status, 200);
    let all: Value = serde_json::from_str(&all.body).unwrap();
    assert_eq!(all["as_of"], 0);
}

#[test]
fn writes_work_above_the_disk_floor() {
    let service = start_with(&["--min-free-bytes", "1"]);
    let a1 = service.add("a1", false);
    assert_eq!(ready(&a1).status, 200);
}

#[test]
fn a_client_refuses_a_service_with_the_wrong_certificate() {
    let service = start();
    let mut a1 = service.add("a1", false);
    a1.fingerprint = "0".repeat(64);
    let error = request(&a1, "GET", "/v1/stories", None, &[]).unwrap_err();
    assert!(
        error.to_string().contains("pairing") || error.to_string().contains("certificate"),
        "{error}"
    );
}

fn pair(config: &Path, code: &str) -> std::process::Output {
    let mut child = rally()
        .arg("--config")
        .arg(config)
        .arg("pair")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(code.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn cli(config: &Path, name: &str, args: &[&str]) -> std::process::Output {
    rally()
        .arg("--config")
        .arg(config)
        .args(["--as", name])
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn a_backfilled_entry_gets_the_services_time_not_the_entrys() {
    let service = start();
    let pairing = service.add("a1", false);
    let config = service.dir.path().join("client/config.json");
    assert!(pair(&config, &pairing.code()).status.success());
    let entered = cli(
        &config,
        "watch",
        &[
            "--entered-at-ms",
            "1",
            "--entered-by",
            "bob",
            "ready",
            "RAL-30",
        ],
    );
    assert!(
        entered.status.success(),
        "{}",
        String::from_utf8_lossy(&entered.stderr)
    );
    let shown = cli(&config, "x", &["show", "RAL-30"]);
    let shown: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    let entry = &shown["detail"]["history"][0];
    assert!(entry["at_ms"].as_i64().unwrap() > 1_000_000_000_000);
    assert_eq!(entry["backfilled"]["entered_at_ms"], 1);
    assert_eq!(entry["backfilled"]["entered_by"], "bob");
    assert_eq!(entry["source"]["declared_name"], "watch");
}

#[test]
fn the_command_works_end_to_end_through_the_service() {
    let service = start();
    let pairing = service.add("a1", false);
    let config = service.dir.path().join("client/config.json");

    let unpaired = cli(&config, "x", &["show"]);
    assert!(!unpaired.status.success());
    assert!(String::from_utf8_lossy(&unpaired.stderr).contains("rally pair"));

    let paired = pair(&config, &pairing.code());
    assert!(paired.status.success());
    let printed = String::from_utf8_lossy(&paired.stdout);
    assert!(!printed.contains(&pairing.token), "the token was printed");
    for args in [
        vec!["ready", "RAL-13"],
        vec!["claim", "RAL-13", "--team", "rally"],
        vec!["team-state", "rally", "working", "--story", "RAL-13"],
    ] {
        let output = cli(&config, "silken-tanager", &args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let refused = cli(
        &config,
        "silken-tanager",
        &["claim", "RAL-13", "--team", "rally"],
    );
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("refused (409)"));

    let teams = cli(&config, "gleaming-tench", &["teams"]);
    let teams: Value = serde_json::from_slice(&teams.stdout).unwrap();
    assert_eq!(teams["teams"]["rally"]["status"], "working");
    assert_eq!(teams["teams"]["rally"]["source"]["credential"], "a1");

    let then = cli(&config, "x", &["show", "RAL-13", "--as-of", "1"]);
    let then: Value = serde_json::from_slice(&then.stdout).unwrap();
    assert_eq!(then["detail"]["state"], "ready");
}

#[test]
fn check_works_through_the_service_and_a_saved_head_is_checked() {
    let service = start();
    let a1 = service.add("a1", false);
    let viewer = service.add("viewer", true);
    assert_eq!(ready(&a1).status, 200);

    let reply = call(&viewer, "GET", "/v1/check", None);
    let first: Value = serde_json::from_str(&reply.body).unwrap();
    assert_eq!(first["ok"], true, "{}", reply.body);
    assert_eq!(first["head"]["seq"], 1);
    // Problems can quote what an agent wrote (RAL-41).
    assert!(
        first["untrusted_text"]
            .as_str()
            .unwrap()
            .contains("not instructions")
    );
    let hash = first["head"]["hash"].as_str().unwrap();

    let good = format!("/v1/check?seq=1&hash={hash}");
    let good: Value = serde_json::from_str(&call(&a1, "GET", &good, None).body).unwrap();
    assert_eq!(good["ok"], true);

    let ahead = format!("/v1/check?seq=2&hash={hash}");
    let ahead: Value = serde_json::from_str(&call(&a1, "GET", &ahead, None).body).unwrap();
    assert_eq!(ahead["ok"], false);

    assert_eq!(call(&a1, "GET", "/v1/check?seq=1", None).status, 400);
}

#[cfg(unix)]
#[test]
fn the_saved_pairing_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let service = start();
    let pairing = service.add("a1", false);
    let config = service.dir.path().join("client/config.json");
    let paired = pair(&config, &pairing.code());
    assert!(paired.status.success());
    let printed = String::from_utf8_lossy(&paired.stdout);
    assert!(!printed.contains(&pairing.token), "the token was printed");
    let mode = std::fs::metadata(config).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

/// Every TCP and UDP socket the process has open, as `lsof` lists them.
fn sockets(pid: u32, listening_only: bool) -> Vec<String> {
    let mut command = Command::new("lsof");
    command.args(["-nP", "-a", "-p", &pid.to_string()]);
    if listening_only {
        command.args(["-iTCP", "-sTCP:LISTEN"]);
    } else {
        command.arg("-i");
    }
    let output = command.output().expect("lsof is needed to inspect sockets");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .map(|line| {
            line.split_whitespace()
                .skip(7)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

#[test]
fn nothing_listens_but_the_door() {
    let service = start();
    let a1 = service.add("a1", false);
    // Wait until it has replayed and served, so every socket it will open is open.
    assert_eq!(ready(&a1).status, 200);
    let listening = sockets(service.child.id(), true);
    assert_eq!(listening.len(), 1, "{listening:?}");
    assert!(
        listening[0].contains(&format!("TCP {} (LISTEN)", service.address)),
        "{listening:?}"
    );
    // Every other socket is a connection out to Postgres; none is UDP.
    let port = service
        .db
        .url
        .rsplit_once(':')
        .unwrap()
        .1
        .split('/')
        .next()
        .unwrap();
    for socket in sockets(service.child.id(), false) {
        let to_postgres =
            socket.starts_with("TCP ") && socket.contains(&format!("->127.0.0.1:{port} "));
        assert!(
            socket.contains("(LISTEN)") || to_postgres,
            "unexpected socket: {socket}"
        );
    }
}

#[test]
fn myko_cant_be_reached_through_the_door() {
    // Raw events and the generated DeleteLedgerRecord need a myko connection. The door has no
    // route to one, so they can't arrive: with a valid token, myko's paths are simply missing.
    let service = start();
    let a1 = service.add("a1", false);
    assert_eq!(ready(&a1).status, 200);
    for (method, path) in [
        ("GET", "/myko"),
        ("GET", "/myko/mcp"),
        ("POST", "/myko/mcp"),
        ("POST", "/v1/records/1"),
        ("DELETE", "/v1/records"),
    ] {
        let status = call(&a1, method, path, Some("{}")).status;
        assert!(matches!(status, 404 | 405), "{method} {path}: {status}");
    }
    let event = json!({
        "item": {"id": "1", "record": "forged", "hash": "0".repeat(64)},
        "changeType": "DEL", "itemType": "LedgerRecord"
    })
    .to_string();
    assert_eq!(call(&a1, "POST", "/v1/records", Some(&event)).status, 400);
    let check: Value = serde_json::from_str(&call(&a1, "GET", "/v1/check", None).body).unwrap();
    assert_eq!(check["ok"], true);
    assert_eq!(check["head"]["seq"], 1);
}

fn host_check(service: &Service, expect: Option<&str>) -> std::process::Output {
    let mut command = rally();
    command.arg("--db").arg(&service.db.url);
    command.arg("--head-file").arg(service.head_file());
    command.arg("check");
    if let Some(expect) = expect {
        command.args(["--expect", expect]);
    }
    command.output().unwrap()
}

fn stopped<'a>(service: &mut Service<'a>) {
    let _ = service.child.kill();
    let _ = service.child.wait();
}

/// Writes rows as myko itself would for an event that reached it: a new version of a record, or a
/// deletion. Rally's table refuses both, so this lifts the protections first, as someone with the
/// database's owner credentials could.
/// A record number, `SET` or `DEL`, and for a `SET` the record and its hash.
type MykoRow<'a> = (u64, &'a str, Option<(&'a str, &'a str)>);

fn as_myko_would(service: &Service, rows: &[MykoRow]) {
    let mut sql = service.db.connect();
    sql.batch_execute(
        "DROP INDEX rally_records_once;
         ALTER TABLE rally_events DISABLE TRIGGER rally_only_sets;",
    )
    .unwrap();
    for (seq, change, item) in rows {
        let item = match item {
            Some((record, hash)) => json!({"id": seq.to_string(), "record": record, "hash": hash}),
            None => json!({"id": seq.to_string()}),
        };
        let event = json!({
            "item": item, "changeType": change, "itemType": "LedgerRecord",
            "createdAt": "2026-11-03T00:00:00Z", "tx": "forged", "sourceId": "forger"
        });
        sql.execute(
            "INSERT INTO rally_events (item_type, item_id, change_type, tx, source_id, event)
             VALUES ('LedgerRecord', $1, $2, 'forged', 'forger', ($3::text)::jsonb)",
            &[&seq.to_string(), change, &event.to_string()],
        )
        .unwrap();
    }
}

fn chain_hash(previous: &str, seq: u64, record: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(format!("{previous}\n{seq}\n{record}").as_bytes());
    hex::encode(hash.finalize())
}

fn stored(service: &Service, seq: u64) -> (String, String) {
    let row = service
        .db
        .connect()
        .query_one(
            "SELECT event->'item'->>'record', event->'item'->>'hash' FROM rally_events
             WHERE item_type = 'LedgerRecord' AND item_id = $1 ORDER BY id LIMIT 1",
            &[&seq.to_string()],
        )
        .unwrap();
    (row.get(0), row.get(1))
}

#[test]
fn the_deciding_test_three_real_records_a_saved_head_a_real_restart_and_check() {
    let mut service = start();
    let pairing = service.add("a1", false);
    let config = service.dir.path().join("client/config.json");
    assert!(pair(&config, &pairing.code()).status.success());
    for args in [
        vec!["ready", "RAL-39"],
        vec!["claim", "RAL-39", "--team", "rally"],
        vec![
            "submit",
            "RAL-39",
            "--team",
            "rally",
            "--evidence",
            "Castor-o7/rally abc",
        ],
    ] {
        let output = cli(&config, "silken-tanager", &args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // The submission's evidence check is Rally's own record 4 (RAL-5): wait for it, so the saved
    // head is the last one.
    let mut checked = Value::Null;
    for _ in 0..50 {
        let output = cli(&config, "x", &["check"]);
        assert!(output.status.success());
        checked = serde_json::from_slice(&output.stdout).unwrap();
        if checked["head"]["seq"] == 4 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(checked["head"]["seq"], 4, "{checked}");
    let head = format!("4:{}", checked["head"]["hash"].as_str().unwrap());

    for signal in ["-TERM", "-KILL"] {
        service.restart(signal);
        // The pairing names the old port; the restarted service has a new one.
        let mut moved = pairing.clone();
        moved.address = service.address.clone();
        assert!(pair(&config, &moved.code()).status.success());
        let output = cli(&config, "x", &["check", "--expect", &head]);
        assert!(
            output.status.success(),
            "{signal}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let shown = cli(&config, "x", &["show", "RAL-39"]);
        let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
        assert_eq!(shown["detail"]["state"], "in_review", "{signal}");
        // On the host, read-only from Postgres, without myko, while the service runs.
        let output = host_check(&service, Some(&head));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Rewriting record 2 and re-signing 3 and 4, as events through myko's store would.
    stopped(&mut service);
    let (text2, _) = stored(&service, 2);
    let (text3, _) = stored(&service, 3);
    let (text4, _) = stored(&service, 4);
    assert!(text2.contains("silken-tanager"));
    let (_, hash1) = stored(&service, 1);
    let forged2 = text2.replace("silken-tanager", "mallory");
    let hash2 = chain_hash(&hash1, 2, &forged2);
    let hash3 = chain_hash(&hash2, 3, &text3);
    let hash4 = chain_hash(&hash3, 4, &text4);
    as_myko_would(
        &service,
        &[
            (2, "SET", Some((&forged2, &hash2))),
            (3, "SET", Some((&text3, &hash3))),
            (4, "SET", Some((&text4, &hash4))),
        ],
    );
    let forged_head = format!("4:{hash4}");
    std::fs::write(
        service.head_file(),
        json!({"seq": 4, "hash": hash4}).to_string(),
    )
    .unwrap();
    // A writer won't start over a record with two versions, so this ledger can't grow.
    assert!(service.refuses_to_start().contains("more than one version"));
    let output = host_check(&service, Some(&head));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("rewritten"));
    // The history shows it even without a saved head, while the old rows are there.
    let output = host_check(&service, Some(&forged_head));
    assert!(String::from_utf8_lossy(&output.stderr).contains("versions in Postgres"));

    // With the old rows gone too, only the saved head shows it.
    stopped(&mut service);
    let mut sql = service.db.connect();
    sql.batch_execute(
        "ALTER TABLE rally_events DISABLE TRIGGER rally_no_delete;
         DELETE FROM rally_events WHERE tx <> 'forged' AND item_id IN ('2', '3', '4');",
    )
    .unwrap();
    service.restart_after_stop();
    let fooled = host_check(&service, None);
    assert!(
        fooled.status.success(),
        "fooled without a saved head: {}",
        String::from_utf8_lossy(&fooled.stderr)
    );
    let output = host_check(&service, Some(&head));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("rewritten"));
}

#[test]
fn truncation_through_the_store_is_caught_by_a_saved_head() {
    let mut service = start();
    let a1 = service.add("a1", false);
    for story in ["RAL-1", "RAL-2", "RAL-3"] {
        let body = json!({"change": "ready", "story": story});
        assert_eq!(write(&a1, "x", "declared", body).status, 200);
    }
    let first: Value = serde_json::from_str(&call(&a1, "GET", "/v1/check", None).body).unwrap();
    let head = format!("3:{}", first["head"]["hash"].as_str().unwrap());
    let (_, hash2) = stored(&service, 2);

    // A deletion of the newest record, as a DEL event through myko would leave it, with the head
    // file wound back to match.
    stopped(&mut service);
    as_myko_would(&service, &[(3, "DEL", None)]);
    std::fs::write(
        service.head_file(),
        json!({"seq": 2, "hash": hash2}).to_string(),
    )
    .unwrap();
    let output = host_check(&service, Some(&head));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("removed"));
    // Writes stop too: no writer starts over the deletion's second version of record 3.
    assert!(service.refuses_to_start().contains("more than one version"));
}

#[test]
fn concurrent_writers_through_the_service_keep_one_chain() {
    let service = start();
    let writers: Vec<Pairing> = (0..8)
        .map(|i| service.add(&format!("m{i}"), false))
        .collect();
    let threads: Vec<_> = writers
        .into_iter()
        .enumerate()
        .map(|(i, pairing)| {
            std::thread::spawn(move || {
                (0..10)
                    .map(|j| {
                        let story = format!("RAL-{}", (i + 1) * 100 + j);
                        let body = json!({"change": "ready", "story": story});
                        let reply = write(&pairing, "x", "declared", body);
                        assert_eq!(reply.status, 200, "{}", reply.body);
                        let value: Value = serde_json::from_str(&reply.body).unwrap();
                        value["recorded"].as_u64().unwrap()
                    })
                    .collect::<Vec<u64>>()
            })
        })
        .collect();
    let mut recorded: Vec<u64> = threads
        .into_iter()
        .flat_map(|t| t.join().unwrap())
        .collect();
    recorded.sort_unstable();
    assert_eq!(recorded, (1..=80).collect::<Vec<u64>>());
    let output = host_check(&service, None);
    assert!(output.status.success());
    let check: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(check["head"]["seq"], 80);
}

#[test]
fn a_direct_write_is_refused_while_the_service_holds_the_ledger() {
    let service = start();
    let a1 = service.add("a1", false);
    assert_eq!(ready(&a1).status, 200);
    let output = rally()
        .arg("--db")
        .arg(&service.db.url)
        .arg("--head-file")
        .arg(service.head_file())
        .args(["--credential", "local", "--as", "x", "ready", "RAL-2"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("another process is writing"));
    let check: Value = serde_json::from_str(&call(&a1, "GET", "/v1/check", None).body).unwrap();
    assert_eq!(check["head"]["seq"], 1);
}

#[test]
fn with_postgres_stopped_a_write_fails_and_nothing_acknowledged_is_lost() {
    let cluster = Cluster::start();
    let mut service = start_on(&cluster, &GENEROUS);
    let a1 = service.add("a1", false);
    let mark = |pairing: &Pairing, story: &str| {
        write(
            pairing,
            "x",
            "declared",
            json!({"change": "ready", "story": story}),
        )
    };
    assert_eq!(mark(&a1, "RAL-1").status, 200);
    assert_eq!(mark(&a1, "RAL-2").status, 200);

    cluster.stop();
    for story in ["RAL-3", "RAL-4"] {
        let started = std::time::Instant::now();
        let reply = mark(&a1, story);
        assert_ne!(reply.status, 200, "a false success: {}", reply.body);
        assert!(!reply.body.contains("\"recorded\":"), "{}", reply.body);
        assert!(started.elapsed() < std::time::Duration::from_secs(30));
    }
    cluster.restart();
    assert_eq!(mark(&a1, "RAL-5").status, 200);
    let all: Value = serde_json::from_str(&call(&a1, "GET", "/v1/stories", None).body).unwrap();
    let stories: Vec<&String> = all["stories"].as_object().unwrap().keys().collect();
    assert_eq!(stories, ["RAL-1", "RAL-2", "RAL-5"]);

    service.restart("-KILL");
    let mut a1 = a1;
    a1.address = service.address.clone();
    let check: Value = serde_json::from_str(&call(&a1, "GET", "/v1/check", None).body).unwrap();
    assert_eq!(check["ok"], true, "{check}");
    assert_eq!(check["head"]["seq"], 3);
    drop(service);
}

#[test]
fn many_appends_and_repeated_restarts_hold_up() {
    // Longer than the spike's brief run: a thousand appends through the door, and restarts in
    // between with both signals, checking against a saved head each time.
    let mut service = start_with(&[
        "--burst",
        "100000",
        "--write-burst",
        "100000",
        "--min-free-bytes",
        "0",
    ]);
    let mut a1 = service.add("a1", false);
    let mut seq = 0;
    for round in 0..5 {
        for i in 0..200 {
            let body = json!({"change": "ready", "story": format!("RAL-{}", round * 1000 + i)});
            let reply = write(&a1, "x", "declared", body);
            assert_eq!(reply.status, 200, "{}", reply.body);
            seq += 1;
        }
        let check: Value = serde_json::from_str(&call(&a1, "GET", "/v1/check", None).body).unwrap();
        assert_eq!(check["ok"], true);
        let head = format!("{seq}:{}", check["head"]["hash"].as_str().unwrap());
        service.restart(if round % 2 == 0 { "-KILL" } else { "-TERM" });
        a1.address = service.address.clone();
        let output = host_check(&service, Some(&head));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let all: Value = serde_json::from_str(&call(&a1, "GET", "/v1/stories", None).body).unwrap();
        assert_eq!(all["as_of"], seq);
    }
}

/// Resumes a stopped cluster when dropped.
struct Resume<'a>(&'a Cluster);

impl Drop for Resume<'_> {
    fn drop(&mut self) {
        signal_cluster(self.0, "-CONT");
    }
}

/// Sends `signal` to the scratch cluster's own processes: its postmaster, named in its data
/// directory, and that postmaster's children. Nothing else.
fn signal_cluster(cluster: &Cluster, signal: &str) {
    let data = cluster.directory().unwrap().join("data");
    let text = std::fs::read_to_string(data.join("postmaster.pid")).unwrap();
    let postmaster = text.lines().next().unwrap().trim().to_owned();
    let command = Command::new("ps")
        .args(["-o", "command=", "-p", &postmaster])
        .output()
        .unwrap();
    let command = String::from_utf8_lossy(&command.stdout);
    assert!(
        command.contains(&*data.to_string_lossy()),
        "{postmaster} isn't the scratch cluster: {command}"
    );
    let children = Command::new("pgrep")
        .args(["-P", &postmaster])
        .output()
        .unwrap();
    let mut pids: Vec<String> = String::from_utf8_lossy(&children.stdout)
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    pids.push(postmaster);
    assert!(
        Command::new("kill")
            .arg(signal)
            .args(&pids)
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn reads_carry_on_while_a_write_waits_on_a_hung_postgres() {
    let cluster = Cluster::start();
    let service = start_on(&cluster, &GENEROUS);
    let a1 = service.add("a1", false);
    assert_eq!(ready(&a1).status, 200);

    signal_cluster(&cluster, "-STOP");
    // However this test ends, the cluster isn't left stopped.
    let _resume = Resume(&cluster);
    let writer = {
        let a1 = a1.clone();
        std::thread::spawn(move || {
            let body = json!({"change": "ready", "story": "RAL-2"});
            write(&a1, "x", "declared", body)
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(500));
    let started = std::time::Instant::now();
    let reply = call(&a1, "GET", "/v1/stories", None);
    let waited = started.elapsed();
    signal_cluster(&cluster, "-CONT");
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert!(
        waited < std::time::Duration::from_secs(3),
        "the read waited {waited:?}"
    );
    // The write finishes once Postgres does, and is never a false success.
    let written = writer.join().unwrap();
    let all: Value = serde_json::from_str(&call(&a1, "GET", "/v1/stories", None).body).unwrap();
    let recorded = all["stories"].as_object().unwrap().contains_key("RAL-2");
    assert_eq!(written.status == 200, recorded, "{}", written.body);
}

/// Whether the service closes `connection` within `within`, given that the client sends nothing.
fn closed_within(connection: &mut rally::client::Connection, within: Duration) -> bool {
    use std::io::Read;
    connection.sock.set_read_timeout(Some(within)).unwrap();
    let mut byte = [0u8; 1];
    match connection.read(&mut byte) {
        Ok(0) => true,
        Ok(_) => false,
        Err(error) => !matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ),
    }
}

#[test]
fn a_connection_that_sends_nothing_is_closed_before_it_authenticates() {
    let service = start_with(&["--connection-seconds", "2"]);
    let pairing = service.add("a1", false);
    let mut idle = rally::client::connect(&pairing).unwrap();
    let started = Instant::now();
    assert!(closed_within(&mut idle, Duration::from_secs(8)));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(call(&pairing, "GET", "/v1/whoami", None).status, 200);
}

#[test]
fn connections_are_capped_and_a_closed_one_frees_its_place() {
    let service = start_with(&[
        "--max-connections",
        "4",
        "--max-connections-per-address",
        "3",
    ]);
    let pairing = service.add("a1", false);
    // All from 127.0.0.1, so the per-address cap is the one reached.
    let mut held: Vec<_> = (0..3)
        .map(|_| rally::client::connect(&pairing).unwrap())
        .collect();
    let refused = rally::client::connect(&pairing)
        .map_err(|e| e.to_string())
        .and_then(|_| request(&pairing, "GET", "/v1/whoami", None, &[]).map_err(|e| e.to_string()));
    assert!(
        refused.is_err(),
        "a fourth connection from one address got in: {refused:?}"
    );
    held.pop();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(call(&pairing, "GET", "/v1/whoami", None).status, 200);
    drop(held);
}

#[test]
fn failed_token_attempts_are_limited_per_address() {
    let service = start_with(&[
        "--failed-token-burst",
        "5",
        "--failed-token-per-second",
        "0.01",
    ]);
    let pairing = service.add("a1", false);
    let wrong = service.wrong_token();
    for attempt in 0..5 {
        let status = call(&wrong, "GET", "/v1/whoami", None).status;
        assert_eq!(status, 401, "attempt {attempt}");
    }
    let reply = call(&wrong, "GET", "/v1/whoami", None);
    assert_eq!(reply.status, 429, "{}", reply.body);
    // Refused before the token is looked at, so a guess can't tell a right token from a wrong one.
    assert_eq!(call(&pairing, "GET", "/v1/whoami", None).status, 429);
}

#[test]
fn checks_and_past_state_reads_have_their_own_limit_and_plain_reads_carry_on() {
    let service = start_with(&[
        "--burst",
        "1000",
        "--write-burst",
        "1000",
        "--min-free-bytes",
        "0",
        "--heavy-burst",
        "3",
        "--heavy-per-second",
        "0.001",
    ]);
    let pairing = service.add("a1", false);
    assert_eq!(ready(&pairing).status, 200);
    assert_eq!(call(&pairing, "GET", "/v1/check", None).status, 200);
    assert_eq!(
        call(&pairing, "GET", "/v1/stories?as_of=1", None).status,
        200
    );
    assert_eq!(call(&pairing, "GET", "/v1/check", None).status, 200);
    for path in [
        "/v1/check",
        "/v1/stories?as_of=1",
        "/v1/teams/rally?as_of=1",
    ] {
        let reply = call(&pairing, "GET", path, None);
        assert_eq!(reply.status, 429, "{path}: {}", reply.body);
    }
    // Another credential has its own, and the current state is still read freely.
    let other = service.add("a2", true);
    assert_eq!(call(&other, "GET", "/v1/check", None).status, 200);
    for _ in 0..20 {
        assert_eq!(call(&pairing, "GET", "/v1/stories", None).status, 200);
    }
}

#[test]
fn lists_and_histories_come_in_pages_through_the_service() {
    let service = start();
    let pairing = service.add("a1", false);
    let total = rally::views::HISTORY_LIMIT + 5;
    for i in 0..total {
        let body = json!({"change": "team_state", "team": "rally", "status": "working",
                          "note": format!("state {i}")});
        assert_eq!(write(&pairing, "someone", "declared", body).status, 200);
    }
    let page: Value =
        serde_json::from_str(&call(&pairing, "GET", "/v1/teams/rally", None).body).unwrap();
    assert_eq!(page["history_total"], total);
    assert_eq!(
        page["detail"]["history"].as_array().unwrap().len(),
        rally::views::HISTORY_LIMIT
    );
    let earlier = page["earlier"].as_u64().unwrap();
    let path = format!("/v1/teams/rally?before={earlier}");
    let page: Value = serde_json::from_str(&call(&pairing, "GET", &path, None).body).unwrap();
    assert_eq!(page["detail"]["history"].as_array().unwrap().len(), 5);
    assert!(page.get("earlier").is_none());

    assert_eq!(ready(&pairing).status, 200);
    let list: Value =
        serde_json::from_str(&call(&pairing, "GET", "/v1/stories", None).body).unwrap();
    assert!(list["stories"]["RAL-13"].is_object());
    assert!(list.get("next").is_none());
    let after: Value =
        serde_json::from_str(&call(&pairing, "GET", "/v1/stories?after=RAL-13", None).body)
            .unwrap();
    assert!(after["stories"].as_object().unwrap().is_empty());
    let long = format!("/v1/stories?after={}", "x".repeat(300));
    assert_eq!(call(&pairing, "GET", &long, None).status, 400);
}

#[test]
fn limits_below_one_are_refused_at_start() {
    let service = start();
    for (flag, value) in [
        ("--failed-token-burst", "0"),
        ("--heavy-burst", "0.5"),
        ("--write-per-second=-1", ""),
        ("--max-connections", "0"),
    ] {
        let output = rally()
            .arg("--db")
            .arg(&service.db.url)
            .arg("--head-file")
            .arg(service.head_file())
            .args(["serve", "--bind", "127.0.0.1:0", flag])
            .args([value].into_iter().filter(|value| !value.is_empty()))
            .arg("--state-dir")
            .arg(service.state_dir())
            .output()
            .unwrap();
        assert!(!output.status.success(), "{flag} {value} was accepted");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("must") || error.contains("can't"),
            "{flag}: {error}"
        );
    }
}

#[test]
fn a_refused_write_says_nothing_was_recorded_rather_than_that_the_ledger_cant_be_read() {
    let service = start();
    let pairing = service.add("a1", false);
    assert_eq!(ready(&pairing).status, 200);
    std::fs::remove_file(service.head_file()).unwrap();
    let body = json!({"change": "team_state", "team": "rally", "status": "working"});
    let reply = write(&pairing, "someone", "declared", body);
    assert_eq!(reply.status, 500, "{}", reply.body);
    assert!(
        reply.body.contains("nothing was recorded"),
        "{}",
        reply.body
    );
    assert!(!reply.body.contains("can't be read"), "{}", reply.body);
}

#[test]
fn writes_behind_one_stuck_on_a_hung_postgres_are_told_the_service_is_busy() {
    let cluster = Cluster::start();
    let service = start_on(&cluster, &GENEROUS);
    let a1 = service.add("a1", false);
    assert_eq!(ready(&a1).status, 200);
    signal_cluster(&cluster, "-STOP");
    let _resume = Resume(&cluster);
    let first = {
        let a1 = a1.clone();
        std::thread::spawn(move || {
            let body = json!({"change": "ready", "story": "RAL-2"});
            let text = json!({"kind": "declared", "declared_name": "x", "body": body}).to_string();
            // It may get no reply at all, once its connection's time runs out.
            request(&a1, "POST", "/v1/records", Some(&text), &[]).map_err(|e| e.to_string())
        })
    };
    std::thread::sleep(Duration::from_millis(500));
    // Later writes don't each wait on Postgres, holding a thread; they're told to try again.
    let later: Vec<_> = (0..4)
        .map(|i| {
            let a1 = a1.clone();
            std::thread::spawn(move || {
                let started = Instant::now();
                let body = json!({"change": "ready", "story": format!("RAL-{}", 10 + i)});
                let reply = write(&a1, "x", "declared", body);
                (reply, started.elapsed())
            })
        })
        .collect();
    for thread in later {
        let (reply, waited) = thread.join().unwrap();
        assert_eq!(reply.status, 503, "{}", reply.body);
        assert!(reply.body.contains("busy"), "{}", reply.body);
        assert!(waited < Duration::from_secs(20), "{waited:?}");
    }
    assert_eq!(call(&a1, "GET", "/v1/stories", None).status, 200);
    signal_cluster(&cluster, "-CONT");
    let _ = first.join();
}

#[test]
fn export_gives_every_record_with_its_hash_a_page_at_a_time_to_a_read_only_credential() {
    let service = start();
    let writer = service.add("a1", false);
    let backup = service.add("backup", true);
    let total = rally::store::EXPORT_LIMIT + 3;
    for i in 0..total {
        let body = json!({"change": "team_state", "team": "rally", "status": "working",
                          "note": format!("state {i}")});
        assert_eq!(write(&writer, "x", "declared", body).status, 200);
    }
    let mut seen = Vec::new();
    let mut previous = "0".repeat(64);
    let mut after = 0;
    loop {
        let reply = call(&backup, "GET", &format!("/v1/export?after={after}"), None);
        assert_eq!(reply.status, 200, "{}", reply.body);
        let page: Value = serde_json::from_str(&reply.body).unwrap();
        assert!(page["untrusted_text"].is_string());
        assert_eq!(page["head"]["seq"], total);
        let records = page["records"].as_array().unwrap();
        assert!(records.len() <= rally::store::EXPORT_LIMIT);
        for entry in records {
            let seq = entry["seq"].as_u64().unwrap();
            let record = entry["record"].as_str().unwrap();
            let hash = entry["hash"].as_str().unwrap();
            // The backup can recheck the chain from what it's given alone.
            assert_eq!(chain_hash(&previous, seq, record), hash, "record {seq}");
            previous = hash.to_owned();
            seen.push(seq);
        }
        match page["next"].as_u64() {
            Some(next) => after = next,
            None => break,
        }
    }
    assert_eq!(seen, (1..=total as u64).collect::<Vec<_>>());
    let page: Value = serde_json::from_str(
        &call(&backup, "GET", &format!("/v1/export?after={total}"), None).body,
    )
    .unwrap();
    assert!(page["records"].as_array().unwrap().is_empty());
    assert_eq!(
        call(&service.wrong_token(), "GET", "/v1/export", None).status,
        401
    );
    assert_eq!(call(&backup, "POST", "/v1/export", Some("{}")).status, 403);
}

#[test]
fn a_backup_through_the_service_holds_a_head_that_check_expect_accepts() {
    let service = start();
    let writer = service.add("a1", false);
    let code = {
        let out = service.dir.path().join("backup.pair");
        let output = rally()
            .args([
                "add-machine",
                "backup",
                "--read-only",
                "--address",
                &service.address,
            ])
            .arg("--state-dir")
            .arg(service.state_dir())
            .arg("--out")
            .arg(&out)
            .output()
            .unwrap();
        assert!(output.status.success());
        std::fs::read_to_string(&out).unwrap()
    };
    // On "another machine": outside the service's own directory, which goes when it stops.
    let elsewhere = tempfile::tempdir().unwrap();
    let config = elsewhere.path().join("backup.json");
    assert!(pair(&config, &code).status.success());
    let backup = elsewhere.path().join("backup");
    let run = || {
        rally()
            .arg("--config")
            .arg(&config)
            .args(["backup", "--to"])
            .arg(&backup)
            .output()
            .unwrap()
    };
    for _ in 0..3 {
        assert_eq!(ready_with(&writer).status, 200);
    }
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let first: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        (first["added"].as_u64(), first["backed_up_to"].as_u64()),
        (Some(3), Some(3))
    );
    assert_eq!(ready_with(&writer).status, 200);
    let second: Value = serde_json::from_slice(&run().stdout).unwrap();
    assert_eq!(
        (second["added"].as_u64(), second["backed_up_to"].as_u64()),
        (Some(1), Some(4))
    );
    // The saved head is the one `check --expect` takes.
    let expect = second["expect"].as_str().unwrap();
    let output = rally()
        .arg("--config")
        .arg(&config)
        .args(["check", "--expect", expect])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Restored into a new ledger, it checks against the backup's head and holds the same stories.
    let target = scratch_pg::shared().database();
    let restored_dir = tempfile::tempdir().unwrap();
    let restored_head = restored_dir.path().join("ledger.head");
    let on_restored = |args: &[&str]| {
        rally()
            .arg("--db")
            .arg(&target.url)
            .arg("--head-file")
            .arg(&restored_head)
            .args(args)
            .output()
            .unwrap()
    };
    let output = on_restored(&["restore", "--from", backup.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let restored: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(restored["expect"], second["expect"]);
    assert_eq!(restored["backup_head_file"], second["expect"]);
    let output = on_restored(&["check", "--expect", expect]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let original = rally()
        .arg("--config")
        .arg(&config)
        .arg("show")
        .output()
        .unwrap();
    let copy = on_restored(&["show"]);
    let stories = |output: &std::process::Output| -> Value {
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["stories"].clone()
    };
    assert_eq!(stories(&copy), stories(&original));
    // A second restore into the same ledger is refused.
    assert!(
        !on_restored(&["restore", "--from", backup.to_str().unwrap()])
            .status
            .success()
    );

    // With the service gone, it fails and says how old the backup is.
    drop(service);
    let output = run();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("hours ago"), "{error}");
}

/// A new story each time, so that writes can repeat.
fn ready_with(pairing: &Pairing) -> Reply {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(100);
    let story = format!("RAL-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    write(
        pairing,
        "someone",
        "declared",
        json!({"change": "ready", "story": story}),
    )
}

#[test]
fn session_start_shows_a_teams_state_through_the_service() {
    let service = start();
    let pairing = service.add("a1", false);
    let config = service.dir.path().join("client/config.json");
    assert!(pair(&config, &pairing.code()).status.success());
    let commit = "a".repeat(40);
    for (name, args) in [
        ("r", vec!["ready", "RAL-60"]),
        ("r", vec!["claim", "RAL-60", "--team", "rally"]),
        ("m", vec!["ready", "RAL-61"]),
        ("m", vec!["claim", "RAL-61", "--team", "muster"]),
        (
            "m",
            vec![
                "submit",
                "RAL-61",
                "--team",
                "muster",
                "--evidence",
                "muster@aaaa",
            ],
        ),
        (
            "w",
            vec![
                "request-read",
                "RAL-61",
                "--commit",
                &commit,
                "--note",
                "please",
            ],
        ),
        (
            "r",
            vec![
                "block",
                "--team",
                "rally",
                "--note",
                "waiting on <the operator>",
            ],
        ),
        (
            "m",
            vec![
                "block",
                "--team",
                "muster",
                "--on-story",
                "RAL-60",
                "--note",
                "needs RAL-60",
            ],
        ),
    ] {
        let out = cli(&config, name, &args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let out = rally()
        .arg("--config")
        .arg(&config)
        .args(["session-start", "--team", "rally"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("<rally_state team=\"rally\">"), "{text}");
    assert!(text.contains("- RAL-60: claimed (declared)"), "{text}");
    assert!(
        text.contains("- RAL-61 (team muster): in review (declared)"),
        "{text}"
    );
    assert!(
        text.contains("- RAL-61 (team muster): 1 open (declared)"),
        "{text}"
    );
    assert!(
        text.contains("team rally waits: waiting on &lt;the operator> (declared)"),
        "{text}"
    );
    assert!(
        text.contains("team muster waits on RAL-60: needs RAL-60"),
        "{text}"
    );
    assert!(text.trim_end().ends_with("</rally_state>"), "{text}");
}

/// An ed25519 key made by ssh-keygen in `dir`; returns its private key's path.
fn ssh_key(dir: &Path, name: &str) -> PathBuf {
    let private = dir.join(name);
    let status = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "test", "-f"])
        .arg(&private)
        .status()
        .unwrap();
    assert!(status.success());
    private
}

#[test]
fn serve_secure_refuses_a_binary_root_doesnt_own_and_gives_the_operators_steps() {
    let service = start();
    let key = ssh_key(service.dir.path(), "operator");
    let output = rally()
        .arg("--db")
        .arg(&service.db.url)
        .arg("--head-file")
        .arg(service.head_file())
        .args([
            "serve",
            "--bind",
            "127.0.0.1:0",
            "--secure",
            "--operator-key",
        ])
        .arg(key.with_extension("pub"))
        .arg("--state-dir")
        .arg(service.state_dir())
        .output()
        .unwrap();
    assert!(!output.status.success(), "it started");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("--secure refused"), "{error}");
    assert!(
        error.contains("sudo install -o root -g root -m 0755"),
        "{error}"
    );
    assert!(error.contains("/etc/rally/operator.pub"), "{error}");

    // The key is only for --secure, and --secure needs the key.
    for args in [&["--operator-key", "x.pub"][..], &["--secure"][..]] {
        let output = rally()
            .args(["serve", "--bind", "127.0.0.1:0", "--state-dir", "x"])
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{args:?}");
    }
}

#[test]
fn a_designation_is_refused_by_a_service_without_secure() {
    let service = start();
    let pairing = service.add("a1", false);
    let config = service.dir.path().join("client/config.json");
    assert!(pair(&config, &pairing.code()).status.success());
    let key = ssh_key(service.dir.path(), "operator");
    let out = rally()
        .arg("--config")
        .arg(&config)
        .args([
            "--as",
            "operator",
            "designate",
            "--watchman",
            "grand-photon",
            "--key",
        ])
        .arg(&key)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(
        error.contains("rally-designation v1\nwatchman: grand-photon\nafter: 0:"),
        "{error}"
    );
    assert!(error.contains("--secure"), "{error}");

    let report = cli(&config, "x", &["designation"]);
    assert!(
        report.status.success(),
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
    let report: Value = serde_json::from_slice(&report.stdout).unwrap();
    assert!(report["watchman"].is_null());
    assert!(report["operator_key"].is_null());
    assert_eq!(
        report["records"].as_array().unwrap().len(),
        0,
        "nothing was recorded"
    );
}

/// RAL-56 end to end: a question to the operator holds the pick until the service's window
/// passes, the service measures the shelve by its own clock and window, and the answer, which
/// says where it came from, brings the story back first.
#[test]
fn a_question_to_the_operator_is_shelved_after_the_window_and_returns_on_an_answer() {
    let mut limits = GENEROUS.to_vec();
    limits.extend(["--operator-window", "2s"]);
    let service = start_with(&limits);
    let pairing = service.add("a1", false);
    let config = service.dir.path().join("client/config.json");
    assert!(pair(&config, &pairing.code()).status.success());
    let roadmap = service.dir.path().join("roadmap.md");
    std::fs::write(
        &roadmap,
        "- [x] **RAL-1: Done.**\n- [ ] **RAL-2: Needs the operator.**\n- [ ] **RAL-3: Next.**\n",
    )
    .unwrap();
    let run = |args: &[&str]| {
        let out = cli(&config, "r", args);
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let error = String::from_utf8_lossy(&out.stderr).into_owned();
        (out.status.success(), text, error)
    };
    let next = || {
        let (ok, text, error) = run(&[
            "next",
            "--team",
            "rally",
            "--roadmap",
            roadmap.to_str().unwrap(),
        ]);
        assert!(ok, "{error}");
        serde_json::from_str::<Value>(&text).unwrap()
    };
    assert_eq!(next()["next"]["story"], "RAL-2");
    assert_eq!(next()["operator_window_ms"], 2_000);

    let (ok, asked, error) = run(&[
        "block",
        "--team",
        "rally",
        "--story",
        "RAL-2",
        "--on-team",
        "operator",
        "--note",
        "may Rally read the token file?",
    ]);
    assert!(ok, "{error}");
    let blocker: Value = serde_json::from_str(&asked).unwrap();
    let blocker = blocker["recorded"].as_u64().unwrap().to_string();

    // Within the window: still the pick, and the service won't shelve it.
    let waiting = next();
    assert_eq!(waiting["next"]["story"], "RAL-2");
    assert_eq!(waiting["waiting_on_operator"][0]["shelve_due"], false);
    let (ok, _, error) = run(&["shelve", &blocker, "--team", "rally"]);
    assert!(!ok && error.contains("window hasn't passed"), "{error}");
    // Nor will it take a window the writer chose.
    let early = write(
        &pairing,
        "r",
        "declared",
        json!({"change": "shelve", "blocker": blocker.parse::<u64>().unwrap(), "team": "rally",
               "window_ms": 1}),
    );
    assert_eq!(early.status, 409, "{}", early.body);
    assert!(
        early.body.contains("isn't the one the service keeps"),
        "{}",
        early.body
    );

    std::thread::sleep(std::time::Duration::from_millis(2_100));
    let due = next();
    assert_eq!(due["waiting_on_operator"][0]["shelve_due"], true);
    assert_eq!(due["after_shelving"]["story"], "RAL-3");
    let (ok, _, error) = run(&["shelve", &blocker, "--team", "rally"]);
    assert!(ok, "{error}");
    let shelved = next();
    assert_eq!(shelved["next"]["story"], "RAL-3");
    assert_eq!(shelved["shelved"][0]["story"], "RAL-2");
    assert_eq!(
        shelved["shelved"][0]["question"],
        "may Rally read the token file?"
    );

    // The answer says where it came from.
    let (ok, _, error) = run(&["unblock", &blocker, "--note", "yes"]);
    assert!(
        !ok && error.contains("where the answer came from"),
        "{error}"
    );
    let (ok, _, error) = run(&[
        "unblock",
        &blocker,
        "--note",
        "yes",
        "--relayed-by",
        "grand-photon",
    ]);
    assert!(ok, "{error}");
    let answered = next();
    assert_eq!(answered["next"]["story"], "RAL-2");
    assert_eq!(answered["shelved"][0]["answered"], true);
    let shown: Value =
        serde_json::from_str(&call(&pairing, "GET", "/v1/blockers/all", None).body).unwrap();
    assert_eq!(
        shown["blockers"][0]["closed"]["answer"],
        json!({"from": "relayed", "by": "grand-photon"})
    );
}

#[test]
fn the_service_keeps_the_times_agreed_with_the_operator() {
    let service = start();
    let pairing = service.add("a1", false);
    let whoami: Value =
        serde_json::from_str(&call(&pairing, "GET", "/v1/whoami", None).body).unwrap();
    assert_eq!(whoami["operator_window_ms"], 300_000);
    assert_eq!(whoami["operator_away_after"], 3);
    assert_eq!(whoami["operator_away_ms"], 3_600_000);
    // A question asked now can't be shelved for five minutes.
    let asked = write(
        &pairing,
        "r",
        "declared",
        json!({"change": "block", "team": "rally", "story": "RAL-2", "on_team": "operator",
               "note": "may we?"}),
    );
    assert_eq!(asked.status, 200, "{}", asked.body);
    let shelve = write(
        &pairing,
        "r",
        "declared",
        json!({"change": "shelve", "blocker": 1, "team": "rally"}),
    );
    assert_eq!(shelve.status, 409, "{}", shelve.body);
    assert!(
        shelve.body.contains("window hasn't passed"),
        "{}",
        shelve.body
    );
    // No machine can take the name on Rally's own records.
    let out = rally()
        .args(["add-machine", "rally", "--address", &service.address])
        .arg("--state-dir")
        .arg(service.state_dir())
        .arg("--out")
        .arg(service.dir.path().join("rally.pair"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Rally's own records"));
}

/// Three shelves in a row with no answer put the operator away for the agreed time, recorded by
/// Rally as observed; nothing waiting on the operator is offered during it, and it's all asked
/// again after it.
#[test]
fn three_unanswered_shelves_put_the_operator_away_for_the_agreed_time() {
    let mut limits = GENEROUS.to_vec();
    limits.extend(["--operator-window", "1s", "--operator-away", "4s"]);
    let service = start_with(&limits);
    let pairing = service.add("a1", false);
    let config = service.dir.path().join("client/config.json");
    assert!(pair(&config, &pairing.code()).status.success());
    let roadmap = service.dir.path().join("roadmap.md");
    std::fs::write(
        &roadmap,
        "- [ ] **RAL-2: A.**\n- [ ] **RAL-3: B.**\n- [ ] **RAL-4: C.**\n- [ ] **RAL-5: D.**\n",
    )
    .unwrap();
    let next = || {
        let out = cli(
            &config,
            "r",
            &[
                "next",
                "--team",
                "rally",
                "--roadmap",
                roadmap.to_str().unwrap(),
            ],
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    let blockers = || -> Value {
        serde_json::from_str(&call(&pairing, "GET", "/v1/blockers/all", None).body).unwrap()
    };
    for story in ["RAL-2", "RAL-3", "RAL-4"] {
        let asked = write(
            &pairing,
            "r",
            "declared",
            json!({"change": "block", "team": "rally", "story": story, "on_team": "operator",
                   "note": format!("may we do {story}?")}),
        );
        assert_eq!(asked.status, 200, "{}", asked.body);
    }
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    let shelve = |blocker: u64| {
        let reply = write(
            &pairing,
            "r",
            "declared",
            json!({"change": "shelve", "blocker": blocker, "team": "rally"}),
        );
        assert_eq!(reply.status, 200, "{}", reply.body);
    };
    shelve(1);
    shelve(2);
    assert!(blockers()["operator_away"].is_null());
    assert_eq!(blockers()["unanswered_shelves"], json!([4, 5]));
    shelve(3);
    let away = blockers()["operator_away"].clone();
    assert_eq!(away["seq"], 7, "{away}");
    assert_eq!(away["shelves"], json!([4, 5, 6]));
    // Written by Rally itself, as observed.
    let records: Value =
        serde_json::from_str(&call(&pairing, "GET", "/v1/export?after=6", None).body).unwrap();
    let text = records["records"][0]["record"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        text.contains("\"kind\":\"observed\"") && text.contains("\"credential\":\"rally\""),
        "{records}"
    );

    // During the hour: RAL-5 is free, and nothing waiting on the operator is offered.
    let during = next();
    assert_eq!(during["next"]["story"], "RAL-5", "{during}");
    assert!(during["operator_away"]["until_ms"].is_i64());
    assert_eq!(during["shelved"].as_array().unwrap().len(), 3);

    // After it, the three are asked again, first in roadmap order.
    std::thread::sleep(std::time::Duration::from_millis(4_100));
    let after = next();
    assert!(after["operator_away"].is_null(), "{after}");
    assert_eq!(after["next"]["story"], "RAL-2");
    assert_eq!(after["waiting_on_operator"][0]["ask_again"], true);
    assert_eq!(blockers()["unanswered_shelves"], json!([]));
}

/// RAL-57: without the operator's key the service can't check designations, so it suggests
/// readers and says so; a suggestion is never the author's team.
#[test]
fn waiting_suggests_a_reader_from_another_team() {
    let service = start();
    let pairing = service.add("a1", false);
    for body in [
        json!({"change": "team_state", "team": "muster", "status": "available"}),
        json!({"change": "ready", "story": "RAL-60"}),
        json!({"change": "claim", "story": "RAL-60", "team": "rally"}),
        json!({"change": "submit", "story": "RAL-60", "team": "rally", "evidence": "rally@a"}),
    ] {
        let reply = write(&pairing, "r", "declared", body);
        assert_eq!(reply.status, 200, "{}", reply.body);
    }
    let waiting: Value =
        serde_json::from_str(&call(&pairing, "GET", "/v1/waiting", None).body).unwrap();
    let row = &waiting["waiting"][0];
    assert_eq!(row["story"], "RAL-60");
    assert_eq!(
        row["suggested_reader"],
        json!({"team": "muster", "kind": "declared"})
    );
    assert!(
        waiting["readers"].as_str().unwrap().contains("can't check"),
        "{waiting}"
    );
    // A past state would be the wrong answer, so it's refused, not ignored.
    let past = call(&pairing, "GET", "/v1/waiting?as_of=1", None);
    assert_eq!(past.status, 400, "{}", past.body);
    assert!(past.body.contains("doesn't take as_of"));
}

/// RAL-22a: the service gives one brief, at its own clock, with every part labelled and aged.
#[test]
fn the_service_gives_a_new_watchman_one_brief() {
    let service = start();
    let pairing = service.add("a1", false);
    for body in [
        json!({"change": "team_state", "team": "rally", "status": "working"}),
        json!({"change": "ready", "story": "RAL-60"}),
        json!({"change": "claim", "story": "RAL-60", "team": "rally"}),
        json!({"change": "submit", "story": "RAL-60", "team": "rally", "evidence": "rally@a"}),
        json!({"change": "ready", "story": "RAL-61"}),
        json!({"change": "claim", "story": "RAL-61", "team": "rally"}),
        json!({"change": "block", "team": "rally", "story": "RAL-61", "on_team": "operator",
               "note": "which way?"}),
    ] {
        let reply = write(&pairing, "r", "declared", body);
        assert_eq!(reply.status, 200, "{}", reply.body);
    }
    let reply = call(&pairing, "GET", "/v1/brief", None);
    assert_eq!(reply.status, 200, "{}", reply.body);
    let brief: Value = serde_json::from_str(&reply.body).unwrap();
    assert_eq!(brief["waiting"][0]["story"], "RAL-60", "{brief}");
    assert_eq!(brief["claims"][0]["story"], "RAL-61");
    assert_eq!(brief["claims"][0]["claimed_by"], "r");
    assert_eq!(brief["teams"][0]["kind"], "declared");
    assert!(brief["teams"][0]["age_ms"].as_i64().unwrap() >= 0);
    assert_eq!(
        brief["operator"]["questions"][0]["holds_up"]["story"],
        "RAL-61"
    );
    assert!(
        brief["authority"]
            .as_str()
            .unwrap()
            .contains("confers no authority")
    );
    assert!(brief["not_recorded"].as_str().unwrap().contains("RAL-64"));
    // Without the operator's key the service can't check designations.
    assert!(brief["watchman"].is_null());
    assert!(
        brief["handover"]["note"]
            .as_str()
            .unwrap()
            .contains("no operator key")
    );
}

/// `rally next` reads every page of blockers at the first page's instant (RAL-56), and the
/// service judges a page at the `at_ms` it's given.
#[test]
fn next_reads_every_page_of_blockers_at_one_instant() {
    let service = start();
    let pairing = service.add("a1", false);
    let config = service.dir.path().join("client/config.json");
    assert!(pair(&config, &pairing.code()).status.success());
    // Over one page of blockers, with the question to the operator on the second.
    for n in 0..201 {
        let reply = write(
            &pairing,
            "r",
            "declared",
            json!({"change": "block", "team": "muster", "note": format!("waiting {n}")}),
        );
        assert_eq!(reply.status, 200, "{}", reply.body);
    }
    let asked = write(
        &pairing,
        "r",
        "declared",
        json!({"change": "block", "team": "rally", "story": "RAL-2", "on_team": "operator",
               "note": "may we?"}),
    );
    assert_eq!(asked.status, 200, "{}", asked.body);
    let page: Value = serde_json::from_str(
        &call(&pairing, "GET", "/v1/blockers/all?at_ms=5&after=200", None).body,
    )
    .unwrap();
    assert_eq!(page["now_ms"], 5);
    let question = page["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|blocker| blocker["seq"] == 202)
        .unwrap();
    assert_eq!(question["question"]["window_from_ms"], question["at_ms"]);

    let roadmap = service.dir.path().join("roadmap.md");
    std::fs::write(&roadmap, "- [ ] **RAL-2: Needs the operator.**\n").unwrap();
    let out = cli(
        &config,
        "r",
        &[
            "next",
            "--team",
            "rally",
            "--roadmap",
            roadmap.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let next: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(next.get("incomplete").is_none(), "{next}");
    assert_eq!(next["next"]["story"], "RAL-2");
    assert_eq!(next["waiting_on_operator"][0]["blocker"], 202);
}

/// RAL-55: teams asking for a story number at once each get a different one, above every number
/// in the ledger and in the roadmap.
#[test]
fn new_story_numbers_are_never_handed_out_twice() {
    let service = start();
    let pairing = service.add("a1", false);
    let config = service.dir.path().join("client/config.json");
    assert!(pair(&config, &pairing.code()).status.success());
    let roadmap = service.dir.path().join("roadmap.md");
    // RAL-7 is written on the roadmap but not in the ledger yet.
    std::fs::write(&roadmap, "- [ ] **RAL-7: Written by hand.**\n").unwrap();
    assert_eq!(ready(&pairing).status, 200); // RAL-13
    let handles: Vec<_> = (0..6)
        .map(|n| {
            let config = config.clone();
            let roadmap = roadmap.clone();
            std::thread::spawn(move || {
                let title = format!("story {n}");
                let out = cli(
                    &config,
                    "r",
                    &[
                        "new-story",
                        "RAL",
                        "--title",
                        &title,
                        "--roadmap",
                        roadmap.to_str().unwrap(),
                    ],
                );
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
                let value: Value = serde_json::from_slice(&out.stdout).unwrap();
                value["story"].as_str().unwrap().to_owned()
            })
        })
        .collect();
    let mut stories: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    stories.sort_by_key(|id| id[4..].parse::<u64>().unwrap());
    assert_eq!(
        stories,
        ["RAL-14", "RAL-15", "RAL-16", "RAL-17", "RAL-18", "RAL-19"]
    );

    // Above the roadmap's numbers too.
    std::fs::write(&roadmap, "- [ ] **RAL-40: Written by hand.**\n").unwrap();
    let out = cli(
        &config,
        "r",
        &[
            "new-story",
            "RAL",
            "--title",
            "t",
            "--roadmap",
            roadmap.to_str().unwrap(),
        ],
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["story"], "RAL-41");
}

/// RAL-53: a designation bound to a credential must name one that's paired and can write.
#[test]
fn a_bound_designation_must_name_a_paired_credential_that_can_write() {
    let service = start();
    let a1 = service.add("a1", false);
    let _viewer = service.add("viewer", true);
    let designate = |credential: &str| {
        write(
            &a1,
            "operator",
            "declared",
            json!({"change": "designate", "watchman": "grand-photon", "credential": credential,
                   "after_seq": 0, "after_hash": "0".repeat(64), "key_fingerprint": "SHA256:x",
                   "signature": "-----BEGIN SSH SIGNATURE-----\nx\n-----END SSH SIGNATURE-----\n"}),
        )
    };
    let ghost = designate("a1-watchman");
    assert_eq!(ghost.status, 409, "{}", ghost.body);
    assert!(ghost.body.contains("isn't paired"), "{}", ghost.body);
    let viewer = designate("viewer");
    assert_eq!(viewer.status, 409, "{}", viewer.body);
    assert!(viewer.body.contains("read-only"), "{}", viewer.body);
    // A paired credential that can write passes this check, and goes on to the operator's key.
    let _watchman = service.add("a1-watchman", false);
    let keyless = designate("a1-watchman");
    assert_eq!(keyless.status, 409, "{}", keyless.body);
    assert!(keyless.body.contains("--secure"), "{}", keyless.body);
}

/// RAL-5: a submission's evidence is checked by Rally itself, recorded as observed beside the
/// evidence. Without a token, it says it wasn't checked, and evidence Rally can't read says why.
#[test]
fn submitted_evidence_is_checked_and_recorded_as_observed() {
    let service = start();
    let pairing = service.add("a1", false);
    let commit = "0123456789abcdef0123456789abcdef01234567";
    for (story, evidence) in [
        ("RAL-60", format!("Castor-o7/rally@{commit}")),
        ("RAL-61", "muster#34".to_owned()),
    ] {
        for body in [
            json!({"change": "ready", "story": story}),
            json!({"change": "claim", "story": story, "team": "rally"}),
            json!({"change": "submit", "story": story, "team": "rally", "evidence": evidence,
                   "untested": "the Windows build"}),
        ] {
            let reply = write(&pairing, "r", "declared", body);
            assert_eq!(reply.status, 200, "{}", reply.body);
        }
    }
    let mut waiting = Value::Null;
    for _ in 0..50 {
        waiting = serde_json::from_str(&call(&pairing, "GET", "/v1/waiting", None).body).unwrap();
        let rows = waiting["waiting"].as_array().unwrap();
        if rows.iter().all(|row| !row["evidence_check"].is_null()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let rows = waiting["waiting"].as_array().unwrap();
    let checked = &rows[0]["evidence_check"];
    assert_eq!(checked["kind"], "observed", "{waiting}");
    assert_eq!(checked["outcome"]["checkable"], true);
    assert_eq!(checked["outcome"]["commit"], commit);
    assert!(
        checked["outcome"]["reason"]
            .as_str()
            .unwrap()
            .contains("no GitHub token")
    );
    assert_eq!(rows[0]["untested"], "the Windows build");
    let unreadable = &rows[1]["evidence_check"]["outcome"];
    assert_eq!(unreadable["checkable"], false, "{waiting}");
    assert!(
        unreadable["reason"]
            .as_str()
            .unwrap()
            .contains("owner/repo")
    );
    // Only Rally writes it.
    let forged = write(
        &pairing,
        "r",
        "observed",
        json!({"change": "evidence_check", "story": "RAL-60", "submit": 3,
               "outcome": {"checkable": true, "exists": true}}),
    );
    assert_eq!(forged.status, 403);
}
