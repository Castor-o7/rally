//! Findings from the second read of RAL-39, each shown through the real `rally` binary.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Output, Stdio};

use rally::scratch_pg::{self, Cluster, Database};
use serde_json::{Value, json};

fn rally() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rally"));
    for name in [
        "RALLY_DB",
        "RALLY_HEAD_FILE",
        "RALLY_DB_PASSWORD_FILE",
        "RALLY_CONFIG",
        "RALLY_CREDENTIAL",
        "RALLY_NAME",
        "RALLY_STATE_DIR",
    ] {
        command.env_remove(name);
    }
    command
}

fn on(url: &str, dir: &Path) -> Command {
    let mut command = rally();
    command.arg("--db").arg(url);
    command.arg("--head-file").arg(dir.join("ledger.head"));
    command
}

fn ready(url: &str, dir: &Path, story: &str) -> Output {
    on(url, dir)
        .args(["--credential", "a1", "--as", "x", "ready", story])
        .output()
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A row myko would load as record 3, with columns that say it's something else.
const DISGUISED: &str = "INSERT INTO rally_events (item_type, item_id, change_type, tx, event)
    VALUES ('X', 'x', 'SET', 'forged', jsonb_build_object(
        'item', jsonb_build_object('id', '3', 'record', 'forged', 'hash', repeat('f', 64)),
        'changeType', 'SET', 'itemType', 'LedgerRecord'))";

fn two_records(db: &Database, dir: &Path) {
    for story in ["RAL-1", "RAL-2"] {
        let output = ready(&db.url, dir, story);
        assert!(output.status.success(), "{}", stderr(&output));
    }
}

#[test]
fn a_row_whose_columns_disguise_a_record_is_refused() {
    let db = scratch_pg::shared().database();
    let dir = tempfile::tempdir().unwrap();
    two_records(&db, dir.path());
    let error = db.connect().batch_execute(DISGUISED);
    assert!(error.is_err(), "Postgres took a disguised record");
}

#[test]
fn a_disguised_row_already_there_is_reported_by_check() {
    let db = scratch_pg::shared().database();
    let dir = tempfile::tempdir().unwrap();
    two_records(&db, dir.path());
    let checked = on(&db.url, dir.path()).arg("check").output().unwrap();
    let head: Value = serde_json::from_slice(&checked.stdout).unwrap();
    let head = format!("2:{}", head["head"]["hash"].as_str().unwrap());
    db.connect()
        .batch_execute(&format!(
            "ALTER TABLE rally_events DROP CONSTRAINT IF EXISTS rally_row_is_its_event; {DISGUISED}"
        ))
        .unwrap();
    let output = on(&db.url, dir.path())
        .args(["check", "--expect", &head])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "check passed over a disguised record"
    );
    // And no writer starts over it.
    let output = ready(&db.url, dir.path(), "RAL-3");
    assert!(!output.status.success(), "a write went ahead over it");
}

#[test]
fn passwords_never_reach_a_message() {
    for url in [
        "postgres://rally:hunter2secret@127.0.0.1:1/rally",
        "postgres://rally@127.0.0.1:1/rally?password=hunter2secret",
        "host=127.0.0.1 port=1 user=rally password=hunter2secret dbname=rally",
        "host=127.0.0.1 port=1 user=rally password='hunter2secret' dbname=rally",
    ] {
        let shown = rally::store::redact(url);
        assert!(!shown.contains("hunter2secret"), "{url} showed as {shown}");
        assert!(shown.contains("rally"), "{shown}");
        let dir = tempfile::tempdir().unwrap();
        let output = on(url, dir.path()).arg("check").output().unwrap();
        assert!(!output.status.success());
        assert!(
            !stderr(&output).contains("hunter2secret"),
            "{}",
            stderr(&output)
        );
    }
}

