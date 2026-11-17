//! Rally's own way to Marshal's daemon (RAL-199): a Myko client of the daemon itself, so nothing
//! waits on a shim subcommand. Upstream Marshal 0.60.2's `marshal-shim` has no `roster`, and
//! Muster's daemon is retired, so `<shim> roster` can no longer give the roster.
//!
//! It only reads: it watches the daemon's sessions, nicknames, room memberships and wake watchers,
//! and never publishes a session of its own, so reading the roster puts no row on it. The rows are
//! shaped the way the viewer and [`crate::shims`] read a shim's roster: what the session said about
//! itself is `declared`, and what the daemon holds about its connection and wait is `recorded`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use marshal_entities::{
    GetAllRoomMembers, GetAllSessionNicknames, GetAllSessions, GetAllWakeWatchers, RoomMember,
    Session, SessionNickname, WakeWatcher,
};
use myko::client::MykoClient;
use myko::prelude::Gettable as _;
use serde_json::{Value, json};

/// The name `RALLY_ROSTER_FROM` gives this source: not a command, but Rally reading Marshal's daemon.
pub const SOURCE: &str = "marshal";

/// Where the daemon is when nothing says otherwise, as `marshal-shim` has it.
pub const DEFAULT_ADDRESS: &str = "ws://localhost:6155";

/// How long a roster read may take before it's given up on, connecting included.
pub const ROSTER_LIMIT: Duration = Duration::from_secs(10);

/// Rally's own override of the daemon's address, which wins over everything `marshal-shim` reads.
/// The tests set it to a port nothing listens on, so no test ever reaches a live daemon.
pub const OVERRIDE_ENV: &str = "RALLY_MARSHAL_ADDRESS";

/// The daemon's address: [`OVERRIDE_ENV`] when it's set, and otherwise found the way
/// `marshal-shim` finds it, first match wins: the first line of `marshal/daemon-address` under
/// `$XDG_CONFIG_HOME` or `~/.config`, then `MARSHAL_DAEMON_ADDRESS`, then `MYKO_ADDRESS`, then
/// [`DEFAULT_ADDRESS`]. The file comes before the variables, as it does for the shim, so Rally and
/// the sessions it reads about agree on the daemon.
pub fn daemon_address() -> String {
    if let Some(address) = std::env::var(OVERRIDE_ENV)
        .ok()
        .filter(|address| !address.trim().is_empty())
    {
        return address.trim().to_owned();
    }
    address_from(
        address_file_candidates()
            .into_iter()
            .find_map(|path| std::fs::read_to_string(path).ok())
            .as_deref(),
        std::env::var("MARSHAL_DAEMON_ADDRESS").ok().as_deref(),
        std::env::var("MYKO_ADDRESS").ok().as_deref(),
    )
}

/// [`daemon_address`] with its inputs given.
pub fn address_from(file: Option<&str>, env: Option<&str>, legacy: Option<&str>) -> String {
    let file = file.and_then(|text| text.lines().next()).map(str::trim);
    [file, env.map(str::trim), legacy.map(str::trim)]
        .into_iter()
        .flatten()
        .find(|address| !address.is_empty())
        .unwrap_or(DEFAULT_ADDRESS)
        .to_owned()
}

fn address_file_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        out.push(PathBuf::from(xdg).join("marshal").join("daemon-address"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        out.push(PathBuf::from(home).join(".config/marshal/daemon-address"));
    }
    out
}

/// Marshal's roster now, as `{"sessions": [...]}`, read from the daemon at [`daemon_address`]
/// within [`ROSTER_LIMIT`].
pub fn roster() -> Result<Value, String> {
    roster_at(&daemon_address(), ROSTER_LIMIT)
}

