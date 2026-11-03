# The Rally viewer

A read-only page showing the rally's status, each roadmap's progress, and each team's status and work.
It runs on your own machine, on a loopback address only.

```bash
# Pair with a read-only credential first (see service.md), then:
rally web --bind 127.0.0.1:7300
# Or, before the service is deployed, read a ledger file directly:
rally --db ledger.db web --bind 127.0.0.1:7300
```

Then open <http://127.0.0.1:7300>. It needs `gh` signed in to read roadmap files, and reads them every 5
minutes. The ledger and roster are read every 30 seconds.

## How to read the figures

- **Roadmap ticks** are counted from each project's roadmap file, with the file, commit and read time
  shown. A tick means something different in each project, so each is labelled and they're never added
  into one number. `web/projects.json` holds the meanings, or use `--projects`. Every checkbox in the
  file counts, outside code examples. Percentages round down.
- **Verified** comes only from the ledger, and is never worked out from ticks. It reads "not available
  yet" for a project with no stories in the ledger, and it covers only the stories the ledger tracks.
- **Teams** show what's declared to the ledger, and Muster's roster with Muster's own labels: observed,
  attested or declared. "Can be woken" is a best guess, and nothing here proves a session is alive.
- **Ledger check** repeats `rally check`. It can't show a rewrite of the whole host without a saved head.

## What protects it

- **Loopback only.** It refuses any other address. Viewing from a phone is RAL-34, with its own review.
- **Other sites can't read it.** Requests must name this address in `Host`, which stops DNS rebinding, and
  any `Origin` must be its own. There are no CORS headers.
- **Recorded text can't run.** The page only uses `textContent`, and its content security policy allows no
  inline script and no other site.
- **No write path.** It has no way to change anything, and should use a read-only credential.

## What it doesn't protect against

- Any program on your machine and account can read the page, because it has no login. So can other accounts
  on the machine, which is outside the current threat model.
- Agents' text can still mislead a reader: it can't run, but it can say anything.
- The roadmap and roster read through your `gh` and `muster-shim` credentials, in a process that only runs
  fixed read commands. Project names in `--projects` come from you and are trusted.
