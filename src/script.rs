//! Writing a script that's run straight after, without "Text file busy" (RAL-189).
//!
//! A test that writes a stand-in program (a fake `gh`, `levi` or `coredumpctl`) with
//! `std::fs::write` and then runs it can fail with ETXTBSY ("Text file busy", os error 26). The
//! file is open for writing in the test process for a moment, and if another thread of the same
//! process forks to start a program in that moment, the child holds a copy of that descriptor
//! until it execs. Running a file that any process holds open for writing is refused. The
//! descriptor is close-on-exec, which narrows the window but can't shut it, since the copy lives
//! between the fork and the exec. On 2026-11-14 hosted CI's `levi::tests::
//! output_held_open_after_levi_exits_doesnt_outlast_the_deadline` failed so; on Murph, writing and
//! running a script 300 times beside four threads starting `true` failed 6 and 7 times in two runs.
//!
//! So the script is written by a child process (`sh`, `cat` and `chmod`), and this process never
//! holds the file open at all: no thread's fork can copy a descriptor that isn't there. By the time
//! [`write`] returns, the child has exited and nothing holds the file open for writing. The same
//! 300 runs this way failed none. The text goes to the child on its standard input.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Writes `text` to `path` as an executable (mode 755), through a child process, so that it can be
/// run straight after even while other threads start programs.
pub fn write(path: impl AsRef<Path>, text: impl AsRef<[u8]>) -> std::io::Result<()> {
    let path = path.as_ref();
    let mut child = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()?;
    let sent = child.stdin.take().expect("piped").write_all(text.as_ref());
    let status = child.wait()?;
    // sh's own failure first (RAL-194): when it can't open the file it exits without reading, and
    // a text too long for the pipe then fails to send with "Broken pipe", which says nothing of
    // why. The exit status does.
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "couldn't write the script {}: sh exited with {status}{}",
            path.display(),
            match &sent {
                Err(error) => format!(" (and sending the text failed: {error})"),
                Ok(()) => String::new(),
            }
        )));
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_is_written_whole_and_can_be_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a script, with spaces");
        write(&path, "#!/bin/sh\nprintf '%s' \"$1\"\n").unwrap();
        let out = Command::new(&path).arg("ran").output().unwrap();
        assert_eq!(out.stdout, b"ran");
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        // Written again, it's replaced, not added to.
        write(&path, "#!/bin/sh\necho again\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "#!/bin/sh\necho again\n"
        );
    }

    #[test]
    fn a_script_where_none_can_be_written_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = write(dir.path().join("no-such-dir/x"), "x").unwrap_err();
        assert!(
            err.to_string().contains("couldn't write the script"),
            "{err}"
        );
    }

    /// RAL-194: a text too long for the pipe, to where sh can't write: sh exits without reading,
    /// the send breaks, and the error is still sh's exit status, with the broken pipe beside it.
    #[test]
    fn shs_exit_status_is_reported_ahead_of_a_broken_pipe() {
        let dir = tempfile::tempdir().unwrap();
        let long = vec![b'#'; 4 * 1024 * 1024];
        let err = write(dir.path().join("no-such-dir/x"), &long).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("couldn't write the script"), "{text}");
        assert!(text.contains("sh exited with exit status: "), "{text}");
        assert!(text.contains("sending the text failed"), "{text}");
        // A long text where it can be written is written whole.
        let path = dir.path().join("long");
        write(&path, &long).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), long);
    }

    /// Written and run 200 times while four threads start programs without pause: never busy.
    /// With `std::fs::write` and a chmod in place of [`write`], this failed a few in every
    /// hundred on Murph.
    #[test]
    fn a_script_written_beside_forking_threads_is_never_busy() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let stop = Arc::new(AtomicBool::new(false));
        let forkers: Vec<_> = (0..4)
            .map(|_| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        let _ = Command::new("true").status();
                    }
                })
            })
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let mut busy = 0;
        for i in 0..200 {
            let path = dir.path().join(format!("s{i}"));
            write(&path, "#!/bin/sh\nexit 0\n").unwrap();
            match Command::new(&path).status() {
                Err(e) if e.raw_os_error() == Some(26) => busy += 1,
                Err(e) => panic!("{e}"),
                Ok(status) => assert!(status.success()),
            }
        }
        stop.store(true, Ordering::Relaxed);
        for forker in forkers {
            forker.join().unwrap();
        }
        assert_eq!(busy, 0, "runs refused as Text file busy");
    }
}
