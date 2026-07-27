#!/usr/bin/env bash

set -euo pipefail

BENCH_MODE="scan"
BENCH_RUNS=""
BENCH_WARMUPS=""
BENCH_DURATION="5"
BENCH_INTERVAL="0.01"
BENCH_OUTPUT="benchmark.csv"
BENCH_BUILD=1
BENCH_BINARY="./target/release/cefdetector"
BENCH_CURRENT_RSS=0
BENCH_PEAK_RSS=0

bench_usage() {
    cat <<'EOF'
Usage: ./benchmark.sh [scan|gui] [OPTIONS]

Modes:
  scan                 Benchmark a complete CLI scan (default)
  gui                  Sample GUI memory for a fixed duration

Options:
  -r, --runs N         Measured runs (default: 5 for scan, 1 for gui)
  -w, --warmup N       Unmeasured warmup runs (default: 1 for scan, 0 for gui)
  -d, --duration SEC   GUI sampling duration (default: 5)
  -i, --interval SEC   /proc sampling interval (default: 0.01)
  -o, --output FILE    CSV output path (default: benchmark.csv)
      --no-build       Reuse the existing release binary
  -h, --help           Show this help

Examples:
  ./benchmark.sh scan --runs 10 --warmup 2
  ./benchmark.sh gui --duration 10 --output gui-benchmark.csv
EOF
}

bench_fail() {
    echo "benchmark: $*" >&2
    exit 2
}

bench_require_command() {
    command -v "$1" >/dev/null 2>&1 || bench_fail "required command not found: $1"
}

bench_positive_integer() {
    [[ "$1" =~ ^[0-9]+$ ]] && [ "$1" -gt 0 ]
}

bench_nonnegative_integer() {
    [[ "$1" =~ ^[0-9]+$ ]]
}

if [ "${1:-}" = "scan" ] || [ "${1:-}" = "gui" ]; then
    BENCH_MODE=$1
    shift
fi

while [ "$#" -gt 0 ]; do
    case "$1" in
        -r|--runs)
            [ "$#" -ge 2 ] || bench_fail "$1 requires a value"
            BENCH_RUNS=$2
            shift 2
            ;;
        -w|--warmup)
            [ "$#" -ge 2 ] || bench_fail "$1 requires a value"
            BENCH_WARMUPS=$2
            shift 2
            ;;
        -d|--duration)
            [ "$#" -ge 2 ] || bench_fail "$1 requires a value"
            BENCH_DURATION=$2
            shift 2
            ;;
        -i|--interval)
            [ "$#" -ge 2 ] || bench_fail "$1 requires a value"
            BENCH_INTERVAL=$2
            shift 2
            ;;
        -o|--output)
            [ "$#" -ge 2 ] || bench_fail "$1 requires a value"
            BENCH_OUTPUT=$2
            shift 2
            ;;
        --no-build)
            BENCH_BUILD=0
            shift
            ;;
        -h|--help)
            bench_usage
            exit 0
            ;;
        *)
            bench_fail "unknown argument: $1"
            ;;
    esac
done

if [ "$BENCH_MODE" = "scan" ]; then
    BENCH_RUNS=${BENCH_RUNS:-5}
    BENCH_WARMUPS=${BENCH_WARMUPS:-1}
else
    BENCH_RUNS=${BENCH_RUNS:-1}
    BENCH_WARMUPS=${BENCH_WARMUPS:-0}
fi

bench_positive_integer "$BENCH_RUNS" || bench_fail "--runs must be a positive integer"
bench_nonnegative_integer "$BENCH_WARMUPS" || bench_fail "--warmup must be a nonnegative integer"
bench_require_command date
bench_require_command sleep
bench_require_command stat
bench_require_command awk

if [ ! -r /proc/self/status ]; then
    bench_fail "Linux /proc is required for memory sampling"
fi
if [ "$BENCH_MODE" = "gui" ] \
    && [ -z "${DISPLAY:-}" ] \
    && [ -z "${WAYLAND_DISPLAY:-}" ]; then
    bench_fail "gui mode requires DISPLAY or WAYLAND_DISPLAY"
fi

if [ "$BENCH_BUILD" -eq 1 ]; then
    bench_require_command cargo
    echo "Building the locked release profile..."
    cargo build --locked --release
fi
[ -x "$BENCH_BINARY" ] || bench_fail "release binary not found: $BENCH_BINARY"

BENCH_TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/cefdetector-benchmark.XXXXXX")
trap 'rm -rf "$BENCH_TMP_DIR"' EXIT

bench_sample_process_tree() {
    local root_pid=$1
    local -a pending=("$root_pid")
    local -a children=()
    local process_id status_file children_file key value
    BENCH_CURRENT_RSS=0

    while [ "${#pending[@]}" -gt 0 ]; do
        process_id=${pending[0]}
        pending=("${pending[@]:1}")
        status_file="/proc/$process_id/status"
        children_file="/proc/$process_id/task/$process_id/children"

        if [ -r "$status_file" ]; then
            while read -r key value _; do
                if [ "$key" = "VmRSS:" ]; then
                    BENCH_CURRENT_RSS=$((BENCH_CURRENT_RSS + value))
                    break
                fi
            done < "$status_file"
        fi

        children=()
        if [ -r "$children_file" ]; then
            read -r -a children < "$children_file" || true
            if [ "${#children[@]}" -gt 0 ]; then
                pending+=("${children[@]}")
            fi
        fi
    done

    if [ "$BENCH_CURRENT_RSS" -gt "$BENCH_PEAK_RSS" ]; then
        BENCH_PEAK_RSS=$BENCH_CURRENT_RSS
    fi
}

