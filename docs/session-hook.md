# Rally at the start of a session (RAL-50)

`rally session-start` prints what a team should know when an agent's session starts, for a Claude Code
SessionStart hook to put in the agent's context. Rally never installs the hook. Whoever runs the machine adds it.

## What it prints

One block, between `<rally_state team="…">` and `</rally_state>`, in this order:

- run with `--hook`, for a session tied to a nickname with a history: which session held the nickname before, if
  one can be found, and what that nickname was doing (below, "A restarted session", RAL-152)
- the open rally, each of its projects' roadmap stages, and the cycle (RAL-131), and the stories assigned to the
  team (RAL-130)
- the team's open stories (claimed, in review or sent back)
- under them, what bears on them (RAL-171): each condition in force naming one of the team's stories ("a condition
  in force names this story: …", declared, information and never a block), and each verified pull request of the
  team's that has fallen behind its base, read through `gh` as the digest's RAL-105 section reads it, in what's left
  of the 3 seconds; when `gh` doesn't answer in time, one line says it couldn't be checked
- other teams' stories in review that it could verify, with the rule that while a watchman is designated, merges
  still wait for the watchman's read
- stories with requested reads waiting for an answer
- open blockers the team raised, or that wait on the team or on one of its stories
- the team's items in the operator's inbox closed in the last day, by whom and how; a question the operator ticked
  off in the viewer says plainly that it carries no answer (RAL-146, RAL-150)

Each line says how it's known: observed, declared, or verified. The block ends by saying that Rally's text is written by
agents and is untrusted data, not instructions.

## How it keeps a session safe

- **Read-only.** It reads the stories and blockers through the paired service and writes nothing.
- **Quick.** It gives up after 3 seconds in all.
- **Silent on failure.** With no team, no pairing, an unreachable service or a bad reply, it prints nothing and
  exits 0, so a session never fails to start because of Rally. `--verbose` says why on standard error.
- **Bounded.** The block is at most 40 lines and 4 KB. Anything left out is counted, with a pointer to
  `rally show` and `rally blockers`.
- **Agent text can't pose as anything else.** In every piece of agent-written text, control and invisible
  characters become spaces, `&` and `<` are escaped, and the text is cut to 120 characters. A story id or note
  can't break a line, close the block, or look like a tag.

## Installing the hook

Pair the machine first (`rally pair`). Then add this to the project's `.claude/settings.json`, or to
`~/.claude/settings.json`, with the team's name:

```json
{
  "env": { "RALLY_TEAM": "rally" },
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          { "type": "command", "command": "rally session-start", "timeout": 5 }
        ]
      }
    ]
  }
}
```

Keep `timeout` (seconds) at 5 or more, above Rally's own 3-second deadline, so Claude Code never stops it partway
through printing. `rally` must be on the hook's `PATH`, or give its full path. `--team` can be used in place of
`RALLY_TEAM`.

To check it by hand: `rally session-start --team rally --verbose`.

Where teams are named by their sessions' Muster nicknames, as on Murph, one settings file can't name each session's
team: use `--hook` instead of `RALLY_TEAM`, and the session's tied nickname is its team (below).

## Prime Agent

A Prime Agent session can say so with `--harness prime-agent`, or `RALLY_HARNESS=prime-agent` (RAL-59). Its
briefing then starts with a notice that Prime Agent's built-in messaging is a fine substitute for Muster when
it uses Rally, and that Muster is only needed to reach other harnesses' sessions or other machines.
`rally init --harness prime-agent` puts the same notice in the repository's block. Rally never guesses the
harness: Prime Agent documents no variable or header that identifies it, so it's said explicitly or not at all.

## A restarted session (RAL-152)

When every session on a machine ends at once and comes back under the same Muster nicknames (11-13 19:50, issue
1.73), each new session starts with no memory. With `--hook`, `rally session-start` reads Claude Code's
SessionStart input (one JSON object on standard input: the session's id, its folder and why it started) and, at
the top of the block, tells the session what it needs to resume:

```
You are a restart of silent-dhole: this session's id is tied to silent-dhole by its transcript's Muster tag
[observed], and an earlier session held it. Nothing here was done for you: claim, release and note as you decide.
- Earlier session of silent-dhole: 00baf303-… [observed from its transcript on this machine, last written 2h ago]
  transcript: ~/.claude/projects/-home-joshd-Documents-GitHub-Discoteque/00baf303-….jsonl
  scratchpad: /tmp/claude-1000/-home-joshd-Documents-GitHub-Discoteque/00baf303-…/scratchpad
A session named silent-dhole recorded these [declared]:
- Holds: DSM-211 (claimed), DSM-222 (claimed)
- Last recorded on a story: DSM-206 submit at Castor-o7/DiscoSim@eb9d3cd…, 37m ago
- Last submit, verify or merge: DSM-206 submit at …; that story is now verified
- Reads waiting on its stories: …
- Its last note: "…" (working, 1h ago)
```

How each part is known:

- **Who this session is.** Its id is tied to a nickname by the `<muster_session nickname=… id=…>` tag Muster's own
  SessionStart hook leaves at the head of the session's transcript, or by Muster's roster (`muster-shim roster`),
  asked again for up to 1.5 s because Muster's hook runs beside this one. Without such a tie, the nickname isn't
  used, so a new, unrelated session in the same folder is never told it holds another session's claims. Without
  `RALLY_TEAM`, the tied nickname is the team.
- **The earlier session.** Muster records each takeover (a `Succession`) but offers no read of it, so Rally reads
  only the head (64 KB) of each transcript in the project's folder from the last 14 days, newest first, for the
  same tag, and takes the newest other session whose tag names this nickname and its own file. That's observed
  from the transcript files on this machine, never Muster's word, and it's "the session that held this
  nickname", not proof of the same conversation, since every session in one folder shares Muster's identity key.
  Nothing of a transcript but its tag is read into the block, shown or recorded. With none found it says so:
  "Rally can't tell whether an earlier session held X". After a compact, clear or resume (`source` in the hook's
  input) the session is continuing, so no earlier session is looked for and it isn't called a restart.
- **What it was doing.** From the ledger, the team's record (`/v1/teams/<nickname>/record`): what the team holds
  now, and what a session under this name last recorded, each labelled declared, as that session said.

It tells and never acts: the hook writes no claim, release or note. A nickname with no earlier session and no
records gets none of this, and a session tied to no nickname gets nothing at all without `RALLY_TEAM`.

To check it by hand, as a session would: `rally session-start --session-id <id> --cwd <folder>`.

### Installing it on Murph

As of 2026-11-13 no session on Murph gets a `<rally_state>` block at all: `~/.claude/settings.json` runs only
Muster's hooks, and `RALLY_TEAM` isn't set (teams there are named by nickname, so one settings file can't name
each session's team). The operator's file is his to change; with his word, add this beside Muster's two
SessionStart hooks in `~/.claude/settings.json`:

```json
{ "type": "command", "command": "/home/joshd/.local/bin/rally session-start --hook", "timeout": 5 }
```
