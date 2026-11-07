//! `rally view` (RAL-82): one command that starts the viewer if it isn't running, restarts it when
//! it runs an older build or other settings, waits for it to answer, and opens it signed in.
//!
//! The viewer's settings are remembered in `view-settings.json` beside the pairing, so a restart
//! needs no arguments. What's running is kept in `view-run.json` there too: its process, the build
//! it runs (the binary's SHA-256) and the settings it was started with. Its output goes to
//! `view.log`. None of them is `viewer.json`, which is the viewer's read-only pairing. The viewer
//! is started with that pairing when it exists, so it reads with a credential that can't write.
//! A viewer started by hand with `rally web` is found by its command line, but only stopped when
//! asked to with `--restart` or `--stop`.

use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// What the viewer is started with. Each `rally view` flag that's given replaces the saved one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub bind: Option<String>,
    pub projects: Option<PathBuf>,
    pub backup_dir: Option<PathBuf>,
    pub summaries: Vec<String>,
    pub watchman_summaries: Option<PathBuf>,
    pub key_file: Option<PathBuf>,
}

pub const DEFAULT_BIND: &str = "127.0.0.1:7300";
pub const SETTINGS_FILE: &str = "view-settings.json";
pub const RUN_FILE: &str = "view-run.json";
pub const LOG_FILE: &str = "view.log";
/// The read-only pairing a viewer reads with, beside the writable one (see docs/web.md).
pub const VIEWER_PAIRING: &str = "viewer.json";

/// The pairing the viewer is started with: the read-only `viewer.json` beside `config` when it
/// exists, else `config` itself.
pub fn viewer_pairing(config: &Path) -> PathBuf {
    let read_only = config.with_file_name(VIEWER_PAIRING);
    if read_only.is_file() {
        read_only
    } else {
        config.to_owned()
    }
}

impl Settings {
    /// These settings with each one `given` names put in its place.
    pub fn merged(&self, given: &Settings) -> Settings {
        Settings {
            bind: given.bind.clone().or_else(|| self.bind.clone()),
            projects: given.projects.clone().or_else(|| self.projects.clone()),
            backup_dir: given.backup_dir.clone().or_else(|| self.backup_dir.clone()),
            summaries: if given.summaries.is_empty() {
                self.summaries.clone()
            } else {
                given.summaries.clone()
            },
            watchman_summaries: given
                .watchman_summaries
                .clone()
                .or_else(|| self.watchman_summaries.clone()),
            key_file: given.key_file.clone().or_else(|| self.key_file.clone()),
        }
    }

    pub fn bind(&self) -> &str {
        self.bind.as_deref().unwrap_or(DEFAULT_BIND)
    }

    /// The `rally web` arguments these settings make.
    pub fn web_args(&self) -> Vec<String> {
        let mut args = vec![
            "web".to_owned(),
            "--bind".to_owned(),
            self.bind().to_owned(),
        ];
        let mut path = |flag: &str, value: &Option<PathBuf>| {
            if let Some(value) = value {
                args.push(flag.to_owned());
                args.push(value.display().to_string());
            }
        };
        path("--projects", &self.projects);
        path("--backup-dir", &self.backup_dir);
        path("--watchman-summaries", &self.watchman_summaries);
        path("--key-file", &self.key_file);
        for pair in &self.summaries {
            args.push("--summaries".to_owned());
            args.push(pair.clone());
        }
        args
    }
}

/// The viewer this command started: its process, build and settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Running {
    pub pid: u32,
    pub build: String,
    pub settings: Settings,
}

/// Why a running viewer is restarted, if it is.
pub fn why_restart(running: &Running, build: &str, settings: &Settings) -> Option<&'static str> {
    if running.build != build {
        Some("it runs another build")
    } else if running.settings != *settings {
        Some("its settings changed")
    } else {
        None
    }
}

/// A file's SHA-256, as hex: the build a binary is.
pub fn build_of(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub fn read_json<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<Option<T>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("{} can't be read: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("can't read {}: {e}", path.display())),
    }
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("can't make {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("can't write {}: {e}", path.display()))
}

/// Whether something answers at `bind`.
pub fn answering(bind: &str) -> bool {
    bind.parse::<SocketAddr>().is_ok_and(|address| {
        TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok()
    })
}

/// Whether process `pid` is alive.
pub fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Whether a command line is a viewer serving at `bind`: `rally web` without `--login` or
/// `--new-key`, with that `--bind` or none and `bind` the default.
pub fn is_viewer(args: &[String], bind: &str) -> bool {
    let Some(name) = args.first() else {
        return false;
    };
    let is_rally = Path::new(name).file_name().is_some_and(|n| n == "rally");
    let rest = &args[1..];
    let Some(web) = rest.iter().position(|a| a == "web") else {
        return false;
    };
    let after = &rest[web + 1..];
    if after.iter().any(|a| a == "--login" || a == "--new-key") {
        return false;
    }
    let bound = after
        .iter()
        .position(|a| a == "--bind")
        .and_then(|i| after.get(i + 1))
        .map(String::as_str)
        .or_else(|| after.iter().find_map(|a| a.strip_prefix("--bind=")))
        .unwrap_or(DEFAULT_BIND);
    is_rally && bound == bind
}

/// This user's viewer processes serving at `bind`, found by command line in `/proc`.
pub fn find_viewers(bind: &str) -> Vec<u32> {
    use std::os::unix::fs::MetadataExt;
    let me = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            if pid == std::process::id() || entry.metadata().ok().map(|m| m.uid()) != me {
                return None;
            }
            let raw = std::fs::read(entry.path().join("cmdline")).ok()?;
            let args: Vec<String> = raw
                .split(|b| *b == 0)
                .filter(|a| !a.is_empty())
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect();
            is_viewer(&args, bind).then_some(pid)
        })
        .collect()
}

