# Moving the muster from Muster's daemon to Marshal's (RAL-119, refreshed by RAL-197)

**An inventory and a runbook.** The watchman (mellow-weevil) asked for it on 2026-11-08, toward the
operator's goal of moving from Muster's daemon to Marshal's with no interruption. RAL-197 brought it up to
date on **2026-11-17**. Murph was read read-only that day, along with the Marshal repos and the ledger.
Each claim says where it came from. **box** means read on Murph on 11-17. **git** means read in
`~/src/marshal`, fetched 11-17. **ledger** means a Rally record, by seq. **lamplighter's log** means
`lamplighter-role.md` in the shared memory directory: a secondary source, used only where nothing
first-hand records the event. Anything that couldn't be read says so. Every step is the operator's to
approve and the lamplighter's to run.

## Where it stands on 2026-11-17

- **Muster carries every session.** Muster's roster has 5 sessions. Marshal's has one, mauve-tortoise:
  idle, in `~/src/lamplighter`, with an observe-only status from 11-09. User-wide hooks and the MCP server
  are all `muster-shim` (rows 1 and 2). (box)
- **Marshal runs on Murph, on loopback only.** `marshal-daemon.service` runs `~/.cargo/bin/marshal-daemon`,
  version string `0.58.1`, PID 4158649, started 2026-11-09 19:43:28. The binary and shim are dated
  19:43:29 that day. It listens on `127.0.0.1:6155` (Myko) and `127.0.0.1:6156` (the HTTP hook listener).
  The `bind-loopback.conf` drop-in sets `MARSHAL_BIND=127.0.0.1:6155`, and the hook port is on its
  default loopback address. (box)
- **That binary is the fork's `82335c2`**, the merge of #113 (MAR-85), which carries MAR-23, MAR-24, MAR-80,
  MAR-81 and MAR-83. The source is the lamplighter's log ("2026-11-09 19:43 — Marshal 82335c2 deployed").
  The binary doesn't embed its commit, so this can't be read from the box. Two things agree with the log:
  the start time and PID, and the fact that the fork never bumped its version past the upstream release
  it forked from (`ba6c699`, v0.58.1). Fork main is now `06dd942`, 17 commits past `82335c2`. Over
  `crates/` that's one file and 15 lines, which the log calls test-only. (git, lamplighter's log)
- **The trust model changed.** On 2026-11-17 the operator decided that remote agents and VMs join over a
  private mesh VPN, handled outside Marshal: "Marshal shouldn't worry about authentication itself". This
  matches upstream's own posture: coordination stays "on loopback or the private mesh"
  (`docs/browser-auth.md` at `upstream/main`). Upstream has no TLS or pairing code. The quote is relayed
  by the watchman; the ledger doesn't record it yet.
- **No mesh is set up on Murph yet.** The only interface up is `enp5s0` (192.168.1.241, plus a ULA IPv6).
  No `tailscale`, `wg`, `zerotier-cli`, `nebula` or `netbird` is installed, and none runs as a unit. (box)

## The bind: loopback until the mesh is there

When this was first read on 11-08, the unit set `MARSHAL_BIND=0.0.0.0:6155` with no gate. The operator
decided that Marshal binds to `127.0.0.1` "until the gate lands" (condition 1416 in Rally). The drop-in still
does that (box). The operator ended condition 1416 on 2026-11-17 as out of date (record 5356), since the gate
is no longer the plan. Nothing else changed: the drop-in stays until Murph has a mesh, and then Marshal binds
to the mesh interface. Both listeners have to move, `MARSHAL_BIND`
(6155) and `MARSHAL_HOOK_BIND` (6156): a remote agent's hooks register through the hook port
(`docs/agent-launcher.md` at `27b9e09`).

## What's wired to Muster on Murph

Read on Murph, 2026-11-17 (box).

