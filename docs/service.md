# The Rally service

One service on one machine is the ledger's single writer (RAL-4, RAL-13). Machines reach it with the
`rally` command.

## Setting it up

On the host:

```bash
export RALLY_STATE_DIR=~/.local/state/rally
rally --db ~/.local/state/rally/ledger.db serve --bind <LAN address>:6300
rally add-machine a1 --address <LAN address>:6300 --out a1.pair
```

The file holds a pair code, which holds a secret, so treat it like a password. It's private to your
account and is never printed, to keep it out of agents' transcripts. Move it to the machine being paired
without showing it, then:

```bash
rally pair < a1.pair && rm a1.pair
export RALLY_NAME=<who you are>
rally show
```

`rally add-machine viewer --read-only --out viewer.pair` makes a credential that can read but not write. `rally revoke <name>`
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
  defaults allow about 17,000 writes a day, so up to about 280 MB. Writes stop when the ledger file reaches
  64 MiB, or while the ledger's disk has under 5 GiB free, and reads carry on. Set the floor with
  `--min-free-bytes`, and check it against the host's real free space before rollout.

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
`rally check` does. The head is kept in `<ledger>.head` for this, and `rally check` compares against it and
prints the current head. Writes stop if that file is missing or isn't on the chain, because a write would
otherwise replace the evidence. A head file behind the ledger is allowed, because a crash leaves it that
way.

To recover, run `rally check`, keeping the damaged files to look at. Then either restore the ledger and its
head file together from a backup, or, if the chain is intact and a head saved elsewhere still matches, run
`rally --db <ledger> adopt-head` on the host. It refuses a broken chain, but it does accept any removal
of the newest records that `check` was reporting, so don't use it to make a warning go away, and don't edit
the head file by hand.

Save that head somewhere other than the host from time to time, then check it with
`rally check --expect SEQ:HASH`. That's the only protection against the case below.

This detects and doesn't prevent. **Someone with write access to the host can rewrite the whole ledger,
its hashes and the head file together, and `rally check` alone will pass.** Only a head saved elsewhere
would show it, and it can't show anything changed after that head was saved, or show which version is
true. Without a saved head, this also goes unseen if someone winds the head file back along with the
records, or removes every record and the head file, which looks like a new ledger.

## Known limits

- `--db` works on a ledger file directly, with no service and no checks on who is writing. It's for tests
  and for the host's own administration.
- The disk floor is checked before each write, so it can't see what other programs write in between, and
  it only stops Rally. It doesn't count backups stored elsewhere, and it can't free space.
- Write limits are per machine, so all of a machine's sessions share them. Watch for 429s in real use.
- One compromised credential could reach the 64 MiB stop in about 6 hours, and several credentials
  proportionally faster, which blocks everyone's writes
  while reads carry on. If that happens, revoke the credential and find its records by credential with
  `rally show`. Those records stay in the ledger, because it's append-only. Raising
  `--max-ledger-bytes` restores writes but costs memory, as below, so check the host's memory first.
- A failed token attempt isn't rate limited, and connections aren't capped or timed out while they're
  still unauthenticated. Tokens are 256 random bits, so guessing isn't practical, but a flood or slow
  connections could still load the service (RAL-35). This is why it stays off the open internet.
- Each request reads and replays the whole ledger in memory, holding the store's lock, and blocks one of
  the service's workers. Measured with a release build on a1, on worst-case 15 KB records through the
  local command rather than the service: about 150 MB and 0.4 s at 64 MiB, and 2.3 GB and 2.9 s at 1 GiB.
  That's why the limit is 64 MiB: the host shares its memory with Muster's daemon, and a read-only
  credential can trigger this too. Don't raise it before RAL-35. Typical records are far smaller, so 64
  MiB holds many more than that test did.
- A damaged `machines.json` logs a line for each request, before any rate limit, until it's fixed.
- If `cert.pem` or `key.pem` goes missing, the service refuses to start rather than making a new
  certificate, which would break every machine's pin. Restore it, or remove both and pair every machine
  again.
- File permissions keep secrets private on Unix only. Windows isn't a supported host.
- Any process on the host's account can read or change the ledger, the key and the token hashes.
- Replacing the certificate means pairing every machine again.
