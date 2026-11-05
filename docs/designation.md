# Designating the watchman (RAL-51)

Only the operator names the watchman. Rally records a designation where the operator chooses to use it: Rally
isn't required. Until Rally's signed designation is set up, or without Rally, the operator's direct word designates
the watchman, as Muster's `docs/board.md` says. A designation is a ledger record carrying the operator's SSH signature over a
fixed statement:

```
rally-designation v1
watchman: <name, or none to end a designation>
after: <seq>:<chain hash of the record just before it>
```

The operator signs with a Secure Enclave key in [Secretive](https://github.com/maxgoedjen/secretive), which asks for
Touch ID on every use. An agent can ask for a signature, but only the operator can give one. Naming the record just
before it means a signature can't be used anywhere else, in this ledger or another.

## What the service checks

- **When a designation is written:** the service accepts it only if it was started with `--secure` and
  `--operator-key`. It must name the current head, carry the operator key's fingerprint, and verify against that
  key in the `rally-designation` namespace. Every other designation is refused, including one written through `--db`
  on the host.
- **When a designation is reported** (`rally designation`, `GET /v1/designation`): every designation is checked again,
  because the ledger's Postgres can be written past the service. That's its signature against the operator's key,
  and its place: the record it names must be the one just before it in the chain. A designation that fails either
  check is listed as "not in force", with the reason. The watchman is the newest designation that passes. If that
  designation is an `--end`, there's no watchman.

## `serve --secure`

`rally serve --secure --operator-key <file>` refuses to start unless the running binary and the key file, and every
directory above each, are owned by root and writable by no one else. If they aren't, it prints the commands below.
Without `--secure`, designations are refused.

## The operator's steps, by hand

### On a1: make the key

1. Install Secretive (`brew install secretive`, or from its releases page) and open it.
2. In Secretive, create a new key named "Rally designation", with **Require Authentication** on. Use this key only for
   designations, so that a Touch ID prompt from it can mean only one thing.
3. Set up its agent as Secretive describes, so that `SSH_AUTH_SOCK` points at Secretive's socket in the shell where
   you designate.
4. In Secretive, export the public key to a file, for example `~/rally-designation.pub`.

### On Murph: make the binary, the key and the service root-owned

Copy `rally-designation.pub` to Murph, then:

```sh
sudo install -o root -g root -m 0755 ~/.local/bin/rally /usr/local/bin/rally
sudo install -d -o root -g root -m 0755 /etc/rally
sudo install -o root -g root -m 0644 ~/rally-designation.pub /etc/rally/operator.pub
```

The service also has to run from a unit file the agents can't edit. Today it's a systemd **user** unit in
`~/.config/systemd/user/rally.service`, which any process in the `joshd` account can change, for example to run
another program on Rally's port. Move it to a **system** unit, owned by root, that still runs as `joshd`:

```sh
systemctl --user disable --now rally
sudo tee /etc/systemd/system/rally.service >/dev/null <<'UNIT'
[Unit]
Description=Rally ledger service
After=network-online.target

[Service]
User=joshd
ExecStart=/usr/local/bin/rally serve --bind 192.168.1.241:6300 --secure --operator-key /etc/rally/operator.pub
Environment=RALLY_STATE_DIR=/home/joshd/.local/state/rally
Environment="RALLY_DB=postgres://rally@/rally?host=/run/user/1000/marshal-pg"
Environment=RALLY_HEAD_FILE=/home/joshd/.local/state/rally/ledger.head
Environment=RALLY_DB_PASSWORD_FILE=/home/joshd/.local/state/rally/db-password
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
UNIT
sudo systemctl daemon-reload
sudo systemctl enable --now rally
```

Postgres runs as Muster's `marshal-postgres` **user** unit, and a system unit can't wait on a user unit. So the
service retries every 5 seconds until Postgres's socket is there, which needs lingering for `joshd`
(`loginctl enable-linger joshd`, already on if Muster's daemon runs at boot). Its startup line shows the operator
key's fingerprint, which should match Secretive's.

To update Rally later, rebuild, then run the `sudo install` of the binary again and `sudo systemctl restart rally`.

### Designating

On a1, in a shell whose `SSH_AUTH_SOCK` is Secretive's:

```sh
rally designate --watchman grand-photon --key ~/rally-designation.pub
rally designate --end --key ~/rally-designation.pub
rally designation        # who the watchman is, each designation checked again
```

`designate` prints the statement before asking for the signature. Read it before you touch: the Touch ID prompt
itself doesn't show what you're signing. If another record lands in between, the service refuses the designation
("read the head again"). Run it again.

### Binding the watchman to its own credential (RAL-53)

Without a credential, a verification is labelled `watchman` when its declared name matches, and any session can
declare any name. A designation can instead name a credential that only the watchman's session holds. Then a read
counts as the watchman's only when it's made with that credential, which the service sets from the connection,
and a session that declares the watchman's name from any other credential is labelled `other`, with a note.

The credential is only as private as its pairing file. Every session on one macOS account can read every file
that account can, so choose where the watchman runs:

- **(A) Recommended: a separate macOS account on a1** for the watchman's session. Its pairing is in that
  account's home, mode 0600, where the sessions on your own account can't read it.
- **(B) Another machine** for the watchman, with its own pairing.
- **(C) The same account, with a separate pairing file.** This catches mistakes, such as a session on the
  default pairing that declares the watchman's name. It doesn't stop a session that goes looking for the file.

Then, by hand, never through Muster or any session:

1. On Murph: `rally add-machine a1-watchman --address 192.168.1.241:6300 --state-dir ~/.local/state/rally --out
   ~/a1-watchman.pair`. The pair code is written to that private file, never printed.
2. Carry the code to the watchman's account or machine yourself, for example by AirDrop to that account, and
   there run `rally --config ~/.config/rally/client.json pair < a1-watchman.pair`, then delete the file.
3. Start the watchman's session there, so its `rally verify` uses that pairing.
4. Designate it, bound to the credential:

```sh
rally designate --watchman grand-photon --credential a1-watchman --key ~/rally-designation.pub
```

The statement is then version 2, with a `credential:` line, so the signature covers the binding. The service
refuses a credential that isn't paired, or is read-only. `rally designation` shows the bound credential, and
whether it's still paired. `rally revoke a1-watchman` on Murph ends it: the watchman's reads stop counting as the
watchman's until it's paired again and designated again. A designation made before RAL-53, without a credential,
still verifies and still matches by declared name, and `rally verifications` says which way each was matched.

### More than one watchman at once (RAL-63)

A v1 or v2 designation replaces every watchman in force. To have several at once, for example one on Murph and one
on a1, add each with a **scope**, which makes the statement version 3:

```sh
rally designate --watchman frank-crocodile --credential murph-watchman --scope machine:murph --key ~/rally-designation.pub
rally designate --watchman fiery-snapper --credential a1-watchman --scope machine:a1 --key ~/rally-designation.pub
rally designate --end --watchman fiery-snapper --key ~/rally-designation.pub   # ends that one; the other stays
rally designate --end --key ~/rally-designation.pub                            # ends every watchman
```

Each prints its statement before asking for the signature:

```
rally-designation v3
action: add                  (or end)
watchman: <name>
credential: <credential>     (an add only: every v3 watchman is bound, as in RAL-53)
scope: <scope>               (an add only)
after: <seq>:<chain hash of the record just before it>
```

The first line keeps a v3 signature from verifying as v1 or v2, and the other way round. The scope and credential
are signed, so neither can be changed after the operator signs.

- **The set in force** is built from the valid designations in order. A v1 or v2 designation replaces it; a v3 add
  puts its watchman in, replacing that watchman's earlier entry by name; a v3 end takes one watchman out; an `--end`
  with no name empties it. `rally designation` lists every watchman in force under `watchmen`, with its scope,
  credential and whether that's still paired. `watchman` is filled only while exactly one is in force.
- **What's refused.** A v3 add whose credential is already bound to another watchman in force, so a read's label is
  never ambiguous, and a v3 end naming no watchman in force. The service refuses both when written. A v3 add also
  ends any watchman in force with no credential (from a v1 designation), since one matched by declared name would let
  any session read as that watchman beside a bound one; `rally designation` says so on the add's record, under
  `ended`. A v1 or v2 designation replaces every watchman in force, so `rally designate` warns first when more than
  one is. One written
  straight into Postgres is listed as not in force, with the reason, as is anything not signed by the operator's
  key.
- **Scopes** are `all`, `team:<team>` for stories that team holds, or `machine:<credential>` for submissions made
  with that **pairing credential**. A machine scope names a pairing, not a host: renaming a pairing, pairing a
  machine under a new name or `rally revoke` changes what the scope covers, so designate again after any of them.
  A team scope matches the team that holds the story now, so a story that changes hands moves between scopes,
  and what it waited for is judged by its current team, not the one it had when it was read.
- **A scope only decides whose read a merge waits for.** `rally waiting` gives each submission `awaits`, the
  watchmen whose scope covers it. If none does, it names every watchman in force and says any may read it.
  `rally handover` gives each watchman the peer-verified stories in its scope, made while no watchman in force
  covered them, since the story was last covered. A story goes to the first watchman whose scope covers it, even
  when watchmen with other scopes were added in between. A watchman's read outside its scope is still labelled `watchman`, with `watchman_name` saying
  which.
- **With N watchmen, there are N credentials to protect.** Each holds only as well as its own pairing file, by the
  options above. Losing one lets whoever holds it read as that watchman, anywhere, not only inside its scope,
  until it's ended or revoked. The others aren't affected. Give each watchman its own account or machine (A or B),
  and end a watchman as soon as its session is done.

## Runbook: the first real designation of two watchmen (RAL-63)

For the two-watchman rally of 2026-11-05: fiery-snapper on a1 and frank-crocodile on Murph. Every step is the
operator's, by hand. An agent can prepare the commands, but the key, the pair codes and the Touch ID are never
passed through Muster or a session.

### 0. Check the starting point

On a1:

```sh
rally designation
```

Go on only when the report has an `operator_key` (the fingerprint the service was started with) and a `watchmen`
field. On 2026-11-05 at 15:50 EST it had neither. `operator_key` was null, so the service wasn't started with
`--secure --operator-key`, and with no `watchmen` field Murph's build predated RAL-63 (merged as cd27484). So first:

1. Deploy main at cd27484 or later on Murph, by the usual rollout (`docs/service.md`, "Keeping it current").
2. Make the binary, the key and the unit root-owned, and start with `--secure --operator-key`, as in "On Murph"
   above. The service's startup line prints the operator key's fingerprint. Check it against Secretive's.

Once the first designation is written, a build older than RAL-53 or RAL-63 can't read the ledger back, so the
rollback window for the deploy ends there.

### 1. The key, on a1

As in "On a1: make the key" above: a Secretive key named "Rally designation", with Require Authentication on, its
agent in `SSH_AUTH_SOCK`, and its public key exported to `~/rally-designation.pub`. Check:

```sh
ssh-keygen -l -f ~/rally-designation.pub        # the fingerprint, which must match the service's startup line
ssh-add -L | grep -F "$(cut -d' ' -f2 ~/rally-designation.pub)"   # the agent holds it
```

`rally designate --key` takes this **public** key file. `ssh-keygen -Y sign` finds the private half in Secretive's
agent, which asks for Touch ID.

### 2. A pairing for each watchman's own credential, on Murph

Each watchman reads with a credential that no other session holds (RAL-53). On Murph, as the service's account:

```sh
rally add-machine a1-watchman    --address 192.168.1.241:6300 --state-dir ~/.local/state/rally --out ~/a1-watchman.pair
rally add-machine murph-watchman --address 192.168.1.241:6300 --state-dir ~/.local/state/rally --out ~/murph-watchman.pair
```

Neither pair code is printed. Carry each one yourself to where that watchman runs. Best is a separate account (A)
or machine (B), as above. There, pair a config of its own and delete the file:

```sh
rally --config ~/.config/rally/watchman.json pair < a1-watchman.pair && rm a1-watchman.pair
```

Start that watchman's session with `RALLY_CONFIG=~/.config/rally/watchman.json`, so its `rally verify` uses this
pairing and no other session's does. If it shares your account (option C), that catches mistakes only.

### 3. Choose the scopes

A `machine:` scope names a **pairing credential**, the one a submission was made with, not a host. Check which
credentials submissions actually come from:

```sh
rally waiting      # each submission's evidence
rally verifications | grep '"credential"' | sort | uniq -c
```

On 2026-11-05, every session on a1 wrote with the credential `a1`, and Murph's sessions weren't paired at all. Their
records were written for them by a1 sessions, so they came from `a1` too. So:

- **fiery-snapper:** `machine:a1` covers everything a1's sessions submit, and today Murph's as well. `all` covers
  every story.
- **frank-crocodile:** `machine:murph` covers nothing until Murph's sessions are paired under a credential named
  `murph` (`rally add-machine murph ...` on Murph, then `rally pair` in Murph's account). Until then, a team scope
  such as `team:catapp` covers the teams it watches. Either way, its reads outside its scope still count as a
  watchman's.

### 4. Designate, on a1

In a shell whose `SSH_AUTH_SOCK` is Secretive's, one at a time:

```sh
rally designate --watchman fiery-snapper   --credential a1-watchman    --scope machine:a1    --key ~/rally-designation.pub
rally designate --watchman frank-crocodile --credential murph-watchman --scope machine:murph --key ~/rally-designation.pub
```

Each prints what it's about to sign before asking for Touch ID. Read it before you touch. The first should be:

```
rally-designation v3
action: add
watchman: fiery-snapper
credential: a1-watchman
scope: machine:a1
after: <seq>:<64 hex characters>
```

Check that the first line is `v3` and that the action is `add`. Check the watchman, credential and scope against
what you meant. `after` must be the ledger's head just now (`rally check` prints it). If `designate` warns that
watchmen are in force and this would replace them, stop: an add always has `--scope`. If another record lands
in between, the service refuses it ("read the head again"). Run it again and read the new statement.

### 5. Check what's in force

```sh
rally designation
```

`watchmen` should list both, each with its credential, its scope and `credential_paired: true`. `records` should show both
adds as `valid`. `watchman` is null now, since there's more than one.

### 6. A test read

The next real verification from each watchman shows the binding. Or, to check before relying on it: from the
watchman's own session (its own pairing), verify a story that's waiting, then

```sh
rally verifications | head -40
```

Its row should say `reader: watchman`, `watchman_name: fiery-snapper`, `matched_by: credential`. A session on the
ordinary `a1` pairing that declares `--as fiery-snapper` and verifies would be labelled `other`, with a note
naming the credential it came from. Don't write one just to see it, since every record stays in the ledger. The
tests cover that case. `rally waiting` should now give each submission `awaits`, naming the watchman whose scope
covers it.

### Undoing it

`rally designate --end --watchman <name> --key ~/rally-designation.pub` ends one, and the other stays.
`rally designate --end --key ~/rally-designation.pub` ends both. `rally revoke a1-watchman` on Murph stops the
credential working. That watchman's reads stop counting as its own until it's paired and designated again.

## Who verified, and what's waiting

- `rally verifications` lists every verification and send-back, newest first. Each is labelled `watchman` if
  a designated watchman wrote it (by its bound credential, or under v1 its declared name; `watchman_name` says
  which watchman, RAL-63), `peer` if no watchman was designated, `other` if one was
  designated but someone else wrote it, or `unknown` if the service has no operator key to check designations
  with. Names are declared.
- `rally handover` is for a newly designated watchman. It lists every story a peer verified since the designation
  before it, merged or not, with the commit read, who verified it and where it merged. Each stays "awaiting the
  watchman's re-read" until the watchman records its own verification, send-back or correction of that story. A
  correction that annotates the peer's verification is enough, and the peer's record is never changed. With no
  watchman designated, it shows what the next one would get. Nothing stands on a peer's verification alone.
- `rally waiting` lists submissions waiting for a reader, oldest first, with the evidence and the commit of any
  requested read. While no watchman is designated, it suggests a reader for each (RAL-57): a team other than the
  author's, the one whose last read is oldest, so reads spread across teams. A suggestion counts as that team's
  latest, so the next submission goes to another. Following it is optional, and a read counts by the rules
  above. A reader says its team with `rally verify --team` or `rally send-back --team` (declared; refused if it's
  the story's own team). While a watchman is designated, nothing is suggested; without the operator's key, Rally
  can't tell, so it suggests and says so.
- `rally track-record` now also counts what each verifier gave, by declared name, since a record names its
  verifier but not the verifier's team.

## Limits

- **A deleted designation.** If a later designation, such as an `--end`, were deleted from Postgres, an earlier one
  would be in force again. Signatures can't show a removal. The hash chain does: run `rally check --expect` against a
  head saved somewhere the agents can't reach. Rally doesn't record whether that check last passed, so the
  designation report can't say.
- **Several watchmen, several pairings (RAL-63).** Each watchman's label is as strong as its own pairing file, and
  a scope doesn't limit a stolen credential: it reads as that watchman everywhere. The set rule that one credential
  can't be bound to two watchmen in force keeps labels unambiguous, not credentials safe.
- **The `watchman` label is only as strong as the watchman's pairing.** Under a designation bound to a credential
  (RAL-53), the label follows that credential. It protects against every session that can't read the watchman's
  pairing file, which, with the watchman on its own account or machine, is every other session. It doesn't protect
  against anything that can read that file: the watchman's own account, root, or, if the watchman shares your
  account (option C), every session. Under a designation without a credential, the label is a declared-name match,
  which catches mistakes, not impersonation.
- **Readers trust the service's report.** The report is computed by the service on Murph. With the binary, key and
  unit root-owned, an agent can't change what it computes. A client doesn't yet check signatures itself.
- **The host's root.** Anyone with root on Murph, or the operator's password, can change all of this. That's the
  operator.
- **Postgres.** An agent can still write rows straight into the ledger's database, as before. Those writes don't
  make a designation count, because every designation is checked when it's reported. Other records are covered only
  by the chain and `check --expect`, as they always were.