| # | Where | What it does now | On Marshal |
|---|---|---|---|
| 1 | `~/.claude/settings.json` hooks | `SessionStart` runs `muster-shim claude-hook session-start` and `claude-hook wait`, `Stop` runs `claude-hook wait`, `UserPromptSubmit` runs `claude-hook prompt-submit`, and `PostToolUse` runs `claude-hook post-tool-use`, each by the absolute path `~/.cargo/bin/muster-shim` | The same five, from `marshal-shim` (`plugins/marshal-shim/hooks/hooks.json` at `82335c2`) |
| 2 | `~/.claude.json` MCP server `muster` | `~/.cargo/bin/muster-shim` on stdio, no args or env: the `mcp__muster__*` tools and `muster://` resources | MCP server `marshal`: `marshal-shim`, with `MARSHAL_DAEMON_ADDRESS` (default `ws://localhost:6155`) |
| 3 | `muster-daemon.service` | `0.0.0.0:6255` (Myko, TLS by `tls.conf`) and `127.0.0.1:6256`, Postgres db `muster` on the `marshal-pg` socket, `MUSTER_OPERATOR` set. PID 2558205 | `marshal-daemon.service`: `127.0.0.1:6155` and `127.0.0.1:6156`, plain, Postgres db `marshal` on the same socket |
| 4 | `~/.config/muster/` | `local-token`, and per-session `channels-`, `health-`, `nickname-` and `waiter-` files (171 sessions' worth) | `~/.config/marshal/` has its own per-session files (38), a `local-token` (written 11-09 17:55), and `cutover/`, which holds step 3's no-hooks user settings and first prompt. `~/.local/state/marshal` has no pairings |
| 5 | Rally's viewer (`rally web`, `rally view`) | Since RAL-126, reads every roster in `--roster-from` (`RALLY_ROSTER_FROM`, by default `marshal-shim,muster-shim`, `src/shims.rs`) and shows their union, each row naming its source | Nothing to change: sessions show from whichever daemon they're on |
| 6 | `rally wake`, `rally digest`, `rally stalls` and `rally brief` (`src/shims.rs`) | Since RAL-126, read every roster and merge them; since RAL-120, send through `marshal-shim` first, falling back to `muster-shim` only when the first says nothing was sent | Nothing to change |
| 7 | Rally's SessionStart hook (`rally session-start`, docs/session-hook.md) | Reads the ledger only | Nothing to change |
| 8 | `rally.service` | A **system** unit (`/etc/systemd/system/rally.service`, `User=joshd`) on `192.168.1.241:6300`, `--secure`. It shares only the `marshal-pg` Postgres with the daemons | Nothing to change |
| 9 | Claude Code plugins | Only `clangd-lsp` and `rust-analyzer-lsp` are enabled. Muster is wired by hand (rows 1 and 2) | Either install Marshal's plugin, or wire it by hand the same way. Not both, or every hook runs twice (`marshal-shim doctor` catches it, MAR-36) |
| 10 | Binaries | `~/.cargo/bin/muster-shim` and `muster-daemon` (11-08), `marshal-shim` and `marshal-daemon` (11-09, `0.58.1`) | Keep `muster-*` installed until rollback is no longer needed |

### Remote machines (a1, VMs), to confirm there

a1 is a Mac that pairs with Rally's service (`rally pair`, docs/service.md). Nothing of a1 could be read
from Murph. Under the 11-17 decision, a1 and any VM reach Marshal over the mesh, not through Marshal's own
gate. Before step 6, someone on each machine lists its hooks, its MCP server, its daemon address and any
Muster pairing, the same as rows 1, 2 and 4, and confirms it can reach both of Murph's mesh ports.

## Which Marshal: the fork, and upstream

Read 2026-11-17 (git).

- Upstream `ignition-is-go/marshal` main is `3d595ec`. That's tag `v0.60.2` (`c36dd04`) plus 2 commits
  (`git describe`: `v0.60.2-2-g3d595ec`).
- The fork `Castor-o7/marshal` main is `06dd942`. It is **401 commits ahead of upstream main and 41
  behind** (`git rev-list --left-right --count castor/main...upstream/main`). Over `crates/` the two
  differ by 112 files (+24522 −2194).
- Murph runs the fork, at `82335c2` (above). The fork's gate work (MAR-23 TLS, verified and merged as
  `a49ade4`; MAR-24 pairing, `37f1003`) is opt-in and off on Murph: the unit passes no `--tls`, and there
  are no pairings. After the mesh decision, Murph doesn't need it.