/// Starts the service, and returns it once it's listening.
fn serve(db: &Database, dir: &Path) -> std::process::Child {
    let mut child = on(&db.url, dir)
        .args(["serve", "--bind", "127.0.0.1:0", "--min-free-bytes", "0"])
        .arg("--state-dir")
        .arg(dir.join("state"))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert!(line.contains("listening"), "{line}");
    // It prints before it opens the ledger: wait until it holds the writer lock.
    let mut sql = db.connect();
    for _ in 0..400 {
        let row = sql
            .query_one(
                "SELECT count(*) FROM pg_locks l JOIN pg_database d ON d.oid = l.database
                 WHERE l.locktype = 'advisory' AND d.datname = current_database()",
                &[],
            )
            .unwrap();
        if row.get::<_, i64>(0) > 0 {
            return child;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("the service never took the writer lock")
}

#[test]
fn adopt_head_is_refused_while_the_service_runs() {
    let db = scratch_pg::shared().database();
    let dir = tempfile::tempdir().unwrap();
    two_records(&db, dir.path());
    let mut service = serve(&db, dir.path());
    let before = std::fs::read_to_string(dir.path().join("ledger.head")).unwrap();
    let output = on(&db.url, dir.path()).arg("adopt-head").output().unwrap();
    let _ = service.kill();
    let _ = service.wait();
    assert!(
        !output.status.success(),
        "adopt-head ran beside the service"
    );
    assert!(
        stderr(&output).contains("another process is writing"),
        "{}",
        stderr(&output)
    );
    let after = std::fs::read_to_string(dir.path().join("ledger.head")).unwrap();
    assert_eq!(before, after);

    // With the service stopped, it works, and never creates a ledger.
    let output = on(&db.url, dir.path()).arg("adopt-head").output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let empty = scratch_pg::shared().database();
    let output = on(&empty.url, dir.path())
        .arg("adopt-head")
        .output()
        .unwrap();
    assert!(stderr(&output).contains("no ledger"), "{}", stderr(&output));
}

#[test]
fn a_password_comes_from_a_private_file() {
    let cluster = Cluster::start();
    let db = cluster.database();
    let mut admin = db.connect();
    admin
        .batch_execute(
            "CREATE ROLE secret LOGIN PASSWORD 'correct horse';
             GRANT ALL ON SCHEMA public TO secret;",
        )
        .unwrap();
    let data = cluster.directory().unwrap().join("data");
    std::fs::write(
        data.join("pg_hba.conf"),
        "host all secret 127.0.0.1/32 scram-sha-256\nhost all all 127.0.0.1/32 trust\n",
    )
    .unwrap();
    admin.batch_execute("SELECT pg_reload_conf()").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let url = db.url.replacen("rally@", "secret@", 1);
    let dir = tempfile::tempdir().unwrap();

    // Without the password, Postgres refuses.
    let output = ready(&url, dir.path(), "RAL-1");
    assert!(!output.status.success());

    let password = dir.path().join("password");
    std::fs::write(&password, "correct horse\n").unwrap();
    let with = |file: &Path, args: &[&str]| {
        on(&url, dir.path())
            .env("RALLY_DB_PASSWORD_FILE", file)
            .args(args)
            .output()
            .unwrap()
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o644)).unwrap();
        let output = with(
            &password,
            &["--credential", "a1", "--as", "x", "ready", "RAL-1"],
        );
        assert!(
            !output.status.success(),
            "a password others can read was used"
        );
        assert!(stderr(&output).contains("private"), "{}", stderr(&output));
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let output = with(
        &password,
        &["--credential", "a1", "--as", "x", "ready", "RAL-1"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let output = with(&password, &["check"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["head"]["seq"], json!(1));
    assert!(!stderr(&output).contains("correct horse"));
}

/// A cluster with a role, `secret`, that needs the password "correct horse", and its URL.
fn password_cluster(cluster: &Cluster) -> (Database<'_>, String) {
    let db = cluster.database();
    let mut admin = db.connect();
    admin
        .batch_execute(
            "CREATE ROLE secret LOGIN PASSWORD 'correct horse';
             GRANT ALL ON SCHEMA public TO secret;",
        )
        .unwrap();
    let data = cluster.directory().unwrap().join("data");
    std::fs::write(
        data.join("pg_hba.conf"),
        "host all secret 127.0.0.1/32 scram-sha-256\nhost all all 127.0.0.1/32 trust\n",
    )
    .unwrap();
    admin.batch_execute("SELECT pg_reload_conf()").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let url = db.url.replacen("rally@", "secret@", 1);
    (db, url)
}

fn private_password_file(dir: &Path) -> std::path::PathBuf {
    let password = dir.join("password");
    std::fs::write(&password, "correct horse\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    password
}

#[test]
fn a_failure_inside_myko_never_shows_the_password() {
    let cluster = Cluster::start();
    let (db, url) = password_cluster(&cluster);
    let dir = tempfile::tempdir().unwrap();
    let password = private_password_file(dir.path());
    let write = || {
        on(&url, dir.path())
            .env("RALLY_DB_PASSWORD_FILE", &password)
            .args(["--credential", "a1", "--as", "x", "ready", "RAL-1"])
            .output()
            .unwrap()
    };
    let output = write();
    assert!(output.status.success(), "{}", stderr(&output));
    // Enough rows that myko's replay can't finish within the role's own one-millisecond limit,
    // which Rally's connections override and myko's don't, so only myko's startup fails.
    db.connect()
        .batch_execute(
            "INSERT INTO rally_events (item_type, item_id, change_type, tx, event)
             SELECT 'LedgerRecord', n::text, 'SET', 't', jsonb_build_object(
                 'item', jsonb_build_object('id', n::text, 'record', 'x', 'hash', 'y'),
                 'changeType', 'SET', 'itemType', 'LedgerRecord')
             FROM generate_series(2, 60000) n;
             ALTER ROLE secret SET statement_timeout = 1;",
        )
        .unwrap();
    let output = write();
    assert!(!output.status.success(), "myko's replay was meant to fail");
    let text = format!(
        "{}{}",
        stderr(&output),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        text.contains("statement timeout") || text.contains("replay"),
        "{text}"
    );
    for secret in ["correct horse", "correct%20horse", "correct+horse"] {
        assert!(!text.contains(secret), "the password showed: {text}");
    }
}

#[test]
fn help_never_shows_the_database_address_or_password_file() {
    let output = rally()
        .env(
            "RALLY_DB",
            "postgres://rally:hunter2secret@127.0.0.1:1/rally",
        )
        .env("RALLY_DB_PASSWORD_FILE", "/secret/place/hunter2file")
        .arg("--help")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("--db"), "{text}");
    assert!(!text.contains("hunter2"), "{text}");
}

#[test]
fn a_password_twice_or_a_password_file_alone_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let password = private_password_file(dir.path());
    let both = on("postgres://rally:inline@127.0.0.1:1/rally", dir.path())
        .env("RALLY_DB_PASSWORD_FILE", &password)
        .arg("check")
        .output()
        .unwrap();
    assert!(!both.status.success());
    assert!(stderr(&both).contains("both"), "{}", stderr(&both));
    let alone = rally()
        .env("RALLY_DB_PASSWORD_FILE", &password)
        .args(["--config", "/nonexistent/client.json", "check"])
        .output()
        .unwrap();
    assert!(!alone.status.success());
    assert!(stderr(&alone).contains("--db"), "{}", stderr(&alone));
}

/// Record 3 with its id as a number in the event, and as text in the column.
const NUMERIC_ID: &str = "INSERT INTO rally_events (item_type, item_id, change_type, tx, event)
    VALUES ('LedgerRecord', '3', 'SET', 'forged', jsonb_build_object(
        'item', jsonb_build_object('id', 3, 'record', 'forged', 'hash', repeat('f', 64)),
        'changeType', 'SET', 'itemType', 'LedgerRecord'))";

#[test]
fn a_record_whose_id_isnt_text_is_refused_and_reported() {
    let db = scratch_pg::shared().database();
    let dir = tempfile::tempdir().unwrap();
    two_records(&db, dir.path());
    assert!(
        db.connect().batch_execute(NUMERIC_ID).is_err(),
        "Postgres took a record whose id is a number"
    );
    db.connect()
        .batch_execute(&format!(
            "ALTER TABLE rally_events DROP CONSTRAINT rally_row_is_its_event; {NUMERIC_ID}"
        ))
        .unwrap();
    let output = on(&db.url, dir.path()).arg("check").output().unwrap();
    assert!(!output.status.success(), "check passed over it");
}
