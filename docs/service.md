# The Rally service

One service on one machine is the ledger's single writer (RAL-4, RAL-13). Machines reach it with the
`rally` command.

## Setting it up

On the host:

```bash
export RALLY_STATE_DIR=~/.local/state/rally
rally --db ~/.local/state/rally/ledger.db serve --bind <LAN address>:6300
rally add-machine a1 --address <LAN address>:6300     # prints a pair code
```

A pair code holds a secret, so treat it like a password. On the machine being paired:

```bash
rally pair '<pair code>'
export RALLY_NAME=<who you are>
rally show
```

`rally add-machine viewer --read-only` makes a credential that can read but not write. `rally revoke <name>`
stops a token working straight away. Don't expose the port to the open internet.

## What's enforced

- **TLS with a pinned certificate.** A client only trusts the certificate named in its pair code.
- **A token on every route.** The host stores only hashes. A missing or wrong token gets a 401, including
  on routes that don't exist.
- **No web pages.** Any request carrying an `Origin` header is refused.
- **The service sets each record's credential and time.** A client only supplies a declared name, a kind and
  a change. Only Rally itself may record observed facts.
- **Limits.** 64 KiB per request, and a request rate for each credential.

## What an accepted write proves

It proves that a process holding that machine's token sent it, and that the change was valid at that point.

It doesn't prove who or which team sent it. Every session on a machine's account can read that machine's
token, so a session can declare any name and any kind, including `verified`. So the check that authors don't
verify their own work catches mistakes, not forgery. Revisit this with MUS-39.

## Known limits

- `--db` works on a ledger file directly, with no service and no checks on who is writing. It's for tests
  and for the host's own administration.
- A failed token attempt isn't rate limited. Tokens are 256 random bits, so guessing isn't practical, but
  a flood of bad requests could still load the service.
- Any process on the host's account can read or change the ledger, the key and the token hashes (RAL-14).
- Replacing the certificate means pairing every machine again.
