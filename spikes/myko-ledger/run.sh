#!/usr/bin/env bash
# Runs every RAL-39 proof against a scratch Postgres in a new temp directory, then removes it.
# Usage: run.sh <base-dir-for-temp> [scenario ...]   (default: all scenarios)
# Touches nothing but the Postgres it starts and the server processes it starts.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="$HERE/target/release/myko-ledger-spike"
BASE="$1"; shift
SCENARIOS="${*:-t1 t1b t2 t3 t4 t5 t6 t7}"
PG_BIN=/opt/homebrew/bin
PG_PORT=54391
MYKO_PORT=57391
ADDR="127.0.0.1:$MYKO_PORT"
SPIKE_LOG="${SPIKE_LOG:-error}"; export SPIKE_LOG

for port in "$PG_PORT" "$MYKO_PORT"; do
  if lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "port $port is in use; stopping"; exit 2
  fi
done

WORK="$(mktemp -d "$BASE/ral39.XXXXXX")"
PGDATA="$WORK/pg"
SERVER_PID=""
cleanup() {
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  "$PG_BIN/pg_ctl" -D "$PGDATA" -m immediate -w stop >/dev/null 2>&1
  rm -rf "$WORK"
  echo "cleanup: scratch dir exists? $([ -e "$WORK" ] && echo yes || echo no);" \
    "listening on $PG_PORT? $(lsof -nP -iTCP:$PG_PORT -sTCP:LISTEN >/dev/null 2>&1 && echo yes || echo no);" \
    "on $MYKO_PORT? $(lsof -nP -iTCP:$MYKO_PORT -sTCP:LISTEN >/dev/null 2>&1 && echo yes || echo no)"
}
trap cleanup EXIT

"$PG_BIN/initdb" -D "$PGDATA" -U spike -A trust -E UTF8 >/dev/null || exit 2
cat >>"$PGDATA/postgresql.conf" <<EOF
listen_addresses = '127.0.0.1'
port = $PG_PORT
unix_socket_directories = ''
EOF
pg_start() { "$PG_BIN/pg_ctl" -D "$PGDATA" -l "$WORK/pg.log" -w start >/dev/null || exit 2; }
pg_stop() { "$PG_BIN/pg_ctl" -D "$PGDATA" -m fast -w stop >/dev/null; }
pg_start
export MYKO_POSTGRES_URL="postgres://spike@127.0.0.1:$PG_PORT/postgres"

sql() { "$PG_BIN/psql" -h 127.0.0.1 -p "$PG_PORT" -U spike -d postgres -X -c "$1"; }
sqlv() { "$PG_BIN/psql" -h 127.0.0.1 -p "$PG_PORT" -U spike -d postgres -X -At -c "$1"; }
rows() {
  sql "select id, item_id, change_type, tx is not null as has_tx, left(event->'item'->>'text', 28) as text,
       left(event->'item'->>'hash', 10) as hash, left(source_id, 8) as source from $1 where item_type = 'LedgerRecord' order by id"
}
use_table() { export MYKO_POSTGRES_TABLE="$1" MYKO_POSTGRES_CHANNEL="$1_ch"; TABLE="$1"; }
hr() { echo; echo "=== $* ==="; }
say() { echo "\$ $*"; }

start() {  # [host-id]
  "$BIN" serve "$ADDR" "$@" >"$WORK/server.out" 2>"$WORK/server.err" &
  SERVER_PID=$!
  for _ in $(seq 1 600); do
    grep -q '^replayed:' "$WORK/server.out" 2>/dev/null \
      && lsof -nP -iTCP:"$MYKO_PORT" -sTCP:LISTEN >/dev/null 2>&1 && break
    kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died:"; cat "$WORK/server.err"; return 1; }
    sleep 0.1
  done
  echo "server pid=$SERVER_PID: $(grep '^replayed:' "$WORK/server.out")"
}
stop() {  # [signal]
  local sig="${1:-TERM}"
  kill "-$sig" "$SERVER_PID"
  wait "$SERVER_PID" 2>/dev/null
  echo "server pid=$SERVER_PID stopped with SIG$sig; alive? $(kill -0 "$SERVER_PID" 2>/dev/null && echo yes || echo no);" \
    "port listening? $(lsof -nP -iTCP:"$MYKO_PORT" -sTCP:LISTEN >/dev/null 2>&1 && echo yes || echo no)"
  [ -s "$WORK/server.err" ] && { echo "server stderr (tail):"; tail -n 5 "$WORK/server.err" | cut -c1-300; }
  SERVER_PID=""
}
append() { "$BIN" append "$ADDR" "$@"; }
check() { say "check ${1:-(no saved head)}"; "$BIN" check "$ADDR" "$@"; }
barrier() { say "barrier $1ms"; "$BIN" barrier "$ADDR" "$1"; }

