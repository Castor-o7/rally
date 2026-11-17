//! RAL-199 against a real Marshal daemon: Rally reads the roster as a Myko client of the daemon.
//!
//! Ignored by default, since it needs upstream Marshal's `marshal-daemon` and `marshal-shim`
//! (0.60.2). It starts its own ephemeral daemon on free loopback ports, never the live one. Run it
//! with `cargo test --test marshal -- --ignored --nocapture`; `RALLY_TEST_MARSHAL_DAEMON` and
//! `RALLY_TEST_MARSHAL_SHIM` name the binaries when they aren't on `PATH`.

use std::io::Write as _;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a free port")
        .local_addr()
        .expect("its address")
        .port()
}

fn binary(env: &str, name: &str) -> String {
    std::env::var(env).unwrap_or_else(|_| name.to_owned())
}

/// An ephemeral daemon (no Postgres, so it keeps nothing), and its WebSocket address.
fn daemon() -> (Killed, String) {
    let (ws, hook) = (free_port(), free_port());
    let child = Command::new(binary("RALLY_TEST_MARSHAL_DAEMON", "marshal-daemon"))
        .env("MARSHAL_BIND", format!("127.0.0.1:{ws}"))
        .env("MARSHAL_HOOK_BIND", format!("127.0.0.1:{hook}"))
        .env_remove("MYKO_POSTGRES_URL")
        .env_remove("MARSHAL_AUTH_POSTGRES_URL")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("marshal-daemon runs: set RALLY_TEST_MARSHAL_DAEMON");
    let until = Instant::now() + Duration::from_secs(20);
    while std::net::TcpStream::connect(("127.0.0.1", ws)).is_err() {
        assert!(
            Instant::now() < until,
            "the daemon didn't listen on {ws} within 20s"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    (Killed(child), format!("ws://127.0.0.1:{ws}"))
}

/// The session id the throwaway shim is given, so the test never borrows the identity of whoever
/// runs it (a Claude Code session's own `CLAUDE_CODE_SESSION_ID`, which the shim reads first).
const TEST_SESSION_ID: &str = "e2e00199-0000-4000-8000-000000000199";

/// A real shim connected to `address`, as a session would run it, with an empty config directory
/// so no address file on this machine overrides the throwaway daemon's. Nothing of the runner's
/// own session reaches it: every inherited `CLAUDE*`, `MARSHAL_*` and `MYKO_*` variable is removed,
/// and it's given [`TEST_SESSION_ID`].
fn shim(address: &str, config: &std::path::Path, cwd: &std::path::Path) -> Killed {
    let mut command = Command::new(binary("RALLY_TEST_MARSHAL_SHIM", "marshal-shim"));
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy();
        if name.starts_with("CLAUDE") || name.starts_with("MARSHAL_") || name.starts_with("MYKO_") {
            command.env_remove(name.as_ref());
        }
    }
    let mut child = command
        .env("CLAUDE_CODE_SESSION_ID", TEST_SESSION_ID)
        .env("MARSHAL_DAEMON_ADDRESS", address)
        .env("XDG_CONFIG_HOME", config)
        .env("HOME", config)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("marshal-shim runs: set RALLY_TEST_MARSHAL_SHIM");
    // The MCP handshake a harness sends first, so the shim serves as it would for a session.
    let stdin = child.stdin.as_mut().expect("its stdin");
    let hello = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "rally-e2e", "version": "0"}}});
    writeln!(stdin, "{hello}").expect("the handshake is written");
    Killed(child)
}

fn sessions(roster: &Value) -> Vec<Value> {
    roster["sessions"].as_array().cloned().unwrap_or_default()
}

