//! Scratch Postgres for tests: never a real one.
//!
//! With `RALLY_TEST_POSTGRES_URL` set, as CI does, tests use that server. Otherwise each test
//! process starts its own cluster with `initdb` and `pg_ctl` in a new temporary directory, on
//! 127.0.0.1 only, with no Unix socket, and a watcher stops it and removes the directory when the
//! process ends. Either way each test gets a database of its own, dropped afterwards. There's no
//! fallback: if neither works, the test fails rather than skips. It refuses port 5432, so it can't
//! meet a Postgres someone is using.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use postgres::{Client, NoTls};

/// A Postgres server for tests: its address, with no database named.
pub struct Cluster {
    /// For example `postgres://rally@127.0.0.1:54321`.
    pub base: String,
    /// The administration database to connect to for creating others.
    admin_db: String,
    /// Set for a cluster this process started.
    owned: Option<Owned>,
}

struct Owned {
    bin: PathBuf,
    dir: PathBuf,
    data: PathBuf,
}

/// The directory holding `initdb` and `pg_ctl`.
fn bin_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("RALLY_TEST_PG_BIN") {
        return PathBuf::from(dir);
    }
    let mut candidates = vec![PathBuf::from("/opt/homebrew/bin")];
    if let Ok(entries) = std::fs::read_dir("/usr/lib/postgresql") {
        let mut versions: Vec<PathBuf> = entries.flatten().map(|e| e.path().join("bin")).collect();
        versions.sort();
        candidates.extend(versions.into_iter().rev());
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path));
    }
    candidates
        .into_iter()
        .find(|dir| dir.join("initdb").is_file() && dir.join("pg_ctl").is_file())
        .unwrap_or_else(|| {
            panic!(
                "Rally's tests need Postgres: set RALLY_TEST_POSTGRES_URL, or install initdb and \
                 pg_ctl (or set RALLY_TEST_PG_BIN)"
            )
        })
}

fn free_port() -> u16 {
    loop {
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a free loopback port")
            .port();
        if port != 5432 {
            return port;
        }
    }
}

