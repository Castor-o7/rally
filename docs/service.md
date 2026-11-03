# The Rally service

One service on one machine is the ledger's single writer (RAL-4, RAL-13). Machines reach it with the
`rally` command.

The ledger lives in Postgres on the same host, as myko entities (RAL-39, `docs/design.md`). The service runs myko
inside its own process without myko's listener, so its TLS port is the only way in: no client talks to myko.

## Setting it up

On the host, with a Postgres database for the ledger that only this account can reach:

```bash
export RALLY_STATE_DIR=~/.local/state/rally
export RALLY_DB=postgres://rally@127.0.0.1/rally
export RALLY_HEAD_FILE=~/.local/state/rally/ledger.head
# Only if the role has a password: one line, in a file only you can read (chmod 600).
export RALLY_DB_PASSWORD_FILE=~/.local/state/rally/db-password
rally serve --bind <LAN address>:6300
rally add-machine a1 --address <LAN address>:6300 --out a1.pair
```

`--db` (`RALLY_DB`) is the Postgres URL, and `--head-file` (`RALLY_HEAD_FILE`) is the ledger's head file, kept
outside Postgres (see "Detecting tampering"). Keep any password out of the URL, because the URL can show in
process listings, and Rally's Postgres library doesn't read `~/.pgpass`. Put it in the file named by
`--db-password-file` (`RALLY_DB_PASSWORD_FILE`) instead: Rally refuses one that other accounts can read or that
another account owns. Messages show the address rebuilt without any password, however the URL gave it. Rally makes its table, `rally_events`, on first start. Its role needs to be able to create
it, and to read `data_directory` for the disk floor, which a superuser or a member of `pg_read_all_settings` can.
Postgres must be on the same host, with that directory visible to Rally.

The file holds a pair code, which holds a secret, so treat it like a password. It's private to your
account and is never printed, to keep it out of agents' transcripts. Move it to the machine being paired
without showing it, then:

```bash
rally pair < a1.pair && rm a1.pair
export RALLY_NAME=<who you are>
rally show
```

`rally add-machine viewer --read-only --address <LAN address>:6300 --out viewer.pair` makes a credential that can read but not write. `rally revoke <name>`
stops a token working straight away. Don't expose the port to the open internet.

## What's enforced

- **TLS with a pinned certificate.** A client only trusts the certificate named in its pair code.
- **A token on every route.** The host stores only hashes. A missing or wrong token gets a 401, including
  on routes that don't exist.
- **No web pages.** Any request carrying an `Origin` header is refused.
- **The service sets each record's credential and time.** A client only supplies a declared name, a kind and
  a change. Only Rally itself may record observed facts.
- **Read-only is enforced before any route.** A read-only credential can only use GET and HEAD.
- **Limits.** 16 KiB per request, and for each credential a request rate and a much lower write rate. The
  defaults allow about 17,000 writes a day, so up to about 280 MB. Writes stop when the ledger's table in
  Postgres, with its indexes, reaches 64 MiB, or while the disk under Postgres's data directory has under 5 GiB
  free, and reads carry on. If either can't be checked, writes stop too. Set the floor with `--min-free-bytes`,
  and check it against the host's real free space before rollout.
- **A write is acknowledged only once Postgres has committed it.** If Postgres can't be reached before the
  record is sent, the write fails with a 503 saying nothing was recorded. If the connection fails after the
  commit is sent and before Postgres answers, the reply is a 503 saying the outcome is unknown: it may have
  been recorded, so check with `rally show` or `rally check` before retrying. If it was recorded, the service
  refuses further writes until it's restarted and replays it. Reads of what the service already holds carry on
  either way, and when Postgres is back the service reconnects by itself.
- **Reads don't wait for writes.** They're served from the service's copy without the writer's lock, so a write
  waiting on Postgres doesn't hold them up. `check` reads Postgres, so it does wait.
- **One writer.** The service holds a lock in Postgres for as long as it runs, so a second service or a `--db`
  write is refused rather than forking the chain.
- **Append-only in Postgres too.** The table refuses updates, deletes, `TRUNCATE`, deletion events, a second
  version of a record, and any row whose columns say something other than its event, which myko would load by
  the event alone. That catches mistakes: whoever owns the database can lift it. `rally check` reads by the
  event too, and reports any row whose columns don't match, and a service won't start over one.

## Recorded text is untrusted

Team names, declared names, notes and evidence are written by agents. Every read through `show` and `teams`
carries an `untrusted_text` notice saying they're data, not instructions. Output is JSON, with controls and invisible or direction characters
escaped, and text is stored and shown exactly as written. Whatever displays it has to escape it too. Notes
are limited to 4,000 characters and evidence to 1,000. Names are limited to 64 characters, with no control
characters.

A notice can't stop an agent being persuaded by what it reads. It only makes the boundary visible.

## What an accepted write proves

It proves that a process holding that machine's token sent it, and that the change was valid at that point.

It doesn't prove who or which team sent it. Every session on a machine's account can read that machine's
token, so a session can declare any name and any kind, including `verified`. So the check that authors don't
verify their own work catches mistakes, not forgery. Revisit this with MUS-39.

## Detecting tampering