bench_monitor_until_exit() {
    local process_id=$1
    BENCH_PEAK_RSS=0
    while kill -0 "$process_id" 2>/dev/null; do
        bench_sample_process_tree "$process_id"
        sleep "$BENCH_INTERVAL"
    done
}

bench_monitor_for_duration() {
    local process_id=$1
    local timer_pid
    BENCH_PEAK_RSS=0
    sleep "$BENCH_DURATION" &
    timer_pid=$!
    while kill -0 "$process_id" 2>/dev/null && kill -0 "$timer_pid" 2>/dev/null; do
        bench_sample_process_tree "$process_id"
        sleep "$BENCH_INTERVAL"
    done
    if kill -0 "$timer_pid" 2>/dev/null; then
        kill "$timer_pid" 2>/dev/null || true
    fi
    wait "$timer_pid" 2>/dev/null || true
}

bench_stop_process() {
    local process_id=$1
    if kill -0 "$process_id" 2>/dev/null; then
        kill -TERM "$process_id" 2>/dev/null || true
    fi
}

bench_warmup_scan() {
    local warmup
    for ((warmup = 1; warmup <= BENCH_WARMUPS; warmup++)); do
        echo "Warmup $warmup/$BENCH_WARMUPS..."
        "$BENCH_BINARY" --json --output "$BENCH_TMP_DIR/warmup-$warmup.json"
    done
}

bench_warmup_gui() {
    local warmup process_id
    for ((warmup = 1; warmup <= BENCH_WARMUPS; warmup++)); do
        echo "Warmup $warmup/$BENCH_WARMUPS..."
        "$BENCH_BINARY" &
        process_id=$!
        sleep "$BENCH_DURATION"
        bench_stop_process "$process_id"
        wait "$process_id" 2>/dev/null || true
    done
}

if [ "$BENCH_MODE" = "scan" ]; then
    bench_warmup_scan
else
    bench_warmup_gui
fi

BENCH_BINARY_BYTES=$(stat -c '%s' "$BENCH_BINARY")
printf 'mode,run,elapsed_ms,peak_rss_kib,exit_status,result_count,binary_bytes\n' > "$BENCH_OUTPUT"

for ((BENCH_RUN = 1; BENCH_RUN <= BENCH_RUNS; BENCH_RUN++)); do
    BENCH_RESULT_FILE="$BENCH_TMP_DIR/result-$BENCH_RUN.json"
    BENCH_START_NS=$(date +%s%N)

    if [ "$BENCH_MODE" = "scan" ]; then
        "$BENCH_BINARY" --json --output "$BENCH_RESULT_FILE" &
    else
        "$BENCH_BINARY" &
    fi
    BENCH_PROCESS_ID=$!

    if [ "$BENCH_MODE" = "scan" ]; then
        bench_monitor_until_exit "$BENCH_PROCESS_ID"
    else
        bench_monitor_for_duration "$BENCH_PROCESS_ID"
        bench_stop_process "$BENCH_PROCESS_ID"
    fi

    if wait "$BENCH_PROCESS_ID" 2>/dev/null; then
        BENCH_STATUS=0
    else
        BENCH_STATUS=$?
        if [ "$BENCH_MODE" = "gui" ] && [ "$BENCH_STATUS" -eq 143 ]; then
            BENCH_STATUS=0
        fi
    fi
    BENCH_END_NS=$(date +%s%N)
    BENCH_ELAPSED_MS=$(( (BENCH_END_NS - BENCH_START_NS) / 1000000 ))

    if [ "$BENCH_MODE" = "scan" ]; then
        BENCH_RESULT_COUNT=$(awk '/"file":[[:space:]]/{count++} END {print count + 0}' "$BENCH_RESULT_FILE")
    else
        BENCH_RESULT_COUNT=""
    fi

    printf '%s,%d,%d,%d,%d,%s,%d\n' \
        "$BENCH_MODE" \
        "$BENCH_RUN" \
        "$BENCH_ELAPSED_MS" \
        "$BENCH_PEAK_RSS" \
        "$BENCH_STATUS" \
        "$BENCH_RESULT_COUNT" \
        "$BENCH_BINARY_BYTES" >> "$BENCH_OUTPUT"
    printf 'Run %d/%d: %d ms, %.2f MiB peak RSS' \
        "$BENCH_RUN" \
        "$BENCH_RUNS" \
        "$BENCH_ELAPSED_MS" \
        "$(awk -v kib="$BENCH_PEAK_RSS" 'BEGIN { print kib / 1024 }')"
    if [ "$BENCH_MODE" = "scan" ]; then
        printf ', %s results' "$BENCH_RESULT_COUNT"
    fi
    printf '\n'
done

awk -F, '
    NR > 1 {
        runs++
        elapsed += $3
        rss += $4
        if (runs == 1 || $3 < elapsed_min) elapsed_min = $3
        if (runs == 1 || $3 > elapsed_max) elapsed_max = $3
        if (runs == 1 || $4 < rss_min) rss_min = $4
        if (runs == 1 || $4 > rss_max) rss_max = $4
        if ($5 != 0) failures++
    }
    END {
        printf "\nSummary (%d runs):\n", runs
        printf "  elapsed: %.1f ms mean (%d-%d ms)\n", elapsed / runs, elapsed_min, elapsed_max
        printf "  peak RSS: %.2f MiB mean (%.2f-%.2f MiB)\n",
            rss / runs / 1024, rss_min / 1024, rss_max / 1024
        printf "  failures: %d\n", failures + 0
    }
' "$BENCH_OUTPUT"

echo "Raw results: $BENCH_OUTPUT"
