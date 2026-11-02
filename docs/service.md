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
- **Read-only is enforced before any route.** A read-only credential can only use GET.
- **Limits.** 16 KiB per request, and for each credential a request rate and a much lower write rate. The
  defaults allow about 17,000 writes a day, so up to about 280 MB. Writes stop when the ledger file reaches
  1 GiB, or while the ledger's disk has under 5 GiB free, and reads carry on. Set the floor with
  `--min-free-bytes`, and check it against the host's real free space before rollout.

## What an accepted write proves

It proves that a process holding that machine's token sent it, and that the change was valid at that point.

It doesn't prove who or which team sent it. Every session on a machine's account can read that machine's
token, so a session can declare any name and any kind, including `verified`. So the check that authors don't
verify their own work catches mistakes, not forgery. Revisit this with MUS-39.

## Known limits

- `--db` works on a ledger file directly, with no service and no checks on who is writing. It's for tests
  and for the host's own administration.
- The disk floor is checked before each write, so it can't see what other programs write in between, and
  it only stops Rally. It doesn't count backups stored elsewhere, and it can't free space.
- Write limits are per machine, so all of a machine's sessions share them. Watch for 429s in real use.
- One compromised credential could reach the 1 GiB stop in about 4 days, which blocks everyone's writes
  while reads carry on. If that happens, revoke the credential, find its records by credential with
  `rally show`, and restart with a higher `--max-ledger-bytes` if there's disk to spare. Records stay
  in the ledger, because it's append-only.
- A failed token attempt isn't rate limited, and connections aren't capped or timed out while they're
  still unauthenticated. Tokens are 256 random bits, so guessing isn't practical, but a flood or slow
  connections could still load the service (RAL-35). This is why it stays off the open internet.
- Each read replays the ledger while holding the store's lock, so heavy reading slows everyone as the
  ledger grows, within the rate limits (RAL-35).
- File permissions keep secrets private on Unix only. Windows isn't a supported host.
- Any process on the host's account can read or change the ledger, the key and the token hashes (RAL-14).
- Replacing the certificate means pairing every machine again.
