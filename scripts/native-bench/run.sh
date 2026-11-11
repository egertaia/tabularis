#!/usr/bin/env bash
# Compares the Tauri app (500 and 1M rows per page) with the native GPUI shell on macOS.
set -uo pipefail

RUNS=5
TIMEOUT_S=${BENCH_TIMEOUT_S:-180}
while [[ $# -gt 0 ]]; do
  case "$1" in
    --runs) RUNS="$2"; shift 2 ;;
    *) echo "usage: $0 [--runs N]" >&2; exit 2 ;;
  esac
done

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DIFF="$ROOT/scripts/native-bench/tauri-bench.diff"
WORK="$(mktemp -d)"
PATCHED=0

cleanup() {
  if [[ $PATCHED -eq 1 ]]; then
    git -C "$ROOT" apply -R "$DIFF" || echo "WARNING: could not revert tauri-bench.diff" >&2
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

build_native() {
  echo ">> building native" >&2
  (cd "$ROOT/src-tauri" && cargo build --release --no-default-features --features native) >&2 || return 1
  cp "$ROOT/src-tauri/target/release/tabularis" "$WORK/tabularis-native"
}

build_tauri() {
  echo ">> building tauri bench app" >&2
  git -C "$ROOT" apply "$DIFF" || return 1
  PATCHED=1
  (cd "$ROOT" && pnpm install --frozen-lockfile && pnpm build:plugin-guest && pnpm build) >&2 || return 1
  (cd "$ROOT/src-tauri" && cargo build --release --features tauri/custom-protocol) >&2 || return 1
  cp "$ROOT/src-tauri/target/release/tabularis" "$WORK/tabularis-tauri"
}

NATIVE_OK=1; TAURI_OK=1
build_native || NATIVE_OK=0
build_tauri || TAURI_OK=0

# Fixtures
: > "$WORK/bench.sqlite"
cat > "$WORK/connections.json" <<JSON
[{"id":"bench-sqlite","name":"BENCH SQLITE","params":{"driver":"sqlite","host":null,"port":null,"username":null,"password":null,"database":"$WORK/bench.sqlite"}}]
JSON
cat > "$WORK/query.sql" <<'SQL'
WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i < 1000000)
SELECT i AS id, 'user_' || i AS name, i * 0.37 AS amount,
       date('2020-01-01', '+' || (i % 2000) || ' days') AS day,
       CASE WHEN i % 7 = 0 THEN NULL ELSE i % 100 END AS bucket,
       hex(randomblob(8)) AS token
FROM n;
SQL

webkit_pids() { ps -axo pid=,command= | awk '/com\.apple\.WebKit\./ && !/awk/ {print $1}' | sort; }
rss_kb() { ps -o rss= -p "$1" 2>/dev/null | tr -d ' '; }

# run_once <scenario> <binary> <page_size> <args...>; prints one "key=value ..." line.
run_once() {
  local scenario="$1" bin="$2" page="$3"; shift 3
  local data="$WORK/data-$scenario" out="$WORK/out-$scenario.txt"
  rm -rf "$data"; mkdir -p "$data"
  cp "$WORK/connections.json" "$data/connections.json"
  printf '{"resultPageSize":%s,"showWelcome":false,"checkForUpdates":false,"autoCheckUpdatesOnStartup":false}\n' "$page" > "$data/config.json"

  local before; before="$(webkit_pids)"
  TABULARIS_DATA_DIR="$data" TABULARIS_BENCH_SQL="$WORK/query.sql" \
    TABULARIS_BENCH_CONNECTION=bench-sqlite "$bin" "$@" >"$out" 2>&1 &
  local pid=$! peak=0 timed_out=0 start=$SECONDS
  while kill -0 "$pid" 2>/dev/null; do
    local total=0 p kb
    for p in $pid $(comm -13 <(echo "$before") <(webkit_pids)); do
      kb="$(rss_kb "$p")"; total=$((total + ${kb:-0}))
    done
    (( total > peak )) && peak=$total
    if (( SECONDS - start > TIMEOUT_S )); then timed_out=1; kill -9 "$pid" 2>/dev/null; break; fi
    sleep 0.1
  done
  wait "$pid" 2>/dev/null; local code=$?
  (( timed_out )) && code=timeout

  local line; line="$(grep -h '^BENCH ' "$out" | sed 's/^BENCH //' | tr '\n' ' ')"
  echo "exit=$code peak_rss_mb=$((peak / 1024)) $line"
}

val() { tr ' ' '\n' <<<"$1" | sed -n "s/^$2=//p" | head -1; }

median() {
  local n; n=$(wc -l <<<"$1" | tr -d ' ')
  [[ -z "$1" ]] && { echo "-"; return; }
  sort -g <<<"$1" | awk -v n="$n" '{a[NR]=$1} END {print (n%2 ? a[(n+1)/2] : (a[n/2]+a[n/2+1])/2)}'
}

SUMMARY=""
METRICS="rows first_paint_ms all_loaded_ms scroll_p50_ms scroll_p99_ms peak_rss_mb"
run_scenario() {
  local scenario="$1" ok="$2" bin="$3" page="$4"; shift 4
  local failures=0 i line k v m cells=""
  if [[ $ok -eq 0 ]]; then
    SUMMARY+="| $scenario | build failed | | | | | | $RUNS |"$'\n'; return
  fi
  for k in $METRICS; do : > "$WORK/col-$scenario-$k"; done
  for ((i = 1; i <= RUNS; i++)); do
    line="$(run_once "$scenario" "$bin" "$page" "$@")"
    echo "[$scenario run $i] $line" >&2
    if [[ "$(val "$line" exit)" != 0 || -n "$(val "$line" error)" || -z "$(val "$line" first_paint_ms)" ]]; then
      failures=$((failures + 1))
    fi
    for k in $METRICS; do
      v="$(val "$line" "$k")"
      [[ "$v" =~ ^[0-9.]+$ ]] && echo "$v" >> "$WORK/col-$scenario-$k"
    done
  done
  for k in $METRICS; do
    m="$(median "$(cat "$WORK/col-$scenario-$k")")"
    [[ "$k" == all_loaded_ms && "$m" == "-" ]] && m="na"
    cells+=" $m |"
  done
  SUMMARY+="| $scenario |$cells $failures |"$'\n'
}

run_scenario native "$NATIVE_OK" "$WORK/tabularis-native" 500 native
run_scenario tauri-500 "$TAURI_OK" "$WORK/tabularis-tauri" 500
run_scenario tauri-1m "$TAURI_OK" "$WORK/tabularis-tauri" 1000000

echo
echo "| scenario | rows | first_paint_ms | all_loaded_ms | scroll_p50_ms | scroll_p99_ms | peak_rss_mb | failures |"
echo "|---|---|---|---|---|---|---|---|"
printf '%s' "$SUMMARY"
