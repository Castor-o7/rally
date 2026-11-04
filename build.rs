//! On macOS, embed `macos/Info.plist` in the binary, so that macOS can ask for, and remember, Local
//! Network access for rally itself when it runs in the background, as the backup agent does. A plain
//! command-line program has no identity of its own for that, so without this its LAN connections
//! from launchd fail with "No route to host" and it never appears under Local Network.

fn main() {
    println!("cargo:rerun-if-changed=macos/Info.plist");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
        println!("cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{dir}/macos/Info.plist");
    }
}