ODD='{"b":1,  "a":"héllo \"q\" \\n ✓",   "z":[1, 2.0, 1e3]}'
seed() {  # appends three records and sets HEAD
  local out; out="$(append 'first' "$ODD" 'third
with a newline and a trailing space ')"
  echo "$out" | cut -c1-20
  HEAD="$(echo "$out" | tail -n 1)"
  barrier 3000
}

t1() {
  hr "1. DECIDING TEST: three appends, real restart, replay, recompute outside myko"
  use_table t1; start; seed; echo "saved head: $HEAD"
  check "$HEAD"
  stop TERM; start; check "$HEAD"
  echo "--- second restart, this time SIGKILL"
  stop KILL; start; check "$HEAD"
  rows "$TABLE"
  sql "select count(*) as all_rows_in_table, count(*) filter (where item_type='LedgerRecord') as ledger_rows from $TABLE"
  stop TERM
}

t1b() {
  hr "1b. Same test with a fixed host id across the restart"
  use_table t1b; local id=7b0e7f6a-1111-4222-8333-444455556666
  start "$id"; seed; check "$HEAD"
  stop TERM; start "$id"; check "$HEAD"
  rows "$TABLE"
  stop TERM
}

forged_tail() {  # forged text for seq 2; re-signs 2 and 3 so the whole chain is self-consistent
  local h1 h2 h3 text3
  h1="$(sqlv "select event->'item'->>'hash' from $TABLE where item_id='1' order by id desc limit 1")"
  text3="$(sqlv "select event->'item'->>'text' from $TABLE where item_id='3' order by id desc limit 1")"
  h2="$("$BIN" hash "$h1" 2 "$1")"
  h3="$("$BIN" hash "$h2" 3 "$text3")"
  "$BIN" raw-set "$ADDR" 2 "$1" "$h2"
  "$BIN" raw-set "$ADDR" 3 "$text3" "$h3"
}

t2() {
  hr "2a. Second SET on seq 2: text changed, its old hash kept"
  use_table t2a; start; seed; local old
  old="$(sqlv "select event->'item'->>'hash' from $TABLE where item_id='2'")"
  say "raw-set 2 'FORGED' <old hash>"; "$BIN" raw-set "$ADDR" 2 FORGED "$old"
  check "$HEAD"; barrier 3000; rows "$TABLE"
  echo "--- after a real restart"; stop TERM; start; check "$HEAD"; stop TERM

  hr "2b. Second SET on seq 2 and re-signed seq 3: a chain that holds by itself"
  use_table t2b; start; seed
  say "re-sign from seq 2 onward"; forged_tail FORGED
  check; check "$HEAD"; barrier 3000; rows "$TABLE"
  echo "--- after a real restart"; stop TERM; start; check; check "$HEAD"; stop TERM

  hr "2c. DEL of a middle record (seq 2)"
  use_table t2c; start; seed
  say "raw-del 2"; "$BIN" raw-del "$ADDR" 2
  check "$HEAD"; barrier 3000; rows "$TABLE"
  echo "--- after a real restart"; stop TERM; start; check "$HEAD"; stop TERM

  hr "2d. DEL of the newest record (seq 3): truncation"
  use_table t2d; start; seed
  say "raw-del 3"; "$BIN" raw-del "$ADDR" 3
  check; check "$HEAD"; barrier 3000; rows "$TABLE"
  echo "--- then the append path, which now reuses seq 3"
  append 'a different third' | cut -c1-20
  check; check "$HEAD"; barrier 3000; rows "$TABLE"
  echo "--- after a real restart"; stop TERM; start; check; check "$HEAD"; stop TERM

  hr "2e. The generated DeleteLedgerRecord command (seq 2), no raw event"
  use_table t2e; start; seed
  say "gen-del 2"; "$BIN" gen-del "$ADDR" 2
  check "$HEAD"; barrier 3000; rows "$TABLE"; stop TERM

  hr "2f. Can the earlier versions rebuild the true chain? First version of each id, from SQL"
  use_table t2b
  sql "select distinct on (item_id::int) item_id, change_type, left(event->'item'->>'text', 28) as text,
       left(event->'item'->>'hash', 10) as hash from $TABLE where item_type='LedgerRecord' order by item_id::int, id asc"
}