fn unique(prefix: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!(
        "{prefix}_{}_{nanos}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

impl Cluster {
    /// Starts a new cluster of this process's own, which `Drop` stops and removes.
    pub fn start() -> Self {
        let bin = bin_dir();
        let dir = std::env::temp_dir().join(unique("rally-pg"));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let data = dir.join("data");
        let status = Command::new(bin.join("initdb"))
            .arg("-D")
            .arg(&data)
            .args(["-U", "rally", "-A", "trust", "-E", "UTF8", "--no-sync"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !status.as_ref().is_ok_and(std::process::ExitStatus::success) {
            // Nothing else owns the directory yet, so it goes before the test fails.
            let _ = std::fs::remove_dir_all(&dir);
            panic!("initdb in {} failed: {status:?}", bin.display());
        }
        let mut cluster = Self {
            base: String::new(),
            admin_db: "postgres".to_owned(),
            owned: Some(Owned { bin, dir, data }),
        };
        for attempt in 0.. {
            let port = free_port();
            assert_ne!(port, 5432);
            let conf = format!(
                "\nlisten_addresses = '127.0.0.1'\nport = {port}\nunix_socket_directories = ''\n\
                 max_connections = 400\n"
            );
            let owned = cluster.owned.as_ref().expect("owned");
            let path = owned.data.join("postgresql.auto.conf");
            std::fs::write(&path, conf).expect("postgresql.auto.conf");
            cluster.base = format!("postgres://rally@127.0.0.1:{port}");
            if cluster.pg_ctl_start() {
                break;
            }
            assert!(attempt < 5, "the scratch Postgres wouldn't start");
        }
        cluster
    }

    fn pg_ctl_start(&self) -> bool {
        let owned = self.owned.as_ref().expect("only an owned cluster starts");
        Command::new(owned.bin.join("pg_ctl"))
            .arg("-D")
            .arg(&owned.data)
            .arg("-l")
            .arg(owned.dir.join("log"))
            .args(["-w", "-t", "60", "start"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// Starts this cluster again after [`Cluster::stop`].
    pub fn restart(&self) {
        assert!(
            self.pg_ctl_start(),
            "the scratch Postgres wouldn't start again"
        );
    }

    /// Stops this cluster, as an outage would.
    pub fn stop(&self) {
        let owned = self.owned.as_ref().expect("only an owned cluster stops");
        let status = Command::new(owned.bin.join("pg_ctl"))
            .arg("-D")
            .arg(&owned.data)
            .args(["-m", "fast", "-w", "stop"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("pg_ctl runs");
        assert!(status.success(), "the scratch Postgres wouldn't stop");
    }

    /// The scratch directory, for a test to check it's gone.
    pub fn directory(&self) -> Option<&Path> {
        self.owned.as_ref().map(|owned| owned.dir.as_path())
    }

    fn admin(&self) -> Client {
        let url = format!("{}/{}", self.base, self.admin_db);
        Client::connect(&url, NoTls).unwrap_or_else(|e| panic!("can't reach test Postgres: {e}"))
    }

    /// A new, empty database on this cluster, dropped when the result is.
    pub fn database(&self) -> Database<'_> {
        let name = unique("rally_test");
        self.admin()
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .expect("CREATE DATABASE");
        Database {
            url: format!("{}/{name}", self.base),
            name,
            cluster: self,
        }
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        if let Some(owned) = &self.owned {
            let _ = Command::new(owned.bin.join("pg_ctl"))
                .arg("-D")
                .arg(&owned.data)
                .args(["-m", "immediate", "-w", "stop"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = std::fs::remove_dir_all(&owned.dir);
        }
    }
}

/// The cluster this test process shares between its tests.
pub fn shared() -> &'static Cluster {
    static SHARED: OnceLock<Cluster> = OnceLock::new();
    SHARED.get_or_init(|| {
        if let Ok(url) = std::env::var("RALLY_TEST_POSTGRES_URL") {
            if let Err(reason) = safe_to_use(&url) {
                panic!("RALLY_TEST_POSTGRES_URL: {reason}");
            }
            let (base, admin_db) = url.rsplit_once('/').expect("a URL ending in /database");
            return Cluster {
                base: base.to_owned(),
                admin_db: admin_db.to_owned(),
                owned: None,
            };
        }
        let cluster = Cluster::start();
        // Statics are never dropped, so a watcher stops the cluster when this process ends, however
        // it ends.
        let owned = cluster.owned.as_ref().expect("owned");
        let script = format!(
            "while kill -0 {pid} 2>/dev/null; do sleep 1; done; \
             '{pg_ctl}' -D '{data}' -m immediate -w stop >/dev/null 2>&1; rm -rf '{dir}'",
            pid = std::process::id(),
            pg_ctl = owned.bin.join("pg_ctl").display(),
            data = owned.data.display(),
            dir = owned.dir.display(),
        );
        // It has to outlive this process, so nothing here waits for it.
        #[allow(clippy::zombie_processes)]
        Command::new("/bin/sh")
            .args(["-c", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the cleanup watcher starts");
        cluster
    })
}

/// Refuses a server URL that names port 5432, or no port, which means 5432, or that can't be read.
pub fn safe_to_use(url: &str) -> Result<(), String> {
    if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
        return Err("give a postgres:// URL ending in an administration database".to_owned());
    }
    let config: postgres::Config = url.parse().map_err(|e| format!("can't be read: {e}"))?;
    let ports = config.get_ports();
    if ports.is_empty() || ports.contains(&5432) {
        return Err("tests refuse port 5432, which a URL with no port also means".to_owned());
    }
    Ok(())
}

/// A database for one test.
pub struct Database<'a> {
    pub url: String,
    pub name: String,
    cluster: &'a Cluster,
}

impl Database<'_> {
    /// A plain connection, for tests that tamper with what's stored.
    pub fn connect(&self) -> Client {
        Client::connect(&self.url, NoTls).expect("connect to the test database")
    }
}

impl Drop for Database<'_> {
    fn drop(&mut self) {
        // FORCE ends myko's replay connections, which have no shutdown of their own.
        let url = format!("{}/{}", self.cluster.base, self.cluster.admin_db);
        if let Ok(mut admin) = Client::connect(&url, NoTls) {
            let drop = format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name);
            let _ = admin.batch_execute(&drop);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::safe_to_use;

    #[test]
    fn the_default_port_is_refused_however_it_is_written() {
        for url in [
            "postgres://postgres@127.0.0.1/postgres",
            "postgres://postgres@127.0.0.1:5432/postgres",
            "postgresql://postgres@localhost/postgres",
            "postgres://postgres@127.0.0.1:54329,127.0.0.1:5432/postgres",
            "host=127.0.0.1 port=54329 dbname=postgres",
            "postgres://postgres@127.0.0.1:not-a-port/postgres",
        ] {
            assert!(safe_to_use(url).is_err(), "{url}");
        }
        assert!(safe_to_use("postgres://postgres:pw@127.0.0.1:54329/postgres").is_ok());
    }
}
