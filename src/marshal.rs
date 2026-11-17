//! Rally's own way to Marshal's daemon (RAL-199): a Myko client of the daemon itself, so nothing
//! waits on a shim subcommand. Upstream Marshal 0.60.2's `marshal-shim` has no `roster`, and
//! Muster's daemon is retired, so `<shim> roster` can no longer give the roster.
//!
//! Reading the roster publishes nothing, so it puts no row on it. The rows are shaped the way the
//! viewer and [`crate::shims`] read a shim's roster: what the session said about itself is
//! `declared`, and what the daemon holds about its connection and wait is `recorded`.
//!
//! Sending ([`send`], RAL-199 part 2) needs a sender on the roster, since the daemon names the
//! sender from the connection's session. So Rally registers its own: kind `service`, project
//! `rally`, a stable id per machine and command (`rally-digest@Murph`), only for as long as the
//! send takes, and deletes it after. It never sends as an agent's session. A `service` row is
//! never shown as a team or counted as one: [`shape`] leaves it out and counts it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use marshal_entities::{
    DeleteSession, DeleteSessionResult, GetAllRoomMembers, GetAllSessionNicknames, GetAllSessions,
    GetAllWakeWatchers, HostInfo, RegisterSession, RoomMember, SendMessage, SendMessageResult,
    Session, SessionId, SessionNickname, WakeWatcher,
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

/// The `kind` Rally's own sessions declare, so no reader takes one for a team.
pub const SERVICE_KIND: &str = "service";

/// How long a send may take, connecting, registering and the daemon's answer included.
pub const SEND_LIMIT: Duration = Duration::from_secs(15);

/// What came of a [`send`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sent {
    /// The daemon stored it: its message id and the session it went to.
    Delivered {
        message_id: String,
        to_session_id: String,
    },
    /// Nothing was sent, since the recipient isn't on this daemon's roster or the daemon couldn't
    /// be reached. Another source may be tried.
    NotHere(String),
    /// It failed after it may have been delivered, or the daemon refused it: try nothing else.
    Failed(String),
}

/// The id Rally's own session has for `command` on this machine, as `rally-digest@Murph`: stable,
/// so each run replaces its own row rather than adding one.
pub fn service_id(command: &str) -> String {
    format!("rally-{command}@{}", machine())
}

fn machine() -> String {
    rustix::system::uname()
        .nodename()
        .to_string_lossy()
        .into_owned()
}

/// Sends `body` to `to`, a session id or a nickname on the roster, from Rally's own `command`
/// session, through the daemon at [`daemon_address`].
pub fn send(command: &str, to: &str, body: &str) -> Sent {
    let state_dir = std::env::var_os("RALLY_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state/rally"))
        })
        .unwrap_or_else(|| PathBuf::from("/"));
    send_at(
        &daemon_address(),
        &service_id(command),
        &state_dir.to_string_lossy(),
        to,
        body,
        SEND_LIMIT,
    )
}

