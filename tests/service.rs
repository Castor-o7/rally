//! Runs the real service over TLS and tries each route with and without access.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rally::access::Pairing;
use rally::client::{Reply, request};
use rally::scratch_pg::{self, Cluster, Database};
use serde_json::{Value, json};

const ROUTES: [(&str, &str); 7] = [
    ("POST", "/v1/records"),
    ("GET", "/v1/stories"),
    ("GET", "/v1/stories/RAL-13"),
    ("GET", "/v1/teams"),
    ("GET", "/v1/teams/rally"),
    ("GET", "/v1/check"),
    ("GET", "/v1/whoami"),
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
    let service = start();
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
    let checked = cli(&config, "x", &["check"]);
    assert!(checked.status.success());
    let checked: Value = serde_json::from_slice(&checked.stdout).unwrap();
    let head = format!("3:{}", checked["head"]["hash"].as_str().unwrap());

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

    // Rewriting record 2 and re-signing 3, as events through myko's store would.
    stopped(&mut service);
    let (text2, _) = stored(&service, 2);
    let (text3, _) = stored(&service, 3);
    assert!(text2.contains("silken-tanager"));
    let (_, hash1) = stored(&service, 1);
    let forged2 = text2.replace("silken-tanager", "mallory");
    let hash2 = chain_hash(&hash1, 2, &forged2);
    let hash3 = chain_hash(&hash2, 3, &text3);
    as_myko_would(
        &service,
        &[
            (2, "SET", Some((&forged2, &hash2))),
            (3, "SET", Some((&text3, &hash3))),
        ],
    );
    let forged_head = format!("3:{hash3}");
    std::fs::write(
        service.head_file(),
        json!({"seq": 3, "hash": hash3}).to_string(),
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
         DELETE FROM rally_events WHERE tx <> 'forged' AND item_id IN ('2', '3');",
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
