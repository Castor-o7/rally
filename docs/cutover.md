# Moving the muster from Muster's daemon to Marshal's (RAL-119)

**An inventory and a runbook. Nothing in it has been done.** The watchman (mellow-weevil) asked for it on
2026-11-08, toward the operator's goal of moving from Muster's daemon to Marshal's with no interruption.
Everything below was read on Murph on 2026-11-08, read-only, with Marshal at 0.58.1, and the gaps were brought
up to date from Marshal's roadmap and the ledger the same day. Where something couldn't be read from Murph,
as on a1, it says so. Every step is the operator's to approve and the lamplighter's to run.

## First, a decision for the operator: Marshal's daemon is open on the LAN today

`marshal-daemon.service` sets `MARSHAL_BIND=0.0.0.0:6155`, and it's listening there now. Marshal has no TLS
(MAR-23) and no per-machine pairing (MAR-24) yet, so anything on the network can connect, register a session
and send mail. Muster's daemon on 6255 is TLS-gated, and refuses to start plain while it has pairings
(MUS-64, `tls.conf`). Until MAR-23 and MAR-24 land, the choices are:

- **Bind Marshal to `127.0.0.1:6155`.** Murph's sessions work, and a1 can't join Marshal until the gate exists.
- **Keep `0.0.0.0` and accept the exposure** on a trusted LAN, deliberately and on record (`rally condition`).

This decides when a1 can move (step 6). The runbook assumes the first until the operator says otherwise.

## What's wired to Muster on Murph

