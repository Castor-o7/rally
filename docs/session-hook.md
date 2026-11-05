# Rally at the start of a session (RAL-50)

`rally session-start` prints what a team should know when an agent's session starts, for a Claude Code
SessionStart hook to put in the agent's context. Rally never installs the hook. Whoever runs the machine adds it.

## What it prints

One block, between `<rally_state team="…">` and `</rally_state>`:

- the team's open stories (claimed, in review or sent back)
- other teams' stories in review that it could verify, with the rule that while a watchman is designated, merges
  still wait for the watchman's read
- stories with requested reads waiting for an answer
- open blockers the team raised, or that wait on the team or on one of its stories

Each line says how it's known: declared, or verified. The block ends by saying that Rally's text is written by
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