/// [`send`] to the daemon at `address`, as the session `id` started in `cwd`, within `limit`.
pub fn send_at(address: &str, id: &str, cwd: &str, to: &str, body: &str, limit: Duration) -> Sent {
    if let Err(why) = reachable(address, limit) {
        return Sent::NotHere(why);
    }
    marshal_entities::link();
    let until = Instant::now() + limit;
    let client = MykoClient::new_with_auto_reconnect(false);
    let sessions = client.watch_query_state::<GetAllSessions>(GetAllSessions {});
    let nicknames = client.watch_query_state::<GetAllSessionNicknames>(GetAllSessionNicknames {});
    let watchers = client.watch_query_state::<GetAllWakeWatchers>(GetAllWakeWatchers {});
    client.set_address(Some(address.to_owned()));
    let sent = (|| {
        wait(until, || {
            if let Some(error) = sessions
                .error()
                .get()
                .or_else(|| nicknames.error().get())
                .or_else(|| watchers.error().get())
            {
                return Some(Err(Sent::NotHere(format!(
                    "the daemon at {address} refused a query: {error}"
                ))));
            }
            (sessions.ready().get() && nicknames.ready().get() && watchers.ready().get())
                .then_some(Ok(()))
        })
        .unwrap_or_else(|| {
            Err(Sent::NotHere(format!(
                "no roster from the daemon at {address} within {}s",
                limit.as_secs()
            )))
        })?;
        let to_session_id = match recipient(
            &sessions.items().get(),
            &nicknames.items().get(),
            &watchers.items().get(),
            now_ms(),
            to,
        ) {
            Ok(id) => id,
            Err(why) => return Err(Sent::NotHere(why)),
        };
        let me = SessionId(Arc::from(id));
        let now = now_ms();
        let session = Session {
            id: me.clone(),
            client_id: None,
            agent_id: None,
            pid: std::process::id(),
            cwd: cwd.to_owned(),
            git_branch: None,
            current_task: Some(format!(
                "Rally's {} sending",
                id.trim_start_matches("rally-")
            )),
            session_name: None,
            activity: Some("busy".to_owned()),
            kind: Some(SERVICE_KIND.to_owned()),
            connected_at: now,
            last_activity_at: Some(now),
            last_tool: None,
            last_tool_at: None,
            operator: std::env::var("USER").ok(),
            host: Some(HostInfo {
                name: machine(),
                os: std::env::consts::OS.to_owned(),
                arch: std::env::consts::ARCH.to_owned(),
            }),
            project: Some("rally".to_owned()),
            channels_enabled: None,
            acks_pushes: None,
        };
        let registered = client.send_command::<RegisterSession, ()>(&RegisterSession { session });
        // Registered once the daemon answers and the row is bound to this connection, since the
        // daemon names a sender by that binding.
        wait(until, || match registered.get() {
            Some(Err(error)) => Some(Err(Sent::NotHere(format!(
                "the daemon refused Rally's session: {error}"
            )))),
            Some(Ok(())) => sessions
                .items()
                .get()
                .iter()
                .any(|s| s.id == me && s.client_id.is_some())
                .then_some(Ok(())),
            None => None,
        })
        .unwrap_or_else(|| {
            Err(Sent::NotHere(format!(
                "the daemon didn't register Rally's session within {}s",
                limit.as_secs()
            )))
        })?;
        let result = client.send_command::<SendMessage, SendMessageResult>(&SendMessage {
            to_session_id: to_session_id.clone(),
            response_required: false,
            body: body.to_owned(),
            as_session: None,
        });
        // From here it may have been delivered, so nothing else is tried.
        let sent = wait(until, || match result.get() {
            Some(Ok(result)) => Some(Sent::Delivered {
                message_id: result.message_id.0.to_string(),
                to_session_id: result.to_session_id.0.to_string(),
            }),
            Some(Err(error)) => Some(Sent::Failed(format!(
                "the daemon refused the message: {error}"
            ))),
            None => None,
        })
        .unwrap_or_else(|| {
            Sent::Failed(format!(
                "no answer to the message within {}s: it may have been delivered",
                limit.as_secs()
            ))
        });
        let deleted =
            client.send_command::<DeleteSession, DeleteSessionResult>(&DeleteSession { id: me });
        let _ = wait(Instant::now() + Duration::from_secs(2), || {
            deleted.get().map(|_| ())
        });
        Ok(sent)
    })();
    client.close();
    match sent {
        Ok(sent) | Err(sent) => sent,
    }
}