| # | Where | What it does now | On Marshal |
|---|---|---|---|
| 1 | `~/.claude/settings.json` hooks | `SessionStart` runs `muster-shim claude-hook session-start` and `claude-hook wait`, `Stop` runs `claude-hook wait`, `UserPromptSubmit` runs `claude-hook prompt-submit`, and `PostToolUse` runs `claude-hook post-tool-use`, each by the absolute path `~/.cargo/bin/muster-shim` | The same five, from `marshal-shim` (Marshal's `plugins/marshal-shim/hooks/hooks.json`) |
| 2 | `~/.claude.json` MCP server `muster` | `muster-shim` on stdio: the `mcp__muster__*` tools and `muster://` resources | MCP server `marshal`: `marshal-shim`, with `MARSHAL_DAEMON_ADDRESS` (default `ws://localhost:6155`) |
| 3 | `muster-daemon.service` | `0.0.0.0:6255`, TLS on, Postgres db `muster` on the `marshal-pg` socket, `MUSTER_OPERATOR` set | `marshal-daemon.service`, already running: `0.0.0.0:6155`, plain, Postgres db `marshal` |
| 4 | `~/.config/muster/` | `local-token`, and per-session `channels-`, `health-`, `nickname-` and `waiter-` files | `~/.config/marshal/` already has its own per-session files, and no token, since there's no gate |
| 5 | Rally's viewer (`rally web`, `rally view`) | Since RAL-116, reads `marshal-shim roster` first and falls back to `muster-shim roster` (`--roster-from`, `RALLY_ROSTER_FROM`) | Nothing to change: it moves by itself once Marshal has sessions |
| 6 | `rally wake` and `rally digest` (`src/wake.rs`, `src/main.rs`) | Send with `muster-shim tell`, and read `muster-shim roster` for stalls and wakes | **Hard-wired to `muster-shim`.** Needs a Rally story to take the command, as RAL-116 did for the viewer |
| 7 | Rally's SessionStart hook (`rally session-start`, docs/session-hook.md) | Reads the ledger only | Nothing to change |
| 8 | `rally.service` | Rally's own service on `192.168.1.241:6300`. It shares only the `marshal-pg` Postgres with the daemons | Nothing to change |
| 9 | Claude Code plugins | Only the LSP plugins are enabled. Muster is wired by hand (rows 1 and 2), not by a plugin | Either install Marshal's plugin, or wire it by hand the same way. Not both, or every hook runs twice (MAR-36 would catch it) |
| 10 | Binaries | `~/.cargo/bin/muster-shim`, `muster-daemon`, `marshal-shim`, `marshal-daemon` | Keep `muster-*` installed until rollback is no longer needed |

### On a1, to confirm there

a1 is a Mac that pairs with Rally's service (`rally pair`, docs/service.md), and may also be paired with Muster.
Nothing of a1 could be read from Murph, and no a1 session was on Muster's roster when this was written.
Before step 6, someone on a1 lists its hooks, its MCP server, its Muster pairing and its daemon address, the
same as rows 1, 2 and 4.

## What Marshal can't do yet

| Gap | Story | Effect on the move |
|---|---|---|
| TLS | MAR-23 (waits on myko#129) | a1 can't join safely. See the decision above |
| Per-machine pairing | MAR-24 (after MAR-23) | No gate, so the roster has no `attested` or `observed` facts, and Rally's viewer shows declared and recorded only (RAL-116) |
| Nickname kept across a restart | MAR-9, verified, merging 2026-11-08 | Until it's merged, a restarted session gets a new nickname. Rally already records a restart with `rally rally-session add --succeeds` (RAL-108) |
| Message history and unread mail carried over | None yet | Muster's messages are in the `muster` database and Marshal's in `marshal`. Nothing copies them. Unread mail on Muster at the switch has to be read there first |
| Unread mail across a Marshal daemon restart | MAR-14, merged | Postgres keeps it, and MAR-14's restart cells show it. Still checked on Murph in step 1 |
| A restarted session's unread mail follows it to its successor | Muster's MUS-47, being ported (eager-cobra), on top of MAR-9 | Until it lands, a session reads its unread mail before it restarts |
| A message never offered twice | MAR-3, merged | Done |
| Idle overnight wake on Marshal's hooks | MAR-5 | Not yet shown on Murph. Run it before moving every session |
| A daemon write before the session is ready, retried | MAR-43, verified, merging 2026-11-08 | Once merged, nothing to do |
| A health check | MAR-36, verified, merging 2026-11-08 | Once merged, `marshal-shim doctor` confirms each session is wired once |

## The order: a session is never unreachable

The idea: **run both daemons side by side**, and move sessions one at a time. A session is on exactly one
daemon at a time, and anyone can reach it through the other one for as long as it's there. `tell` works
from a terminal on both (`muster-shim tell`, `marshal-shim tell`), so the lamplighter can relay between
the two while they're mixed.

0. **Before anything moves.** MAR-5 is shown (MAR-3 is already merged), and the operator has made the bind
   decision above. Record a hold in Rally (`rally hold`) naming the cutover, so every team sees it.
1. **Prove one session.** Start a fresh test session wired only to Marshal: the MCP server `marshal`, and the
   five hooks from `marshal-shim`, in a project-local `.claude/settings.json` so nothing user-wide changes.
   Check that it appears in `marshal-shim roster`, wakes when idle on `marshal-shim tell`, and that its
   unread mail survives `systemctl --user restart marshal-daemon`.
2. **Rally reads both.** Land a Rally story so `rally wake` and `rally digest` take `--roster-from` and a
   `tell` command like the viewer (row 6). The viewer already prefers Marshal (RAL-116).
3. **Move the watchman and lamplighter first.** They relay for everyone during the move. Each moves by
   restarting its session with Marshal's wiring (project-local, as in step 1), announcing the new nickname
   on Muster first with a `broadcast`, and on Marshal after.
4. **Move each team when it's between stories.** Never mid-review. The watchman records each with
   `rally rally-session add <new nickname> --succeeds <old nickname>`.
5. **Switch the user-wide wiring.** Once every Murph session is on Marshal, replace rows 1 and 2 in
   `~/.claude/settings.json` and `~/.claude.json`, keeping a copy of each file. New sessions start on Marshal.
6. **a1, after the gate.** Only once MAR-23 and MAR-24 are merged, or the operator accepts the open bind.
7. **Retire Muster.** Only after a full day with no traffic on Muster: stop `muster-daemon.service`, but
   don't disable it, and keep its database and binaries for rollback. End the hold in Rally.

## Rollback

At any step, a session goes back by restarting it with Muster's wiring (the copies kept in step 5, or the
project-local file removed). Nothing in steps 0 to 6 stops `muster-daemon` or touches the `muster` database,
so Muster is there to go back to until step 7. After step 7: `systemctl --user start muster-daemon`, restore the
two files, and restart the sessions. Mail sent on Marshal in the meantime stays in the `marshal` database, and
isn't carried back.
