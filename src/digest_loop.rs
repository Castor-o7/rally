//! The watchman's digest loop, started, stopped and restarted by name (RAL-154).
//!
//! `rally digest --every` holds a pid file (digest.pid beside the pairing, or `--pid-file`) while
//! it runs: written at its start, refused if a live digest already holds it, and given the reason
//! when the loop ends by itself. `--stop` ends the digest the file names and removes the file;
//! `--background` starts one on its own; `--restart` does both, logging to digest.log beside the
//! pid file why the previous one ended if nobody asked it to. No process is ever found by pattern:
//! only the pid in the file, checked to be a `rally digest` still, so a reused pid isn't taken for
//! it. Works where there's no `/proc` or systemd, as on a Mac: `kill -0` and `ps -o args=`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// What a digest loop was started with, so `--restart` can start it again the same way.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub quiet_ms: u64,
    pub max_ms: u64,
    pub every_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watchman: Option<String>,
    pub state_file: PathBuf,
    pub config: PathBuf,
    /// Each `--gate` as given (RAL-180), so a restart passes it again with its paths unexpanded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<String>,
}

impl Settings {
    /// The command line, after the program's name, that runs this loop holding `pid_file`.
    pub fn args(&self, pid_file: &Path) -> Vec<String> {
        let secs = |ms: u64| format!("{}s", (ms / 1000).max(1));
        let mut args = vec![
            "--config".to_owned(),
            self.config.display().to_string(),
            "digest".to_owned(),
            "--quiet".to_owned(),
            secs(self.quiet_ms),
            "--max".to_owned(),
            secs(self.max_ms),
            "--every".to_owned(),
            secs(self.every_ms),
            "--state-file".to_owned(),
            self.state_file.display().to_string(),
            "--pid-file".to_owned(),
            pid_file.display().to_string(),
        ];
        if let Some(watchman) = &self.watchman {
            args.extend(["--watchman".to_owned(), watchman.clone()]);
        }
        for gate in &self.gates {
            args.extend(["--gate".to_owned(), gate.clone()]);
        }
        args
    }
}

/// The pid file's contents: the loop holding it, and, once it has ended by itself, how.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    pub pid: u32,
    pub started_ms: i64,
    pub settings: Settings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<Ended>,
}

/// How a loop ended without being asked to: when, its exit status, and its last error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ended {
    pub at_ms: i64,
    pub status: i32,
    pub why: String,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// Whether process `pid` is alive (`kill -0`, which works without `/proc`).
pub fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Process `pid`'s command line, from `ps`, if it's running.
pub fn command_line(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "args=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !text.is_empty()).then_some(text)
}

/// Whether a command line is a `rally digest`: a program named `rally` given `digest`.
pub fn is_digest(command_line: &str) -> bool {
    let mut words = command_line.split_whitespace();
    let is_rally = words
        .next()
        .and_then(|program| Path::new(program).file_name())
        .is_some_and(|name| name == "rally");
    is_rally && words.any(|word| word == "digest")
}

pub fn read(path: &Path) -> Result<Option<Held>, String> {
    crate::view::read_json(path)
}

fn write(path: &Path, held: &Held) -> Result<(), String> {
    // Written whole and renamed into place, so a reader never sees half a file.
    let partial = path.with_extension("pid.partial");
    crate::view::write_json(&partial, held)?;
    std::fs::rename(&partial, path).map_err(|e| format!("can't write {}: {e}", path.display()))
}

/// The digest holding `path`, if one is running: the file names a live process that is still a
/// `rally digest` and hasn't recorded its end.
pub fn live(path: &Path) -> Result<Option<Held>, String> {
    Ok(read(path)?.filter(|held| {
        held.ended.is_none()
            && alive(held.pid)
            && command_line(held.pid).is_some_and(|line| is_digest(&line))
    }))
}

/// Takes `path` for this process, or says which digest already holds it.
pub fn hold(path: &Path, settings: &Settings) -> Result<(), String> {
    let me = std::process::id();
    if let Some(other) = live(path)?
        && other.pid != me
    {
        return Err(format!(
            "a digest already runs as process {}, started {}, holding {}: stop it with `rally \
             digest --stop` or restart it with `rally digest --restart`",
            other.pid,
            when(other.started_ms),
            path.display()
        ));
    }
    write(
        path,
        &Held {
            pid: me,
            started_ms: now_ms(),
            settings: settings.clone(),
            ended: None,
        },
    )
}