/// The roster of the daemon at `address`, within `limit`. `Err` says why there's none: the daemon
/// couldn't be reached, refused a query, or didn't answer every query in time.
pub fn roster_at(address: &str, limit: Duration) -> Result<Value, String> {
    // A refused connection is said at once: the Myko client would retry until the limit.
    reachable(address, limit)?;
    marshal_entities::link();
    let client = MykoClient::new_with_auto_reconnect(false);
    let sessions = client.watch_query_state::<GetAllSessions>(GetAllSessions {});
    let nicknames = client.watch_query_state::<GetAllSessionNicknames>(GetAllSessionNicknames {});
    let members = client.watch_query_state::<GetAllRoomMembers>(GetAllRoomMembers {});
    let watchers = client.watch_query_state::<GetAllWakeWatchers>(GetAllWakeWatchers {});
    client.set_address(Some(address.to_owned()));
    let until = Instant::now() + limit;
    let result = loop {
        let errors: Vec<String> = [
            sessions.error().get(),
            nicknames.error().get(),
            members.error().get(),
            watchers.error().get(),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !errors.is_empty() {
            break Err(format!(
                "the daemon at {address} refused a query: {}",
                errors.join("; ")
            ));
        }
        let ready = sessions.ready().get()
            && nicknames.ready().get()
            && members.ready().get()
            && watchers.ready().get();
        if ready {
            break Ok(shape(
                &sessions.items().get(),
                &nicknames.items().get(),
                &members.items().get(),
                &watchers.items().get(),
                now_ms(),
            ));
        }
        if Instant::now() >= until {
            break Err(format!(
                "no roster from the daemon at {address} within {}s: it's unreachable or didn't answer",
                limit.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    client.close();
    result
}

/// `Err` when nothing accepts a TCP connection at `address`'s host and port within `limit`.
fn reachable(address: &str, limit: Duration) -> Result<(), String> {
    use std::net::{TcpStream, ToSocketAddrs as _};
    let rest = address.split_once("://").map_or(address, |(_, rest)| rest);
    let authority = rest.split('/').next().unwrap_or(rest);
    let port_default = if address.starts_with("wss://") {
        443
    } else {
        80
    };
    let with_port = if authority
        .rsplit_once(':')
        .is_some_and(|(_, port)| port.parse::<u16>().is_ok())
    {
        authority.to_owned()
    } else {
        format!("{authority}:{port_default}")
    };
    let unreachable = |why: String| format!("the daemon at {address} couldn't be reached ({why})");
    let addrs: Vec<_> = with_port
        .to_socket_addrs()
        .map_err(|error| unreachable(error.to_string()))?
        .collect();
    let mut last = String::from("no address");
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, limit) {
            Ok(_) => return Ok(()),
            Err(error) => last = error.to_string(),
        }
    }
    Err(unreachable(last))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// The rows of a roster from what the daemon holds, each fact under the block its source belongs
/// in: `declared` for what the session's shim said, `recorded` for the daemon's own record.
pub fn shape(
    sessions: &[Arc<Session>],
    nicknames: &[Arc<SessionNickname>],
    members: &[Arc<RoomMember>],
    watchers: &[Arc<WakeWatcher>],
    now_ms: i64,
) -> Value {
    let rows: Vec<Value> = sessions
        .iter()
        .map(|session| {
            let id = session.id.0.as_ref();
            // The daemon's handle, or the one it would give, as the shim shows before it's assigned.
            let nickname = nicknames
                .iter()
                .find(|n| n.id.0.as_ref() == id)
                .map(|n| n.nickname.clone())
                .unwrap_or_else(|| marshal_entities::nickname(id));
            let rooms: Vec<&str> = members
                .iter()
                .filter(|m| m.session_id == session.id)
                .map(|m| m.room_id.0.as_ref())
                .collect();
            let waiting_until = watchers
                .iter()
                .filter(|w| w.id.0.as_ref() == id && w.until > now_ms)
                .map(|w| w.until)
                .max();
            let wake = marshal_entities::wake_state(session, watchers, now_ms);
            json!({
                "session_id": id,
                "nickname": nickname,
                "wake": wake.as_str(),
                "rooms": rooms,
                "declared": {
                    "pid": session.pid,
                    "cwd": session.cwd,
                    "git_branch": session.git_branch,
                    "current_task": session.current_task,
                    "session_name": session.session_name,
                    "activity": session.activity,
                    "kind": session.kind,
                    "project": session.project,
                    "machine": session.host.as_ref().map(|host| host.name.clone()),
                    "connected_at": session.connected_at,
                    "last_activity_at": session.last_activity_at,
                    "last_tool": session.last_tool,
                    "last_tool_at": session.last_tool_at,
                },
                "recorded": {
                    // A live shim holds a connection the daemon bound the session to; a session
                    // only a hook registered has none.
                    "bound_to_connection": session.client_id.is_some(),
                    "waiting": waiting_until.is_some(),
                    "waiting_until": waiting_until,
                },
            })
        })
        .collect();
    json!({"sessions": rows})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_address_file_wins_then_each_variable_then_the_default() {
        assert_eq!(
            address_from(Some("ws://a:1\nrest"), Some("ws://b:2"), None),
            "ws://a:1"
        );
        assert_eq!(
            address_from(Some("  \n"), Some(" ws://b:2 "), Some("ws://c:3")),
            "ws://b:2"
        );
        assert_eq!(address_from(None, None, Some("ws://c:3")), "ws://c:3");
        assert_eq!(address_from(None, Some(""), None), DEFAULT_ADDRESS);
    }

    fn session(id: &str) -> Session {
        serde_json::from_value(json!({"id": id, "pid": 7, "cwd": "/w", "connectedAt": 1}))
            .expect("a session")
    }

    #[test]
    fn a_row_keeps_declared_and_recorded_apart() {
        let mut bound = session("s-1");
        bound.client_id = Some(myko::entities::client::ClientId(Arc::from("c")));
        bound.current_task = Some("RAL-199".into());
        let watcher: WakeWatcher =
            serde_json::from_value(json!({"id": "s-1", "until": 2_000})).expect("a watcher");
        let nickname: SessionNickname =
            serde_json::from_value(json!({"id": "s-1", "nickname": "upbeat-anole"}))
                .expect("a nickname");
        let shaped = shape(
            &[Arc::new(bound), Arc::new(session("s-2"))],
            &[Arc::new(nickname)],
            &[],
            &[Arc::new(watcher)],
            1_000,
        );
        let rows = shaped["sessions"].as_array().expect("rows");
        assert_eq!(rows[0]["nickname"], "upbeat-anole");
        assert_eq!(rows[0]["wake"], "waiting");
        assert_eq!(rows[0]["declared"]["current_task"], "RAL-199");
        assert_eq!(rows[0]["recorded"]["bound_to_connection"], true);
        assert_eq!(rows[0]["recorded"]["waiting_until"], 2_000);
        // With no assigned handle, the one the daemon would give, as the shim shows it.
        assert_eq!(rows[1]["nickname"], marshal_entities::nickname("s-2"));
        assert_eq!(rows[1]["recorded"]["bound_to_connection"], false);
        assert_eq!(rows[1]["recorded"]["waiting"], false);
    }
}