/// Stops process `pid`: asks it to end, and after five seconds makes it.
pub fn stop(pid: u32) -> Result<(), String> {
    let signal = |name: &str| {
        std::process::Command::new("kill")
            .args([name, &pid.to_string()])
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

/// Starts `exe web ...` on its own, outliving this command, with its output in `log`, and waits
/// up to 20 seconds for it to answer at `bind`. On failure, says what the log ended with.
pub fn start(
    exe: &Path,
    global: &[String],
    settings: &Settings,
    log: &Path,
) -> Result<u32, String> {
    use std::os::unix::process::CommandExt;
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("can't make {}: {e}", parent.display()))?;
    }
    let out =
        std::fs::File::create(log).map_err(|e| format!("can't write {}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new(exe)
        .args(global)
        .args(settings.web_args())
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(err)
        // Its own process group, so closing this terminal doesn't end it.
        .process_group(0)
        .spawn()
        .map_err(|e| format!("can't start {}: {e}", exe.display()))?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if answering(settings.bind()) {
            return Ok(child.id());
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("the viewer stopped ({status}): {}", tail(log)));
        }
        if Instant::now() > deadline {
            return Err(format!(
                "the viewer didn't answer within 20 seconds: {}",
                tail(log)
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn tail(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().rev().take(5).collect();
    let shown: Vec<&str> = lines.into_iter().rev().collect();
    if shown.is_empty() {
        format!("nothing in {}", log.display())
    } else {
        format!("{} ends: {}", log.display(), shown.join(" / "))
    }
}

/// Opens `url` in `browser`, a command and its arguments, or else the desktop's default browser.
pub fn open(url: &str, browser: Option<&str>) -> Result<(), String> {
    let default = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut words = browser.unwrap_or(default).split_whitespace();
    let opener = words.next().ok_or("--browser is empty")?;
    std::process::Command::new(opener)
        .args(words)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("can't run {opener}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn given_settings_replace_saved_ones_and_make_the_web_arguments() {
        let saved = Settings {
            summaries: vec!["DSM=/d".into()],
            watchman_summaries: Some("/w".into()),
            ..Settings::default()
        };
        let given = Settings {
            bind: Some("127.0.0.1:7400".into()),
            ..Settings::default()
        };
        let merged = saved.merged(&given);
        assert_eq!(
            merged.web_args(),
            args("web --bind 127.0.0.1:7400 --watchman-summaries /w --summaries DSM=/d")
        );
        assert_eq!(Settings::default().bind(), DEFAULT_BIND);
        assert_eq!(saved.merged(&Settings::default()), saved);
    }

    #[test]
    fn a_viewer_is_restarted_for_another_build_or_other_settings_only() {
        let settings = Settings::default();
        let running = Running {
            pid: 1,
            build: "a".into(),
            settings: settings.clone(),
        };
        assert_eq!(why_restart(&running, "a", &settings), None);
        assert!(why_restart(&running, "b", &settings).is_some());
        let other = Settings {
            projects: Some("/p".into()),
            ..settings
        };
        assert!(why_restart(&running, "a", &other).is_some());
    }

    #[test]
    fn the_viewer_reads_with_the_read_only_pairing_and_its_files_never_take_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("client.json");
        assert_eq!(viewer_pairing(&config), config, "no read-only pairing yet");
        std::fs::write(dir.path().join(VIEWER_PAIRING), "{}").unwrap();
        assert_eq!(viewer_pairing(&config), dir.path().join(VIEWER_PAIRING));
        for name in [SETTINGS_FILE, RUN_FILE, LOG_FILE] {
            assert!(name != VIEWER_PAIRING && name != "client.json" && name != "web-key");
        }
    }

    #[test]
    fn a_viewer_is_found_by_its_command_line_at_its_address() {
        let bind = DEFAULT_BIND;
        assert!(is_viewer(&args("/x/rally web --summaries DSM=/d"), bind));
        assert!(is_viewer(
            &args("rally --config c web --bind 127.0.0.1:7300"),
            bind
        ));
        assert!(is_viewer(&args("rally web --bind=127.0.0.1:7300"), bind));
        assert!(!is_viewer(&args("rally web --bind 127.0.0.1:7400"), bind));
        assert!(!is_viewer(&args("rally web --login"), bind));
        assert!(!is_viewer(&args("rally show"), bind));
        assert!(!is_viewer(&args("vim web"), bind));
        assert!(!is_viewer(&[], bind));
    }
}