#[test]
#[ignore = "needs upstream Marshal 0.60.2's marshal-daemon and marshal-shim"]
fn rally_reads_a_real_daemon_s_roster_and_puts_no_row_on_it() {
    let (_daemon, address) = daemon();
    let limit = Duration::from_secs(10);

    // An empty daemon gives an empty roster, not an error: Rally's own read adds no row.
    let empty = rally::marshal::roster_at(&address, limit).expect("a roster from the daemon");
    println!("empty roster: {empty}");
    assert_eq!(
        sessions(&empty).len(),
        0,
        "Rally's reading client put a row on the roster"
    );

    let config = tempfile::tempdir().expect("a config dir");
    let cwd = tempfile::tempdir().expect("a session dir");
    let _shim = shim(&address, config.path(), cwd.path());
    let until = Instant::now() + Duration::from_secs(20);
    let roster = loop {
        let roster = rally::marshal::roster_at(&address, limit).expect("a roster from the daemon");
        if !sessions(&roster).is_empty() {
            break roster;
        }
        assert!(
            Instant::now() < until,
            "the shim's session never reached the roster"
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    println!("roster with one shim: {roster:#}");
    let rows = sessions(&roster);
    assert_eq!(rows.len(), 1, "one shim, one row, and none for Rally");
    let row = &rows[0];
    assert_eq!(
        row["session_id"], TEST_SESSION_ID,
        "the shim took the runner's identity"
    );
    assert!(
        row["nickname"]
            .as_str()
            .is_some_and(|name| name.contains('-'))
    );
    assert_eq!(
        row["declared"]["cwd"],
        cwd.path().to_string_lossy().as_ref()
    );
    assert_eq!(row["recorded"]["bound_to_connection"], true);

    // The viewer's reading of it: available, with the session under its labels.
    let shaped = rally::web::shape_roster(&roster, 1);
    println!("as the viewer shapes it: {shaped:#}");
    assert_eq!(shaped["available"], true);
    assert_eq!(shaped["sessions"][0]["nickname"], row["nickname"]);
    assert_eq!(shaped["sessions"][0]["machine_label"], "declared");
    assert_eq!(shaped["sessions"][0]["connection"]["label"], "recorded");
}

#[test]
#[ignore = "part of the RAL-199 e2e run against Marshal"]
fn an_unreachable_daemon_is_said_at_once_and_a_silent_one_within_the_limit() {
    let address = format!("ws://127.0.0.1:{}", free_port());
    let started = Instant::now();
    let error = rally::marshal::roster_at(&address, Duration::from_secs(2)).unwrap_err();
    println!("unreachable: {error} after {:?}", started.elapsed());
    assert!(error.contains("couldn't be reached"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(1));

    // Something that takes the connection but never speaks Myko.
    let silent = TcpListener::bind("127.0.0.1:0").expect("a listener");
    let address = format!("ws://{}", silent.local_addr().expect("its address"));
    let started = Instant::now();
    let error = rally::marshal::roster_at(&address, Duration::from_secs(2)).unwrap_err();
    println!("silent: {error} after {:?}", started.elapsed());
    assert!(error.contains("within 2s"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(4));
}

/// Every message the daemon at `address` holds, read as a Myko client, or why not.
fn messages(address: &str) -> Result<Vec<std::sync::Arc<marshal_entities::Message>>, String> {
    use myko::prelude::Gettable as _;
    marshal_entities::link();
    let client = myko::client::MykoClient::new_with_auto_reconnect(false);
    let watch = client
        .watch_query_state::<marshal_entities::GetAllMessages>(marshal_entities::GetAllMessages {});
    client.set_address(Some(address.to_owned()));
    let until = Instant::now() + Duration::from_secs(10);
    let result = loop {
        if let Some(error) = watch.error().get() {
            break Err(error);
        }
        if watch.ready().get() {
            break Ok(watch.items().get());
        }
        if Instant::now() >= until {
            break Err("no answer".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    client.close();
    result
}

#[test]
#[ignore = "needs upstream Marshal 0.60.2's marshal-daemon and marshal-shim"]
fn rally_sends_as_its_own_service_session_and_leaves_no_row() {
    let (_daemon, address) = daemon();
    let limit = Duration::from_secs(10);
    let config = tempfile::tempdir().expect("a config dir");
    let cwd = tempfile::tempdir().expect("a session dir");
    let _shim = shim(&address, config.path(), cwd.path());
    let until = Instant::now() + Duration::from_secs(20);
    let watchman = loop {
        let roster = rally::marshal::roster_at(&address, limit).expect("a roster");
        if let Some(row) = sessions(&roster).into_iter().next() {
            break row;
        }
        assert!(
            Instant::now() < until,
            "the shim's session never reached the roster"
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    let nickname = watchman["nickname"]
        .as_str()
        .expect("a nickname")
        .to_owned();
    let id = watchman["session_id"].as_str().expect("an id").to_owned();
    assert_eq!(id, TEST_SESSION_ID, "the shim took the runner's identity");

    // To someone who isn't there: nothing is sent, and another source may be tried.
    let absent = rally::marshal::send_at(
        &address,
        "rally-digest@test",
        "/state",
        "no-such-team",
        "x",
        limit,
    );
    println!("to an absent team: {absent:?}");
    assert!(matches!(absent, rally::marshal::Sent::NotHere(ref why) if why.contains("isn't on")));

    let body = "<rally_digest>RAL-199 e2e: from Rally's digest</rally_digest>";
    let sent = rally::marshal::send_at(
        &address,
        "rally-digest@test",
        "/state",
        &nickname,
        body,
        limit,
    );
    println!("to {nickname}: {sent:?}");
    let rally::marshal::Sent::Delivered {
        to_session_id,
        message_id,
    } = sent
    else {
        panic!("not delivered: {sent:?}");
    };
    assert_eq!(to_session_id, id);

    // By the session's id, as `rally wake` sends to a team's one session (RAL-199 part 3): the
    // same session, so a team found by its Marshal project name is still reached.
    let by_id = rally::marshal::send_at(
        &address,
        "rally-wake@test",
        "/state",
        &id,
        "<rally_wake>e2e</rally_wake>",
        limit,
    );
    println!("by id {id}: {by_id:?}");
    assert!(
        matches!(by_id, rally::marshal::Sent::Delivered { ref to_session_id, .. } if *to_session_id == id),
        "{by_id:?}"
    );

    // The daemon holds it, from Rally's own session, never the recipient's or an agent's.
    match messages(&address) {
        Ok(all) => {
            let held = all
                .iter()
                .find(|m| m.id.0.as_ref() == message_id)
                .expect("the daemon holds the message");
            println!(
                "held: from {} to {:?}: {}",
                held.from_session_id.0, held.to_session_id, held.body
            );
            assert_eq!(held.from_session_id.0.as_ref(), "rally-digest@test");
            assert_eq!(held.body, body);
        }
        Err(why) => println!(
            "the daemon wouldn't list messages to a reader ({why}); delivery rests on its answer"
        ),
    }

    // Rally's session is gone again: only the shim's row, and no service row left behind.
    let after = rally::marshal::roster_at(&address, limit).expect("a roster");
    println!("roster after: {after}");
    assert_eq!(sessions(&after).len(), 1);
    assert_eq!(
        after["service_sessions"], 0,
        "Rally's service session was left behind"
    );
}
