# Moving the muster from Muster's daemon to Marshal's (RAL-119)

**An inventory and a runbook. Nothing in it has been done.** The watchman (mellow-weevil) asked for it on
2026-11-08, toward the operator's goal of moving from Muster's daemon to Marshal's with no interruption.
Everything below was read on Murph on 2026-11-08, read-only, with Marshal at 0.58.1, and the gaps were brought
up to date from Marshal's roadmap and the ledger the same day. Where something couldn't be read from Murph,
as on a1, it says so. Every step is the operator's to approve and the lamplighter's to run.

## Decided: Marshal's daemon listens on Murph only until it has a gate

When this was first read, `marshal-daemon.service` set `MARSHAL_BIND=0.0.0.0:6155`, so with no TLS (MAR-23) and
no per-machine pairing (MAR-24), anything on the network could connect, register a session and send mail.
Muster's daemon on 6255 is TLS-gated (MUS-64, `tls.conf`). The operator decided on 2026-11-08 that Marshal binds
to `127.0.0.1:6155` until the gate lands (condition 1416 in Rally). A drop-in now sets it, and the daemon was
seen listening on `127.0.0.1:6155` only. So a1 can't join Marshal before the official swap (step 6).

## What's wired to Muster on Murph

| # | Where | What it does now | On Marshal |
|---|---|---|---|
| 1 | `~/.claude/settings.json` hooks | `SessionStart` runs `muster-shim claude-hook session-start` and `claude-hook wait`, `Stop` runs `claude-hook wait`, `UserPromptSubmit` runs `claude-hook prompt-submit`, and `PostToolUse` runs `claude-hook post-tool-use`, each by the absolute path `~/.cargo/bin/muster-shim` | The same five, from `marshal-shim` (Marshal's `plugins/marshal-shim/hooks/hooks.json`) |
| 2 | `~/.claude.json` MCP server `muster` | `muster-shim` on stdio: the `mcp__muster__*` tools and `muster://` resources | MCP server `marshal`: `marshal-shim`, with `MARSHAL_DAEMON_ADDRESS` (default `ws://localhost:6155`) |
| 3 | `muster-daemon.service` | `0.0.0.0:6255`, TLS on, Postgres db `muster` on the `marshal-pg` socket, `MUSTER_OPERATOR` set | `marshal-daemon.service`, already running: `127.0.0.1:6155` (a drop-in, decided above), plain, Postgres db `marshal` |
| 4 | `~/.config/muster/` | `local-token`, and per-session `channels-`, `health-`, `nickname-` and `waiter-` files | `~/.config/marshal/` already has its own per-session files, and no token, since there's no gate |
| 5 | Rally's viewer (`rally web`, `rally view`) | Since RAL-126, reads every roster in `--roster-from` (`RALLY_ROSTER_FROM`, by default `marshal-shim,muster-shim`) and shows their union, each row naming its source | Nothing to change: sessions show from whichever daemon they're on |
| 6 | `rally wake`, `rally digest`, `rally stalls` and `rally brief` (`src/shims.rs`) | Since RAL-126, read every roster and merge them, each row naming its source; since RAL-120, send through `marshal-shim` first, falling back to `muster-shim` only when the first says nothing was sent | Nothing to change: they see sessions on both daemons |
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

Last checked 2026-11-09 against Marshal's fork main at `700a1b2` and the ledger (RAL-127). Each row names
its story's state in Rally and, where it's merged, its merge commit on Castor-o7/marshal.

| Gap | Story | Effect on the move |
|---|---|---|
| TLS | MAR-23, ready, not started: waits on myko#129 | a1 can't join until then. Marshal listens on Murph only (above) |
| Per-machine pairing | MAR-24, ready, not started: after MAR-23 | No gate, so the roster has no `attested` or `observed` facts, and Rally's viewer shows declared and recorded only (RAL-116) |
| Nickname kept across a restart | MAR-9, verified, merged (`83f5202`, #31) | Done: a restarted session keeps its nickname. The watchman still records each move with `rally rally-session add --succeeds` (RAL-108), since a move to another daemon is a new session |
| Message history and unread mail carried over | RAL-122, decided: option A | Nothing copies mail between the `muster` and `marshal` databases. Each session reads its Muster mail before it moves, and the Marshal team runs a check before each move and before step 7 ([mail-carryover.md](mail-carryover.md)) |
| Unread mail across a Marshal daemon restart | MAR-14, verified, merged (`8231de7`, #12) | Postgres keeps it, and MAR-14's restart cells show it. Still checked on Murph in step 1 |
| A restarted session's unread mail follows it to its successor | MAR-42 (Muster's MUS-47), verified, merged (`cd3c87e`, #43) | Done, within Marshal. It doesn't carry mail from Muster: that's RAL-122's check, above |
| A message never offered twice | MAR-3, verified, merged (`cd0c44e`, #10) | Done |
| Idle overnight wake on Marshal's hooks | MAR-5, shelved by the operator (ready in Rally, not started) | Idle wake is shown instead in step 1's test session |
| A daemon write before the session is ready, retried | MAR-43, verified, merged (`6fb0bfa`, #38) | Done |
| A health check | MAR-36, verified, merged (`7c25d42`, #23) | Done: `marshal-shim doctor` confirms each session is wired once |
| The health check reads a project's own wiring | MAR-60, verified, merged (`2271713`, #58) | Done: doctor also checks a project-local `.claude/settings.json` and `.mcp.json`, which is how step 1 and each move are wired |
| What "a full day with no traffic" means at step 7 | MAR-65, in review | Defined in Marshal's [muster-no-traffic-check.md](https://github.com/Castor-o7/marshal/blob/main/docs/muster-no-traffic-check.md), with a read-only query |

## The order: a session is never unreachable

The idea: **run both daemons side by side**, and move sessions one at a time. A session is on exactly one
daemon at a time, and anyone can reach it through the other one for as long as it's there. `tell` works
from a terminal on both (`muster-shim tell`, `marshal-shim tell`), so the lamplighter can relay between
the two while they're mixed.

0. **Before anything moves.** The bind is decided (above) and MAR-3 is merged. MAR-5's overnight run is
   shelved by the operator, so nothing else waits. Record a hold in Rally (`rally hold`) naming the cutover, so every team sees it.
1. **Prove one session.** Start a fresh test session wired only to Marshal: the MCP server `marshal`, and the
   five hooks from `marshal-shim`, in a project-local `.claude/settings.json` so nothing user-wide changes.
   Check that it appears in `marshal-shim roster`, wakes when idle on `marshal-shim tell`, and that its
   unread mail survives `systemctl --user restart marshal-daemon`. Marshal's
   [cutover-test-session.md](https://github.com/Castor-o7/marshal/blob/main/docs/cutover-test-session.md)
   (MAR-59) is the kit for this, and `marshal-shim doctor`, run in the test session's directory, checks its
   project-local wiring (MAR-60).
2. **Rally reads both.** Land a Rally story so `rally wake` and `rally digest` take `--roster-from` and a
   `tell` command like the viewer (row 6). The viewer already prefers Marshal (RAL-116).
   **Before every move in steps 3 and 4 (RAL-122, option A):** the session reads and acks its Muster inbox,
   then the Marshal team runs the read-only check in [mail-carryover.md](mail-carryover.md). Anything unread to a
   session that has already moved is relayed by hand with `marshal-shim tell <new nickname>`, quoting its sender
   and when it was sent.
3. **Move the watchman and lamplighter first.** They relay for everyone during the move. Each moves by
   restarting its session with Marshal's wiring (project-local, as in step 1), announcing the new nickname
   on Muster first with a `broadcast`, and on Marshal after.
4. **Move each team when it's between stories.** Never mid-review. The watchman records each with
   `rally rally-session add <new nickname> --succeeds <old nickname>`.
5. **Switch the user-wide wiring.** Once every Murph session is on Marshal, replace rows 1 and 2 in
   `~/.claude/settings.json` and `~/.claude.json`, keeping a copy of each file. New sessions start on Marshal.
6. **a1, at the official swap.** The operator decided a1 and macOS testing, MAR-35 included, happen at the
   official swap to Marshal, once MAR-23 and MAR-24 let a1 reach the daemon.
7. **Retire Muster.** First the Marshal team runs the mail check a last time, and relays anything left. Then,
   only after a full day with no traffic on Muster, as Marshal's
   [muster-no-traffic-check.md](https://github.com/Castor-o7/marshal/blob/main/docs/muster-no-traffic-check.md)
   defines it and checks it (MAR-65): stop `muster-daemon.service`, but
   don't disable it, and keep its database and binaries for rollback. End the hold in Rally.

## Rollback

At any step, a session goes back by restarting it with Muster's wiring (the copies kept in step 5, or the
project-local file removed). Nothing in steps 0 to 6 stops `muster-daemon` or touches the `muster` database,
so Muster is there to go back to until step 7. After step 7: `systemctl --user start muster-daemon`, restore the
two files, and restart the sessions. Mail sent on Marshal in the meantime stays in the `marshal` database, and
isn't carried back.