t3() {
  hr "3. Raw event from an ordinary client, past the append path"
  use_table t3; start; seed
  say "raw-set 4 'WRITTEN PAST THE APPEND PATH' <64 f's>"
  "$BIN" raw-set "$ADDR" 4 'WRITTEN PAST THE APPEND PATH' ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
  check "$HEAD"; barrier 3000; rows "$TABLE"
  echo "--- append path after that: it trusts the forged head"
  append 'honest fifth' | cut -c1-20
  check; barrier 3000
  echo "--- after a real restart"; stop TERM; start; check; stop TERM

  hr "3b. One raw event whose created_at isn't a timestamp, then an honest append"
  use_table t3b; start; seed
  say "raw-set 4 x x not-a-timestamp"; "$BIN" raw-set "$ADDR" 4 x x not-a-timestamp
  append 'honest append after the bad event' | cut -c1-20
  barrier 3000
  sql "select count(*) as ledger_rows_in_table from $TABLE where item_type='LedgerRecord'"
  check
  echo "--- SIGKILL and restart: what the table held is what survives"
  stop KILL; start; check
  stop TERM
}

t4() {
  hr "4. Replay of 5,000 records"
  use_table t4empty; start; echo "(baseline: empty table)"; stop TERM
  use_table t4; start
  HEAD="$("$BIN" bulk "$ADDR" 5000 400 | tail -n 1)"; echo "head: ${HEAD:0:20}"
  barrier 30000
  sql "select count(*), pg_size_pretty(pg_total_relation_size('$TABLE')) as table_size, avg(pg_column_size(event))::int as avg_event_bytes from $TABLE"
  stop TERM
  for n in 1 2 3; do start; [ "$n" = 3 ] || stop TERM; done
  check "$HEAD"
  stop TERM
}

t5() {
  hr "5. An acknowledged append while Postgres is down, then a crash"
  use_table t5; start; seed; echo "saved head after barrier: ${HEAD:0:20}"
  say "stop the scratch Postgres"; pg_stop
  local out; out="$(append 'acked four' 'acked five')"; echo "append responses:"; echo "$out" | cut -c1-20
  local acked; acked="$(echo "$out" | tail -n 1)"
  barrier 2000
  check "$acked"
  say "SIGKILL the server, start Postgres, restart"; stop KILL; pg_start; start
  check "$acked"; check "$HEAD"; stop TERM
}

t6() {
  hr "6. Eight concurrent clients, 50 appends each, through the writer lock"
  use_table t6; start
  for _ in 1 2 3 4 5 6 7 8; do "$BIN" bulk "$ADDR" 50 40 >/dev/null 2>&1 & pids="$pids $!"; done
  wait $pids; pids=""
  check; barrier 5000; stop TERM; start; check; stop TERM

  hr "6b. The same with the writer lock removed"
  use_table t6b; SPIKE_NO_APPEND_LOCK=1 start
  for _ in 1 2 3 4 5 6 7 8; do "$BIN" bulk "$ADDR" 50 40 >/dev/null 2>&1 & pids="$pids $!"; done
  wait $pids; pids=""
  check; barrier 5000
  sql "select count(*) as rows, count(distinct item_id) as distinct_ids from $TABLE"
  stop TERM
}

t7() {
  hr "7. In-process: run() never called, so no listener"
  use_table t7
  SPIKE_HOLD_MS=1500 "$BIN" embedded-append 'first' "$ODD" 'third' >"$WORK/emb.out" 2>&1 &
  local pid=$!
  for _ in $(seq 1 100); do grep -q holding "$WORK/emb.out" && break; sleep 0.1; done
  cut -c1-120 "$WORK/emb.out"
  echo "lsof LISTEN sockets of pid $pid:"; lsof -nP -a -p "$pid" -iTCP -sTCP:LISTEN || echo "  (none)"
  echo "lsof all TCP of pid $pid:"; lsof -nP -a -p "$pid" -iTCP | awk 'NR>1{print "  "$8,$9,$10}'
  wait "$pid"
  local head; head="$(grep -E '^[0-9]+:[0-9a-f]{64}$' "$WORK/emb.out" | tail -n 1)"
  say "new process: embedded-check"; "$BIN" embedded-check "$head"
  rows "$TABLE"
}

pids=""
for s in $SCENARIOS; do "$s"; done
hr "server log errors from the scratch Postgres (distinct, tail)"
grep ERROR "$WORK/pg.log" | sed 's/^[^E]*//' | sort | uniq -c | tail -n 5 | cut -c1-200