/// Records in `path` that this process's loop ended by itself, with its exit status and why.
pub fn ended(path: &Path, status: i32, why: &str) {
    let Ok(Some(mut held)) = read(path) else {
        return;
    };
    if held.pid != std::process::id() {
        return;
    }
    held.ended = Some(Ended {
        at_ms: now_ms(),
        status,
        why: why.to_owned(),
    });
    let _ = write(path, &held);
}

/// What became of a digest the file names that isn't running, for the log: it ended by itself
/// with a recorded status, or with nothing recorded (a signal, a crash, the machine stopping).
pub fn unexpected_end(held: &Held) -> String {
    let started = format!(
        "the digest started {} as process {}",
        when(held.started_ms),
        held.pid
    );
    match &held.ended {
        Some(ended) => format!(
            "{started} ended by itself {}, exit status {}: {}",
            when(ended.at_ms),
            ended.status,
            ended.why
        ),
        None => format!(
            "{started} ended without saying why: killed by a signal, crashed, or the machine \
             stopped"
        ),
    }
}

/// What `--stop` found and did, as a line for the log and the answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// A running digest was stopped, as asked.
    Stopped(u32),
    /// The file named one that had already ended without being asked: how.
    Unexpected(String),
    /// No pid file.
    Nothing,
}

impl Stopped {
    pub fn line(&self) -> String {
        match self {
            Self::Stopped(pid) => format!("stopped the digest, process {pid}, as asked"),
            Self::Unexpected(why) => why.clone(),
            Self::Nothing => "no digest was running".to_owned(),
        }
    }
}

/// Stops the digest `path` names, if it's running, and removes the file.
pub fn stop(path: &Path) -> Result<Stopped, String> {
    let Some(held) = read(path)? else {
        return Ok(Stopped::Nothing);
    };
    let outcome = if live(path)?.is_some() {
        terminate(held.pid)?;
        Stopped::Stopped(held.pid)
    } else {
        Stopped::Unexpected(unexpected_end(&held))
    };
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("can't remove {}: {e}", path.display())),
    }
    Ok(outcome)
}