/// Polls `ready` until it gives a value or `until` passes.
fn wait<T>(until: Instant, mut ready: impl FnMut() -> Option<T>) -> Option<T> {
    loop {
        if let Some(value) = ready() {
            return Some(value);
        }
        if Instant::now() >= until {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The session `to` names: an exact session id, or the one non-service session whose nickname it
/// is. A name two sessions hold is refused rather than guessed.
pub fn recipient(
    sessions: &[Arc<Session>],
    nicknames: &[Arc<SessionNickname>],
    watchers: &[Arc<WakeWatcher>],
    now_ms: i64,
    to: &str,
) -> Result<SessionId, String> {
    let to = to.trim();
    let teams: Vec<&Arc<Session>> = sessions
        .iter()
        .filter(|s| s.kind.as_deref() != Some(SERVICE_KIND))
        .collect();
    if let Some(session) = teams.iter().find(|s| s.id.0.as_ref() == to) {
        return Ok(session.id.clone());
    }
    let named: Vec<&&Arc<Session>> = teams
        .iter()
        .filter(|s| {
            let id = s.id.0.as_ref();
            nicknames
                .iter()
                .find(|n| n.id.0.as_ref() == id)
                .map_or_else(|| marshal_entities::nickname(id), |n| n.nickname.clone())
                .eq_ignore_ascii_case(to)
        })
        .collect();
    // A nickname reaches a live session only (RAL-202): bound to a shim's connection, or with a
    // wait the daemon holds now. A leftover holding the same name is never the one meant.
    // As `shims::stale` has it: bound with a real pid, or waiting now.
    let live = |s: &Arc<Session>| {
        (s.client_id.is_some() && s.pid != 0)
            || watchers
                .iter()
                .any(|w| w.id.0.as_ref() == s.id.0.as_ref() && w.until > now_ms)
    };
    let stale_only = !named.is_empty() && !named.iter().any(|s| live(s));
    let named: Vec<&&Arc<Session>> = named.into_iter().filter(|s| live(s)).collect();
    match named.as_slice() {
        [one] => Ok(one.id.clone()),
        [] if stale_only => Err(format!(
            "{to} isn't on Marshal's roster as a live session: only a stale row (no shim bound, no wait) holds the name"
        )),
        [] => Err(format!("{to} isn't on Marshal's roster")),
        _ => Err(format!(
            "{to} names {} sessions on Marshal's roster, so it's ambiguous",
            named.len()
        )),
    }
}

/// Where a designated name is reached on Marshal (RAL-199): the nickname of a live roster row, and
/// the names passed through on the way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub to: String,
    /// From the designated name to `to`, each step a session record of the open rally.
    pub chain: Vec<String>,
    /// The record numbers of those steps.
    pub records: Vec<u64>,
}

/// The live session a designated name (a watchman's, as the operator signed it) now is, found only
/// through the open rally's session records: the name itself if it's on the roster, or else the
/// session recorded as succeeding it, and so on. Routing only: it confers nothing. Never a guess by
/// directory or project; with no such chain to a live row, `Err` says why and nothing is sent.
pub fn route(designated: &str, open_rally: &Value, live: &[String]) -> Result<Route, String> {
    let is_live = |name: &str| live.iter().any(|n| n.eq_ignore_ascii_case(name));
    let mut current = designated.trim().to_owned();
    let mut chain = vec![current.clone()];
    let mut records = Vec::new();
    let records_of = open_rally["sessions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    loop {
        if is_live(&current) {
            return Ok(Route {
                to: current,
                chain,
                records,
            });
        }
        // The latest record naming a successor to `current` that isn't a removal.
        let next = records_of
            .iter()
            .filter(|r| r["remove"] != json!(true))
            .filter(|r| {
                r["succeeds"]
                    .as_str()
                    .is_some_and(|old| old.eq_ignore_ascii_case(&current))
            })
            .max_by_key(|r| r["seq"].as_u64().unwrap_or(0));
        let Some(next) = next else {
            let how = if chain.len() == 1 {
                String::new()
            } else {
                format!(" (followed {})", chain.join(" → "))
            };
            return Err(format!(
                "can't find {designated} on Marshal: {current} isn't on the roster, and no session \
                 of the open rally is recorded as succeeding it{how}"
            ));
        };
        let Some(name) = next["session"].as_str().map(str::to_owned) else {
            return Err(format!(
                "a session record for {current}'s successor names no session"
            ));
        };
        if chain.iter().any(|seen| seen.eq_ignore_ascii_case(&name)) {
            return Err(format!(
                "can't find {designated} on Marshal: the succession records loop ({} → {name})",
                chain.join(" → ")
            ));
        }
        records.push(next["seq"].as_u64().unwrap_or(0));
        chain.push(name.clone());
        current = name;
    }
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
    let services = sessions
        .iter()
        .filter(|s| s.kind.as_deref() == Some(SERVICE_KIND))
        .count();
    let rows: Vec<Value> = sessions
        .iter()
        .filter(|s| s.kind.as_deref() != Some(SERVICE_KIND))
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
    // Rally's own sessions, left out so none is taken for a team, but counted.
    json!({"sessions": rows, "service_sessions": services})
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
    fn a_designated_name_is_routed_only_by_the_open_rally_s_records() {
        let open = json!({"sessions": [
            {"seq": 5, "session": "gilded-canyon", "succeeds": "mythic-mackerel", "remove": false},
            {"seq": 7, "session": "mega-jackdaw", "succeeds": "mellow-weevil", "remove": false},
            {"seq": 9, "session": "late-heron", "succeeds": "mega-jackdaw", "remove": true},
        ]});
        let live = ["mega-jackdaw".to_owned(), "gilded-canyon".to_owned()];
        // On the roster itself: no chain.
        let direct = route("Gilded-Canyon", &open, &live).unwrap();
        assert_eq!(
            (direct.to.as_str(), direct.records.len()),
            ("Gilded-Canyon", 0)
        );
        // Through one record; the removal isn't a successor.
        let routed = route("mellow-weevil", &open, &live).unwrap();
        assert_eq!(routed.to, "mega-jackdaw");
        assert_eq!(routed.chain, ["mellow-weevil", "mega-jackdaw"]);
        assert_eq!(routed.records, [7]);
        // No record: nothing is guessed.
        let lost = route("fiery-snapper", &open, &live).unwrap_err();
        assert!(
            lost.contains("can't find fiery-snapper on Marshal"),
            "{lost}"
        );
        // A successor that isn't live, with no further record, is no route either.
        let gone = route("mellow-weevil", &open, &["gilded-canyon".to_owned()]).unwrap_err();
        assert!(
            gone.contains("followed mellow-weevil → mega-jackdaw"),
            "{gone}"
        );
        // A loop ends.
        let looped = json!({"sessions": [
            {"seq": 1, "session": "b", "succeeds": "a"},
            {"seq": 2, "session": "a", "succeeds": "b"},
        ]});
        assert!(route("a", &looped, &[]).unwrap_err().contains("loop"));
    }

    #[test]
    fn a_recipient_is_an_id_or_one_live_team_s_nickname_never_a_service_or_a_ghost() {
        let bound = |id: &str| {
            let mut s = session(id);
            s.client_id = Some(myko::entities::client::ClientId(Arc::from(id)));
            Arc::new(s)
        };
        let mut service = session("rally-digest@Murph");
        service.kind = Some(SERVICE_KIND.into());
        service.client_id = Some(myko::entities::client::ClientId(Arc::from("svc")));
        let nick = |id: &str, name: &str| -> Arc<SessionNickname> {
            Arc::new(serde_json::from_value(json!({"id": id, "nickname": name})).unwrap())
        };
        let waiter: Arc<WakeWatcher> =
            Arc::new(serde_json::from_value(json!({"id": "hook-only", "until": 2_000})).unwrap());
        let sessions = [
            bound("s-1"),
            bound("s-2"),
            bound("s-3"),
            Arc::new(service),
            // Hook-only leftovers: no shim bound, one sharing a live team's name.
            Arc::new(session("ghost-1")),
            Arc::new(session("ghost-2")),
            // Unbound, but a hook holds a wait now: live.
            Arc::new(session("hook-only")),
        ];
        let names = [
            nick("s-1", "mega-jackdaw"),
            nick("s-2", "twin-name"),
            nick("s-3", "twin-name"),
            nick("rally-digest@Murph", "rally-digest"),
            nick("ghost-1", "mega-jackdaw"),
            nick("ghost-2", "only-a-ghost"),
            nick("hook-only", "hook-waiter"),
        ];
        let to = |name: &str| {
            recipient(
                &sessions,
                &names,
                std::slice::from_ref(&waiter),
                1_000,
                name,
            )
        };
        assert_eq!(to("s-2").unwrap().0.as_ref(), "s-2");
        // A ghost sharing a live team's name never makes it ambiguous.
        assert_eq!(to("Mega-Jackdaw").unwrap().0.as_ref(), "s-1");
        assert!(to("twin-name").unwrap_err().contains("ambiguous"));
        assert!(to("only-a-ghost").unwrap_err().contains("only a stale row"));
        assert_eq!(to("hook-waiter").unwrap().0.as_ref(), "hook-only");
        // Bound but declaring pid 0 is stale, as the viewer has it, so not a recipient by name.
        let mut zero = (*bound("z-1")).clone();
        zero.pid = 0;
        let zero = [Arc::new(zero)];
        let zero_name = [nick("z-1", "pid-zero")];
        assert!(
            recipient(&zero, &zero_name, &[], 1_000, "pid-zero")
                .unwrap_err()
                .contains("only a stale row")
        );
        assert!(to("rally-digest").unwrap_err().contains("isn't on"));
        assert!(to("rally-digest@Murph").is_err());
        // And the roster Rally shows leaves the service row out, counting it.
        let shaped = shape(&sessions, &names, &[], &[], 0);
        assert_eq!(shaped["sessions"].as_array().unwrap().len(), 6);
        assert_eq!(shaped["service_sessions"], 1);
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
