# Deploying Rally on Murph

One sitting, once the operator says deploy. Setup details are in [service.md](service.md); this page is the order
of work. Murph is the Linux host that runs Muster's daemon, at 192.168.1.241 on the LAN ([design.md](design.md)).

## Before this page is used

The roadmap's "What a first deployment needs" lists what has to be on `main` first: RAL-15 (backups to a1,
with a real restore), RAL-17 (secrets kept out), RAL-6 (verification limits written down), and RAL-41, which
is to be decided before deployment. RAL-13, RAL-14, RAL-16, RAL-35, RAL-36 and RAL-39 are done. Only the
operator approves the deployment itself, directly, not through a relayed message.

## What the operator does

1. **Reserve Murph's address on the router:** a DHCP reservation of 192.168.1.241 for Murph's network
   interface, so that the address in every pair code stays right.
2. **Install Postgres on Murph** (16 or later, from the distribution's packages), listening on loopback only
   (`listen_addresses = 'localhost'` in `postgresql.conf`), and make Rally's role and database:
   ```sql
   CREATE ROLE rally LOGIN PASSWORD '<a new random password>';
   CREATE DATABASE rally OWNER rally;
   GRANT pg_read_all_settings TO rally;  -- for the disk floor, which reads data_directory
   ```
   with a `pg_hba.conf` line `host rally rally 127.0.0.1/32 scram-sha-256`. Give the password to Rally's
   account on Murph in a file only that account can read (step 2 below), never in chat.
3. **Check headroom on Murph:** at least 5 GiB free under Postgres's data directory, beyond what Muster's
   daemon needs (RAL-36 stops writes below that), and about 0.5 GiB of memory for Rally at its 64 MiB
   ledger limit (RAL-35).
4. If Rally's account should keep running after logout, allow it: `loginctl enable-linger <account>`.
5. **Say deploy**, directly.

## What the Rally team then does

1. **Build** the release binary on Murph from a named commit on `main`, and record that commit.
2. **State.** In Rally's account:
   ```bash
   mkdir -p ~/.local/state/rally && chmod 700 ~/.local/state/rally
   # The password from the operator's step 2, one line:
   install -m 600 /dev/stdin ~/.local/state/rally/db-password
   ```
   with `RALLY_STATE_DIR=~/.local/state/rally`, `RALLY_DB=postgres://rally@127.0.0.1/rally`,
   `RALLY_HEAD_FILE=~/.local/state/rally/ledger.head` and
   `RALLY_DB_PASSWORD_FILE=~/.local/state/rally/db-password`.
3. **Run it** as a systemd user service, so it restarts after a crash or reboot:
   ```ini
   [Service]
   Environment=RALLY_STATE_DIR=%h/.local/state/rally RALLY_DB=postgres://rally@127.0.0.1/rally
   Environment=RALLY_HEAD_FILE=%h/.local/state/rally/ledger.head RALLY_DB_PASSWORD_FILE=%h/.local/state/rally/db-password
   ExecStart=%h/.local/bin/rally serve --bind 192.168.1.241:6300
   Restart=on-failure
   [Install]
   WantedBy=default.target
   ```
   It uses the default limits (`docs/service.md`, "What's enforced"). It prints its certificate's
   fingerprint when it starts: record that too.
4. **Pair:** `rally add-machine a1 --address 192.168.1.241:6300 --out a1.pair`, and `rally add-machine viewer
   --read-only …` for a1's viewer. Each pair code holds a secret: move it to a1 without showing it (over SSH if
   the operator has it set up between the two, or by the operator's hand), then on a1 `rally pair < a1.pair &&
   rm a1.pair`, and `rally --config ~/.config/rally/viewer.json pair < viewer.pair && rm viewer.pair`.

## The first check

1. On Murph: `rally check`, which reads Postgres directly. It should be `ok`, at record 0.
2. On a1, through the service: `rally check`, the same answer, with the fingerprint matching the one recorded.
3. One real record, from a1: Rally's own team state, `RALLY_NAME=<session> rally team-state rally working
   --story <the story in hand>`.
4. On a1: `rally check` again, and save its head somewhere other than Murph, for `rally check --expect` later
   (RAL-14). Until RAL-15's backups run, that saved head is the only copy of the chain's position off Murph.
5. On a1: the viewer with the read-only pairing, `rally --config ~/.config/rally/viewer.json web`, showing the
   ledger as checked and read-only, with Rally's own team state.

Then the trial starts as the roadmap describes: the watchman's table and Rally side by side for Rally's own
stories, with Muster's `docs/board.md` still in force.

## Undoing it

Stop and disable the user service. The ledger stays in Postgres and the head file stays in the state directory,
so it can be restarted or backed up later. Nothing in Muster changes either way, since Muster never depends on
Rally.
