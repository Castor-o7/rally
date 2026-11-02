//! Runs the real service over TLS and tries each route with and without access.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use rally::access::Pairing;
use rally::client::{Reply, request};
use serde_json::{Value, json};

const ROUTES: [(&str, &str); 5] = [
    ("POST", "/v1/records"),
    ("GET", "/v1/stories"),
    ("GET", "/v1/stories/RAL-13"),
    ("GET", "/v1/teams"),
    ("GET", "/v1/teams/rally"),
];

struct Service {
    child: Child,
    dir: tempfile::TempDir,
    address: String,
    fingerprint: String,
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn rally() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rally"));
    for name in [
        "RALLY_DB",
        "RALLY_CONFIG",
        "RALLY_CREDENTIAL",
        "RALLY_NAME",
        "RALLY_STATE_DIR",
    ] {
        command.env_remove(name);
    }
    command
}

fn start_with(limits: &[&str]) -> Service {
    let dir = tempfile::tempdir().unwrap();
    let mut child = rally()
        .arg("--db")
        .arg(dir.path().join("ledger.db"))
        .args(["serve", "--bind", "127.0.0.1:0"])
        .args(limits)
        .arg("--state-dir")
        .arg(dir.path().join("state"))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let started: Value = serde_json::from_str(&line).unwrap();
    Service {
        child,
        address: started["listening"].as_str().unwrap().to_owned(),
        fingerprint: started["fingerprint"].as_str().unwrap().to_owned(),
        dir,
    }
}

fn start() -> Service {
    start_with(&["--burst", "1000", "--write-burst", "1000"])
}

impl Service {
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
    let service = start_with(&["--burst", "5", "--per-second", "0.001"]);
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