- New Marshal work is now aimed at upstream: the launcher (MAR-92 design, upstream PR #179; MAR-93 and
  MAR-94, parts building on `27b9e09` over `3d595ec`). Whether Murph keeps running the fork or moves to
  an upstream build is **an open question for the operator**. It was first raised on 11-06 and isn't
  recorded as answered. The answer decides which Marshal the teams move onto.

## What Marshal can't do yet

Checked 2026-11-17 against the ledger and fork main `06dd942`.

| Gap | Story | Effect on the move |
|---|---|---|
| TLS | MAR-23, verified, merged (`a49ade4`); opt-in and off on Murph | **Superseded by the mesh decision (11-17).** Not needed for the move. Upstream doesn't have it |
| Per-machine pairing | MAR-24, verified, merged (`37f1003`); off on Murph, no pairings | **Superseded by the mesh decision.** Without pairing, the roster has no `attested` facts, and Rally's viewer shows declared and recorded only (RAL-116). That's accepted under the mesh model |
| A mesh on Murph | none in Rally | **Missing.** Nothing remote can reach Marshal until there is one and both listeners bind to it. Setting it up is outside Marshal (operator, 11-17) |
| Which Marshal Murph runs | none in Rally | Open (above) |
| Nickname kept across a restart | MAR-9, verified, merged (`83f5202`) | Done. A move to another daemon is a new session, so the watchman still records it with `rally rally-session add --succeeds` (RAL-108) |
| Message history and unread mail carried over | RAL-122, verified (option A) | Nothing copies mail between the `muster` and `marshal` databases. Each session reads its Muster mail before it moves, and the Marshal team runs [mail-carryover.md](mail-carryover.md)'s check before each move and before step 7 |
| Unread mail across a Marshal daemon restart | MAR-14, verified, merged | Done, and checked on Murph in step 1 (MAR-70) |
| A restarted session's unread mail follows it | MAR-42, verified, merged | Done, within Marshal. Not across daemons: that's RAL-122 |
| A message never offered twice | MAR-3, verified, merged | Done |
| Idle overnight wake on Marshal's hooks | MAR-5, ready, not started (shelved by the operator) | Idle wake was shown in step 1's test session |
| A health check, and a project's own wiring | MAR-36 and MAR-60, verified, merged | Done: `marshal-shim doctor` (in the deployed binary; upstream has none) |
| "A full day with no traffic" at step 7 | MAR-65, verified, merged (`15295bb`) | Defined in Marshal's [muster-no-traffic-check.md](https://github.com/Castor-o7/marshal/blob/main/docs/muster-no-traffic-check.md) |
| "As reliable as Muster" | MAR-73, verified; its soak (MAR-77) set aside by the operator on 11-09 | The operator has said to run both daemons as mutual backups until Marshal proves as reliable (11-09, lamplighter's log), and then that deploying fast came first. The bar is accepted, but no soak has run |
| macOS CI | MAR-52, ready | Matters for a1 at step 6 |

## The order: a session is never unreachable

The idea: **run both daemons side by side**, and move sessions one at a time. A session is on exactly one
daemon at a time, and anyone can reach it through the other one for as long as it's there. `tell` works
from a terminal on both (`muster-shim tell`, `marshal-shim tell`), so the lamplighter can relay between
the two while they're mixed.

0. **Before anything moves.** The operator answers which Marshal Murph runs (above). Record a hold in Rally
   (`rally hold`) naming the cutover, so every team sees it. *Not done.* `rally` has no command that lists
   holds, so whether an old one is still in force couldn't be read.
1. **Prove one session.** *Done 11-09* (MAR-70, verified): the test session elder-caracal was wired only to
   Marshal (project-local), and appeared on Marshal's roster. Its unread mail survived a daemon restart at
   13:56 (lamplighter's log). The kit is Marshal's
   [cutover-test-session.md](https://github.com/Castor-o7/marshal/blob/main/docs/cutover-test-session.md)
   (MAR-59). That was on a build before `82335c2`. If Murph moves to another build, prove it again first.
2. **Rally reads both.** *Done* (RAL-120, RAL-126; rows 5 and 6).
   **Before every move in steps 3 and 4 (RAL-122, option A):** the session reads and acks its Muster inbox,
   then the Marshal team runs the read-only check in [mail-carryover.md](mail-carryover.md). Anything unread
   to a session that has already moved is relayed by hand with `marshal-shim tell <new nickname>`, quoting
   its sender and when it was sent.
3. **Move the watchman and lamplighter first.** *Tried and reversed on 11-09.* The dry run (MAR-72, Marshal's
   `docs/cutover-step3-dry-run.md`) passed. The lamplighter then moved on the operator's word at 15:39, as
   mauve-tortoise (tmux `lamplighter`, cwd `~/src/lamplighter`, Marshal-only by `--setting-sources project`
   and `--strict-mcp-config`). At about 16:35 the operator told the Muster session "you are the lamplighter
   and have always been the lamplighter", so the role went back to Muster. mauve-tortoise is still on
   Marshal's roster, observe-only (lamplighter's log; box). Before trying again, the operator says whether
   the step-3 order stands, and what to do with mauve-tortoise. The wiring in `~/src/lamplighter/.claude/` and
   `.mcp.json` and in `~/.config/marshal/cutover/` is still there (box). Each move announces the new nickname
   on Muster first with a `broadcast`, and on Marshal after.
4. **Move each team when it's between stories.** Never mid-review. The watchman records each with
   `rally rally-session add <new nickname> --succeeds <old nickname>`.
5. **Switch the user-wide wiring.** Once every Murph session is on Marshal, replace rows 1 and 2 in
   `~/.claude/settings.json` and `~/.claude.json`, keeping a copy of each file. New sessions start on Marshal.
6. **Remote machines, over the mesh.** Once Murph is on the mesh and both Marshal listeners bind to its mesh
   address, a1 and any VMs join with `MARSHAL_DAEMON_ADDRESS` set to that address. The 11-08 plan tied a1
   (and MAR-35, macOS testing) to the official swap, waiting on MAR-23/24. It now waits on the mesh. A
   launched remote agent (MAR-93/94) needs the same, and its launcher refuses a loopback address.
7. **Retire Muster.** First the Marshal team runs the mail check a last time, and relays anything left. Then,
   only after a full day with no traffic on Muster, as MAR-65 defines it: stop `muster-daemon.service`,
   but don't disable it, and keep its database and binaries for rollback. End the hold in Rally.

## What's left, in short

1. **The operator decides**: which Marshal Murph runs (fork `82335c2`/`06dd942`, or an upstream build); and whether
   step 3's order stands after 11-09's reversal. (Condition 1416 was ended on 11-17.)
2. **The mesh**, set up outside Marshal, and then both listeners bound to it (step 6). Nothing remote joins
   before that.
3. **A hold in Rally** for the cutover (step 0).
4. **Step 3 again**, then step 4 team by team, with RAL-122's mail check before each move.
5. Steps 5 to 7 as written.

Couldn't be read: anything on a1; which commit the running binary was built from (the lamplighter's log is
the only record); whether any Rally hold is in force.

## Rollback

At any step, a session goes back by restarting it with Muster's wiring (the copies kept in step 5, or the
project-local file removed). Nothing in steps 0 to 6 stops `muster-daemon` or touches the `muster` database,
so Muster is there to go back to until step 7. After step 7: `systemctl --user start muster-daemon`, restore the
two files, and restart the sessions. Mail sent on Marshal in the meantime stays in the `marshal` database, and
isn't carried back.