/// Asks process `pid` to end, and after five seconds makes it.
fn terminate(pid: u32) -> Result<(), String> {
    let signal = |name: &str| {
        std::process::Command::new("kill")
            .args([name, &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("can't run kill: {e}"))
    };
    signal("-TERM")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    if alive(pid) {
        signal("-KILL")?;
        std::thread::sleep(Duration::from_millis(200));
    }
    if alive(pid) {
        return Err(format!("process {pid} didn't stop"));
    }
    Ok(())
}

/// Appends a timestamped line to `log`.
pub fn log_line(log: &Path, line: &str) -> Result<(), String> {
    use std::io::Write;
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("can't make {}: {e}", parent.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .map_err(|e| format!("can't write {}: {e}", log.display()))?;
    let line = serde_json::json!({"at_ms": now_ms(), "rally_digest": line}).to_string();
    writeln!(file, "{line}").map_err(|e| format!("can't write {}: {e}", log.display()))
}

/// Starts `exe` with `settings` on its own, outliving this command, its output appended to `log`,
/// and waits up to ten seconds for it to hold `pid_file`. On failure, says what the log ended with.
pub fn start(exe: &Path, settings: &Settings, pid_file: &Path, log: &Path) -> Result<u32, String> {
    use std::os::unix::process::CommandExt;
    log_line(log, "starting")?;
    let out = std::fs::OpenOptions::new()
        .append(true)
        .open(log)
        .map_err(|e| format!("can't write {}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new(exe)
        .args(settings.args(pid_file))
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(err)
        // Its own process group, so closing the terminal or session doesn't end it.
        .process_group(0)
        .spawn()
        .map_err(|e| format!("can't start {}: {e}", exe.display()))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "the digest stopped at once ({status}): {}",
                tail(log)
            ));
        }
        if read(pid_file)?.is_some_and(|held| held.pid == child.id()) {
            return Ok(child.id());
        }
        if Instant::now() > deadline {
            return Err(format!(
                "the digest didn't take {} within 10 seconds: {}",
                pid_file.display(),
                tail(log)
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn tail(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().rev().take(5).collect();
    let shown: Vec<&str> = lines.into_iter().rev().collect();
    format!("{} ends: {}", log.display(), shown.join(" / "))
}

/// A moment as seconds since the epoch, which `date -d @N` reads.
fn when(ms: i64) -> String {
    format!("at {} (epoch seconds)", ms / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(dir: &Path) -> Settings {
        Settings {
            quiet_ms: 120_000,
            max_ms: 600_000,
            every_ms: 30_000,
            watchman: Some("mellow-weevil".into()),
            state_file: dir.join("digest.json"),
            config: dir.join("config.json"),
            gates: vec!["DSM=~/.local/state/dsm-gates,~/Documents/GitHub/Discoteque".into()],
        }
    }

    #[test]
    fn a_digest_is_told_by_its_command_line() {
        assert!(is_digest("rally digest --every 30s"));
        assert!(is_digest(
            "/home/x/.local/bin/rally --config c digest --quiet 2m"
        ));
        assert!(!is_digest("rally wake --every 30s"));
        assert!(!is_digest("sleep 100"));
        assert!(!is_digest("bash -c while true; do rally digest; done"));
        assert!(!is_digest(""));
    }

    #[test]
    fn the_settings_give_the_command_that_runs_them() {
        let dir = Path::new("/d");
        let args = settings(dir).args(Path::new("/d/digest.pid"));
        assert_eq!(
            args.join(" "),
            "--config /d/config.json digest --quiet 120s --max 600s --every 30s --state-file \
             /d/digest.json --pid-file /d/digest.pid --watchman mellow-weevil --gate \
             DSM=~/.local/state/dsm-gates,~/Documents/GitHub/Discoteque"
        );
        // A pid file from before RAL-180 has no gates, and still reads.
        let old: Settings = serde_json::from_str(
            r#"{"quiet_ms":1,"max_ms":2,"every_ms":3,"state_file":"/s","config":"/c"}"#,
        )
        .unwrap();
        assert!(old.gates.is_empty());
    }

    /// A file naming a process that isn't a digest, or a dead one, holds nothing; one naming this
    /// test's own process doesn't either, since it isn't a `rally digest`.
    #[test]
    fn only_a_live_digest_holds_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("digest.pid");
        assert_eq!(live(&path).unwrap(), None);
        let mut sleeper = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let held = Held {
            pid: sleeper.id(),
            started_ms: 1,
            settings: settings(dir.path()),
            ended: None,
        };
        write(&path, &held).unwrap();
        assert!(alive(sleeper.id()));
        assert_eq!(live(&path).unwrap(), None, "a sleep isn't a digest");
        // So it's taken over, and stopping finds it already ended, not running.
        hold(&path, &settings(dir.path())).unwrap();
        assert_eq!(read(&path).unwrap().unwrap().pid, std::process::id());
        sleeper.kill().unwrap();
        sleeper.wait().unwrap();
        assert!(!alive(sleeper.id()));
    }

    #[test]
    fn an_end_by_itself_is_recorded_and_said_by_the_next_stop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("digest.pid");
        hold(&path, &settings(dir.path())).unwrap();
        ended(&path, 1, "refused (401): bad token");
        let stopped = stop(&path).unwrap();
        let Stopped::Unexpected(why) = &stopped else {
            panic!("{stopped:?}");
        };
        assert!(why.contains("ended by itself"), "{why}");
        assert!(
            why.contains("exit status 1: refused (401): bad token"),
            "{why}"
        );
        assert!(!path.exists());
        assert_eq!(stop(&path).unwrap(), Stopped::Nothing);

        // With nothing recorded, it says so.
        let held = Held {
            pid: 1 << 30,
            started_ms: 5_000,
            settings: settings(dir.path()),
            ended: None,
        };
        assert!(unexpected_end(&held).contains("ended without saying why"));
    }
}
