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

## Who verified, and what's waiting

- `rally verifications` lists every verification and send-back, newest first. Each is labelled `watchman` if
  the designated watchman's declared name wrote it, `peer` if no watchman was designated, `other` if one was
  designated but someone else wrote it, or `unknown` if the service has no operator key to check designations
  with. Names are declared.
- `rally waiting` lists submissions waiting for a reader, oldest first, with the evidence and the commit of any
  requested read.
- `rally track-record` now also counts what each verifier gave, by declared name, since a record names its
  verifier but not the verifier's team.

## Limits

- **A deleted designation.** If a later designation, such as an `--end`, were deleted from Postgres, an earlier one
  would be in force again. Signatures can't show a removal. The hash chain does: run `rally check --expect` against a
  head saved somewhere the agents can't reach. Rally doesn't record whether that check last passed, so the
  designation report can't say.
- **The `watchman` label is a name match.** A verification is labelled `watchman` when its declared name matches
  the designated watchman's. Any session that declares that name gets the label, and on a1 every session shares
  one credential, so Rally can't tell them apart. It catches mistakes, not impersonation. Binding the label to a
  per-session credential is RAL-53.
- **Readers trust the service's report.** The report is computed by the service on Murph. With the binary, key and
  unit root-owned, an agent can't change what it computes. A client doesn't yet check signatures itself.
- **The host's root.** Anyone with root on Murph, or the operator's password, can change all of this. That's the
  operator.
- **Postgres.** An agent can still write rows straight into the ledger's database, as before. Those writes don't
  make a designation count, because every designation is checked when it's reported. Other records are covered only
  by the chain and `check --expect`, as they always were.
