//! On macOS, embed `macos/Info.plist` in the binary, so that macOS can ask for, and remember, Local
//! Network access for rally itself when it runs in the background, as the backup agent does. A plain
//! command-line program has no identity of its own for that, so without this its LAN connections
//! from launchd fail with "No route to host" and it never appears under Local Network.

fn main() {
    println!("cargo:rerun-if-changed=macos/Info.plist");
    commit();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
        println!(
            "cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{dir}/macos/Info.plist"
        );
    }
}

/// Embed the commit this is built from as `RALLY_COMMIT`, or nothing when it isn't a clean git
/// checkout (RAL-100), so `rally view` can tell a rebuild of the same commit from another build:
/// two builds of one commit aren't byte-identical.
fn commit() {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    // Any source change can make a clean checkout dirty, without touching git's own files. A
    // directory here is scanned whole.
    for path in [
        "src",
        "web",
        "macos",
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    for path in ["HEAD", "index"] {
        if let Some(file) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={file}");
        }
    }
    if let Some(head) = git(&["rev-parse", "--symbolic-full-name", "HEAD"])
        && let Some(file) = git(&["rev-parse", "--git-path", &head])
    {
        println!("cargo:rerun-if-changed={file}");
    }
    let clean =
        git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| s.is_empty());
    let commit = git(&["rev-parse", "HEAD"]).filter(|c| clean && c.len() == 40);
    println!(
        "cargo:rustc-env=RALLY_COMMIT={}",
        commit.unwrap_or_default()
    );
}