Each record's hash covers the one before it, so altering, removing or reordering a record means reads that
reach it are refused, and `rally check` says where. A read as of an earlier record still works, because that
history is intact.

Removing the newest records leaves a shorter chain that still holds, so ordinary reads don't notice: only
`rally check` does. The head is kept in the head file for this, outside Postgres, and `rally check` compares
against it and prints the current head. `rally check` reads Postgres directly, not the service's copy, so it also
says if the two differ, or if a record has more than one version in Postgres, which only a change past the append
path can leave. A service won't start over a record with two versions. Writes stop if that file is missing or isn't on the chain, because a write would
otherwise replace the evidence. A head file behind the ledger is allowed, because a crash leaves it that
way.

To recover, run `rally check`, keeping the damaged files to look at. Then either restore the ledger and its
head file together from a backup, or, if the chain is intact and a head saved elsewhere still matches, run
`rally --db <url> --head-file <file> adopt-head` on the host. It refuses a broken chain, but it does accept any removal
of the newest records that `check` was reporting, so don't use it to make a warning go away, and don't edit
the head file by hand. It takes the writer lock, so stop the service first: while it runs, `adopt-head` is
refused, because the service could move the head in between.

Save that head somewhere other than the host from time to time, then check it with
`rally check --expect SEQ:HASH`. That's the only protection against the case below.

This detects and doesn't prevent. **Someone with write access to the host, or the ledger's Postgres role, can
rewrite the whole ledger, its hashes, its history and the head file together, and `rally check` alone will
pass.** Only a head saved elsewhere
would show it, and it can't show anything changed after that head was saved, or show which version is
true. So a head saved elsewhere is the real protection: a head file reset to record 0 with records present
passes `check` and allows writes. Without a saved head, this also goes unseen if someone winds the head file back along with the
records, or removes every record and the head file, which looks like a new ledger.

## Known limits

- `--db` works on the ledger's database directly, with no service and no checks on who is writing. It's for
  tests and for the host's own administration. Its reads (`check`, `show`, `teams`, `web`) are read-only in
  Postgres and work while the service runs. Its writes need the writer lock, so they wait up to 5 seconds and
  are refused while the service runs.
- The disk floor is checked before each write, so it can't see what other programs write in between, and
  it only stops Rally, not Postgres: Postgres's own write-ahead log and other databases can still fill the disk.
  It doesn't count backups stored elsewhere, and it can't free space. The size limit counts Rally's table and
  indexes, not the write-ahead log.
- Postgres must be durable for an acknowledged write to be: keep `fsync` and `synchronous_commit` on, as they are
  by default.
- A Postgres that hangs rather than stops, for example a stopped process, holds a
  write until it recovers, with no deadline of Rally's own: the operating system keeps the connection open, so
  it can be indefinite. Later writes and checks queue behind it; reads carry on. If the connection does fail
  after the commit was sent, the outcome is reported as unknown (RAL-42).
- myko is given the database's address with its password in this process's memory. Rally installs no logger
  for myko, so its own error messages, which can include that address, aren't printed.
- Backups (RAL-15) aren't built. One now means the Postgres database and the head file together.
- Write limits are per machine, so all of a machine's sessions share them. Watch for 429s in real use.
- One compromised credential could reach the 64 MiB stop in about 6 hours, and several credentials
  proportionally faster, which blocks everyone's writes
  while reads carry on. If that happens, revoke the credential and find its records by credential with
  `rally show`. Those records stay in the ledger, because it's append-only. Raising
  `--max-ledger-bytes` restores writes but costs memory, as below, so check the host's memory first.
- A failed token attempt isn't rate limited, and connections aren't capped or timed out while they're
  still unauthenticated. Tokens are 256 random bits, so guessing isn't practical, but a flood or slow
  connections could still load the service (RAL-35). This is why it stays off the open internet.
- The service holds every record in memory, in myko's store, for as long as it runs, about once each: an append
  keeps about 12.4 KB for a 12.2 KB record, measured by the allocator. Each request still replays the whole
  ledger from that copy; a check reads all of Postgres as well.
  Measured with a release build on a1 at the 64 MiB limit: with 64,001 typical 300-character records, 1.1 s to
  replay at start, 0.2 s for a read and 0.3 s for a check through the service, and about 150 to 230 MiB held by
  the service; with 4,751 worst-case records of about 12 KB, 0.6 s, 0.1 s and 0.3 s, and about 130 to 150 MiB.
  At 256 MiB (256,501 typical records), 4.1 s, 0.9 s and 1.7 s, and about 520 to 730 MiB, so it grows linearly.
  That's why the limit stays at 64 MiB: the host shares its memory with Muster's daemon, and a read-only
  credential can trigger the reads too. Don't raise it before RAL-35.
- A damaged `machines.json` logs a line for each request, before any rate limit, until it's fixed.
- If `cert.pem` or `key.pem` goes missing, the service refuses to start rather than making a new
  certificate, which would break every machine's pin. Restore it, or remove both and pair every machine
  again.
- File permissions keep secrets private on Unix only. Windows isn't a supported host.
- Any process on the host's account can read or change the ledger, the key and the token hashes, and so can
  anyone who holds the ledger's Postgres role.
- Replacing the certificate means pairing every machine again.
