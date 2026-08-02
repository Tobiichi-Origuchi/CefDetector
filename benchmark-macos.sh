#!/usr/bin/env bash

set -Eeuo pipefail

BENCH_SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
readonly BENCH_SCRIPT_DIR
cd "${BENCH_SCRIPT_DIR}"

BENCH_MODE="all"
BENCH_SCAN_RUNS=5
BENCH_GUI_RUNS=1
BENCH_SCAN_WARMUPS=1
BENCH_GUI_WARMUPS=0
BENCH_GUI_DURATION_SECONDS="5"
BENCH_SAMPLE_INTERVAL_MS=10
BENCH_TIMEOUT_SECONDS=300
BENCH_OUTPUT="benchmark-macos.csv"
BENCH_BUILD=true

BENCH_ARTIFACT_DIR="${BENCH_SCRIPT_DIR}/target/release/benchmark-macos"
BENCH_IGNORE_BINARY="${BENCH_ARTIFACT_DIR}/cefdetector-ignore"
BENCH_SPOTLIGHT_BINARY="${BENCH_ARTIFACT_DIR}/cefdetector-spotlight"
BENCH_RESULTS_HELPER="${BENCH_SCRIPT_DIR}/.github/scripts/benchmark-macos-results.js"
BENCH_TEMP_DIR=""
BENCH_FIRST_IGNORE_RESULT=""
BENCH_FIRST_SPOTLIGHT_RESULT=""

BENCH_CURRENT_RSS_KIB=0
BENCH_CURRENT_VIRTUAL_KIB=0
BENCH_CURRENT_THREADS=0
BENCH_PEAK_RSS_KIB=0
BENCH_PEAK_VIRTUAL_KIB=0
BENCH_PEAK_THREADS=0
BENCH_SAMPLE_COUNT=0
BENCH_TIMED_OUT=false
BENCH_STOP_METHOD="process-exit"
BENCH_PROCESS_ID=""
BENCH_SAMPLE_INTERVAL_SECONDS=""
BENCH_GUI_DURATION_NS=0
BENCH_TIMEOUT_NS=0
BENCH_LOGICAL_PROCESSORS=0
BENCH_TREE_PIDS=()

bench_usage() {
    cat <<'EOF'
Usage: ./benchmark-macos.sh [all|scan|gui] [OPTIONS]

Benchmarks the ignore and Spotlight macOS search backends. Spotlight scan
results depend on the system metadata index and privacy exclusions.

Modes:
  all                          Run scan and GUI measurements (default)
  scan                         Benchmark complete CLI searches
  gui                          Benchmark first frame and fixed-duration GUI use

Options:
      --scan-runs N            Measured CLI runs (default: 5)
      --gui-runs N             Measured GUI runs (default: 1)
  -r, --runs N                 Runs for the selected mode; sets both in all mode
      --scan-warmup-runs N     Unmeasured CLI warmups (default: 1)
      --gui-warmup-runs N      Unmeasured GUI warmups (default: 0)
  -w, --warmup N               Warmups for the selected mode; sets both in all
  -d, --duration SEC           GUI sample duration (default: 5)
      --sample-interval-ms N   ps sampling interval in ms (default: 10)
  -i, --interval SEC           Compatibility alias using seconds
      --timeout SEC            Scan/startup timeout (default: 300)
  -o, --output FILE            CSV output (default: benchmark-macos.csv)
      --no-build               Reuse backend binaries built by this script
  -h, --help                   Show this help

Examples:
  ./benchmark-macos.sh
  ./benchmark-macos.sh scan --scan-runs 10 --scan-warmup-runs 2
  ./benchmark-macos.sh gui --duration 10 --output gui-macos.csv
  ./benchmark-macos.sh all --no-build
EOF
}

bench_fail() {
    printf 'benchmark: %s\n' "$*" >&2
    exit 2
}

bench_cleanup() {
    if [[ -n "${BENCH_TEMP_DIR}" && -d "${BENCH_TEMP_DIR}" ]]; then
        case "${BENCH_TEMP_DIR}" in
            "${TMPDIR:-/tmp}"/cefdetector-benchmark.*)
                rm -rf -- "${BENCH_TEMP_DIR}"
                ;;
        esac
    fi
}

trap bench_cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

bench_require_command() {
    command -v "$1" >/dev/null 2>&1 ||
        bench_fail "required command not found: $1"
}

bench_positive_integer() {
    [[ "$1" =~ ^[0-9]+$ ]] && ((10#$1 > 0))
}

bench_nonnegative_integer() {
    [[ "$1" =~ ^[0-9]+$ ]]
}

bench_positive_number() {
    [[ "$1" =~ ^([0-9]+([.][0-9]*)?|[.][0-9]+)$ ]] &&
        awk -v value="$1" 'BEGIN { exit !(value > 0) }'
}

bench_set_mode_runs() {
    local value=$1
    if [[ "${BENCH_MODE}" == "scan" ]]; then
        BENCH_SCAN_RUNS=$value
    elif [[ "${BENCH_MODE}" == "gui" ]]; then
        BENCH_GUI_RUNS=$value
    else
        BENCH_SCAN_RUNS=$value
        BENCH_GUI_RUNS=$value
    fi
}

bench_set_mode_warmups() {
    local value=$1
    if [[ "${BENCH_MODE}" == "scan" ]]; then
        BENCH_SCAN_WARMUPS=$value
    elif [[ "${BENCH_MODE}" == "gui" ]]; then
        BENCH_GUI_WARMUPS=$value
    else
        BENCH_SCAN_WARMUPS=$value
        BENCH_GUI_WARMUPS=$value
    fi
}

if [[ "${1:-}" == "all" || "${1:-}" == "scan" || "${1:-}" == "gui" ]]; then
    BENCH_MODE=$1
    shift
fi

while (($# > 0)); do
    case "$1" in
        --mode)
            (($# >= 2)) || bench_fail "$1 requires a value"
            BENCH_MODE=$2
            shift 2
            ;;
        --scan-runs)
            (($# >= 2)) || bench_fail "$1 requires a value"
            BENCH_SCAN_RUNS=$2
            shift 2
            ;;
        --gui-runs)
            (($# >= 2)) || bench_fail "$1 requires a value"
            BENCH_GUI_RUNS=$2
            shift 2
            ;;
        -r | --runs)
            (($# >= 2)) || bench_fail "$1 requires a value"
            bench_set_mode_runs "$2"
            shift 2
            ;;
        --scan-warmup-runs)
            (($# >= 2)) || bench_fail "$1 requires a value"
            BENCH_SCAN_WARMUPS=$2
            shift 2
            ;;
        --gui-warmup-runs)
            (($# >= 2)) || bench_fail "$1 requires a value"
            BENCH_GUI_WARMUPS=$2
            shift 2
            ;;
        -w | --warmup)
            (($# >= 2)) || bench_fail "$1 requires a value"
            bench_set_mode_warmups "$2"
            shift 2
            ;;
        -d | --duration)
            (($# >= 2)) || bench_fail "$1 requires a value"
            BENCH_GUI_DURATION_SECONDS=$2
            shift 2
            ;;
        --sample-interval-ms)
            (($# >= 2)) || bench_fail "$1 requires a value"
            BENCH_SAMPLE_INTERVAL_MS=$2
            shift 2
            ;;
        -i | --interval)
            (($# >= 2)) || bench_fail "$1 requires a value"
            bench_positive_number "$2" ||
                bench_fail "$1 must be a positive number"
            BENCH_SAMPLE_INTERVAL_MS=$(
                awk -v seconds="$2" 'BEGIN { printf "%.0f", seconds * 1000 }'
            )
            shift 2
            ;;
        --timeout)
            (($# >= 2)) || bench_fail "$1 requires a value"
            BENCH_TIMEOUT_SECONDS=$2
            shift 2
            ;;
        -o | --output)
            (($# >= 2)) || bench_fail "$1 requires a value"
            BENCH_OUTPUT=$2
            shift 2
            ;;
        --no-build)
            BENCH_BUILD=false
            shift
            ;;
        -h | --help)
            bench_usage
            exit 0
            ;;
        *)
            bench_fail "unknown argument: $1"
            ;;
    esac
done

[[ "$(uname -s)" == "Darwin" ]] ||
    bench_fail "benchmark-macos.sh must run on macOS"
[[ "${BENCH_MODE}" == "all" || "${BENCH_MODE}" == "scan" || "${BENCH_MODE}" == "gui" ]] ||
    bench_fail "--mode must be one of: all, scan, gui"
bench_positive_integer "${BENCH_SCAN_RUNS}" ||
    bench_fail "--scan-runs must be a positive integer"
bench_positive_integer "${BENCH_GUI_RUNS}" ||
    bench_fail "--gui-runs must be a positive integer"
bench_nonnegative_integer "${BENCH_SCAN_WARMUPS}" ||
    bench_fail "--scan-warmup-runs must be a nonnegative integer"
bench_nonnegative_integer "${BENCH_GUI_WARMUPS}" ||
    bench_fail "--gui-warmup-runs must be a nonnegative integer"
bench_positive_number "${BENCH_GUI_DURATION_SECONDS}" ||
    bench_fail "--duration must be a positive number"
bench_positive_integer "${BENCH_SAMPLE_INTERVAL_MS}" ||
    bench_fail "--sample-interval-ms must be a positive integer"
bench_positive_integer "${BENCH_TIMEOUT_SECONDS}" ||
    bench_fail "--timeout must be a positive integer"

for command in awk date env getconf install mktemp osascript perl pgrep ps sleep stat time; do
    bench_require_command "${command}"
done
if [[ "${BENCH_BUILD}" == true ]]; then
    bench_require_command cargo
fi
[[ -r "${BENCH_RESULTS_HELPER}" ]] ||
    bench_fail "result helper not found: ${BENCH_RESULTS_HELPER}"

BENCH_SAMPLE_INTERVAL_SECONDS=$(
    awk -v milliseconds="${BENCH_SAMPLE_INTERVAL_MS}" \
        'BEGIN { printf "%.6f", milliseconds / 1000 }'
)
BENCH_GUI_DURATION_NS=$(
    awk -v seconds="${BENCH_GUI_DURATION_SECONDS}" \
        'BEGIN { printf "%.0f", seconds * 1000000000 }'
)
BENCH_TIMEOUT_NS=$((10#${BENCH_TIMEOUT_SECONDS} * 1000000000))
BENCH_LOGICAL_PROCESSORS=$(getconf _NPROCESSORS_ONLN)
bench_positive_integer "${BENCH_LOGICAL_PROCESSORS}" ||
    bench_fail "getconf returned an invalid processor count"

bench_build_binaries() {
    mkdir -p -- "${BENCH_ARTIFACT_DIR}"

    printf 'Building locked release binary for the ignore backend...\n'
    cargo build --locked --release --no-default-features --features gui
    install -m755 target/release/cefdetector "${BENCH_IGNORE_BINARY}"

    printf 'Building locked release binary for the Spotlight backend...\n'
    cargo build --locked --release --no-default-features --features gui,index
    install -m755 target/release/cefdetector "${BENCH_SPOTLIGHT_BINARY}"
}

if [[ "${BENCH_BUILD}" == true ]]; then
    bench_build_binaries
fi
for binary in "${BENCH_IGNORE_BINARY}" "${BENCH_SPOTLIGHT_BINARY}"; do
    [[ -x "${binary}" ]] ||
        bench_fail "benchmark binary not found: ${binary} (run without --no-build)"
done

BENCH_TEMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/cefdetector-benchmark.XXXXXXXX")

bench_now_ns() {
    /usr/bin/perl -MTime::HiRes=clock_gettime,CLOCK_MONOTONIC \
        -e 'printf "%.0f\n", clock_gettime(CLOCK_MONOTONIC) * 1000000000'
}

bench_process_alive() {
    local process_id=$1
    local state
    kill -0 "${process_id}" 2>/dev/null || return 1
    state=$(
        /bin/ps -o state= -p "${process_id}" 2>/dev/null |
            awk 'NR == 1 { print $1 }' || true
    )
    [[ -n "${state}" && "${state}" != Z* ]]
}

bench_collect_process_tree() {
    local root_pid=$1
    local process_id child children seen=" "
    local -a pending=("${root_pid}")
    BENCH_TREE_PIDS=()

    while ((${#pending[@]} > 0)); do
        process_id=${pending[0]}
        pending=("${pending[@]:1}")
        [[ "${process_id}" =~ ^[0-9]+$ ]] || continue
        [[ "${seen}" != *" ${process_id} "* ]] || continue
        seen="${seen}${process_id} "
        kill -0 "${process_id}" 2>/dev/null || continue
        BENCH_TREE_PIDS+=("${process_id}")
        children=$(/usr/bin/pgrep -P "${process_id}" 2>/dev/null || true)
        for child in ${children}; do
            pending+=("${child}")
        done
    done
}

bench_sample_process_tree() {
    local root_pid=$1
    local process_id row rss_kib virtual_kib threads

    BENCH_CURRENT_RSS_KIB=0
    BENCH_CURRENT_VIRTUAL_KIB=0
    BENCH_CURRENT_THREADS=0
    bench_collect_process_tree "${root_pid}"
    for process_id in "${BENCH_TREE_PIDS[@]}"; do
        row=$(
            /bin/ps -o rss=,vsz= -p "${process_id}" 2>/dev/null |
                awk 'NR == 1 { print $1, $2 }' || true
        )
        rss_kib=0
        virtual_kib=0
        if [[ -n "${row}" ]]; then
            read -r rss_kib virtual_kib <<<"${row}"
        fi
        threads=$(
            /bin/ps -M -p "${process_id}" 2>/dev/null |
                awk 'NR > 1 { count++ } END { print count + 0 }' || true
        )
        BENCH_CURRENT_RSS_KIB=$((BENCH_CURRENT_RSS_KIB + ${rss_kib:-0}))
        BENCH_CURRENT_VIRTUAL_KIB=$((BENCH_CURRENT_VIRTUAL_KIB + ${virtual_kib:-0}))
        BENCH_CURRENT_THREADS=$((BENCH_CURRENT_THREADS + ${threads:-0}))
    done

    ((BENCH_CURRENT_RSS_KIB <= BENCH_PEAK_RSS_KIB)) ||
        BENCH_PEAK_RSS_KIB=${BENCH_CURRENT_RSS_KIB}
    ((BENCH_CURRENT_VIRTUAL_KIB <= BENCH_PEAK_VIRTUAL_KIB)) ||
        BENCH_PEAK_VIRTUAL_KIB=${BENCH_CURRENT_VIRTUAL_KIB}
    ((BENCH_CURRENT_THREADS <= BENCH_PEAK_THREADS)) ||
        BENCH_PEAK_THREADS=${BENCH_CURRENT_THREADS}
    BENCH_SAMPLE_COUNT=$((BENCH_SAMPLE_COUNT + 1))
}

bench_reset_metrics() {
    BENCH_PEAK_RSS_KIB=0
    BENCH_PEAK_VIRTUAL_KIB=0
    BENCH_PEAK_THREADS=0
    BENCH_SAMPLE_COUNT=0
    BENCH_TIMED_OUT=false
    BENCH_STOP_METHOD="process-exit"
}

bench_monitor_until_exit() {
    local process_id=$1
    local start_ns=$2
    local now_ns

    while bench_process_alive "${process_id}"; do
        bench_sample_process_tree "${process_id}"
        now_ns=$(bench_now_ns)
        if ((now_ns - start_ns >= BENCH_TIMEOUT_NS)); then
            BENCH_TIMED_OUT=true
            return
        fi
        sleep "${BENCH_SAMPLE_INTERVAL_SECONDS}"
    done
}

bench_monitor_for_duration() {
    local process_id=$1
    local start_ns=$2
    local now_ns

    while bench_process_alive "${process_id}"; do
        bench_sample_process_tree "${process_id}"
        now_ns=$(bench_now_ns)
        ((now_ns - start_ns < BENCH_GUI_DURATION_NS)) || return 0
        sleep "${BENCH_SAMPLE_INTERVAL_SECONDS}"
    done
    return 0
}

bench_stop_process_tree() {
    local root_pid=$1
    local index process_id attempt

    bench_collect_process_tree "${root_pid}"
    BENCH_STOP_METHOD="signal-term"
    for ((index = ${#BENCH_TREE_PIDS[@]} - 1; index >= 1; index--)); do
        process_id=${BENCH_TREE_PIDS[index]}
        kill -TERM "${process_id}" 2>/dev/null || true
    done

    for ((attempt = 0; attempt < 50; attempt++)); do
        bench_process_alive "${root_pid}" || return 0
        sleep 0.02
    done

    BENCH_STOP_METHOD="signal-kill"
    bench_collect_process_tree "${root_pid}"
    for ((index = ${#BENCH_TREE_PIDS[@]} - 1; index >= 1; index--)); do
        process_id=${BENCH_TREE_PIDS[index]}
        kill -KILL "${process_id}" 2>/dev/null || true
    done
    kill -TERM "${root_pid}" 2>/dev/null || true
}

bench_start_process() {
    local measure_mode=$1
    local binary=$2
    local result_file=$3
    local stdout_file=$4
    local stderr_file=$5

    if [[ "${measure_mode}" == "scan" ]]; then
        env -u CEFDETECTOR_GUI_SMOKE_TEST /usr/bin/time -l \
            "${binary}" --json --output "${result_file}" \
            >"${stdout_file}" 2>"${stderr_file}" &
    elif [[ "${measure_mode}" == "gui-startup" ]]; then
        CEFDETECTOR_GUI_SMOKE_TEST=1 /usr/bin/time -l \
            "${binary}" >"${stdout_file}" 2>"${stderr_file}" &
    else
        env -u CEFDETECTOR_GUI_SMOKE_TEST /usr/bin/time -l \
            "${binary}" >"${stdout_file}" 2>"${stderr_file}" &
    fi
    BENCH_PROCESS_ID=$!
}

bench_warmup_scan() {
    local backend=$1
    local binary=$2
    local warmup result_file

    for ((warmup = 1; warmup <= BENCH_SCAN_WARMUPS; warmup++)); do
        printf 'Warmup %d/%d: %s scan\n' \
            "${warmup}" "${BENCH_SCAN_WARMUPS}" "${backend}"
        result_file="${BENCH_TEMP_DIR}/warmup-${backend}-scan-${warmup}.json"
        "${binary}" --json --output "${result_file}"
    done
}

bench_warmup_gui() {
    local backend=$1
    local binary=$2
    local warmup process_id

    for ((warmup = 1; warmup <= BENCH_GUI_WARMUPS; warmup++)); do
        printf 'Warmup %d/%d: %s gui\n' \
            "${warmup}" "${BENCH_GUI_WARMUPS}" "${backend}"
        env -u CEFDETECTOR_GUI_SMOKE_TEST "${binary}" \
            >"${BENCH_TEMP_DIR}/warmup-${backend}-gui-${warmup}.out" \
            2>"${BENCH_TEMP_DIR}/warmup-${backend}-gui-${warmup}.err" &
        process_id=$!
        sleep "${BENCH_GUI_DURATION_SECONDS}"
        kill -TERM "${process_id}" 2>/dev/null || true
        wait "${process_id}" 2>/dev/null || true
    done
}

bench_measure() {
    local measure_mode=$1
    local backend=$2
    local binary=$3
    local run=$4
    local result_file="${BENCH_TEMP_DIR}/result-${backend}-${measure_mode}-${run}.json"
    local stdout_file="${BENCH_TEMP_DIR}/${backend}-${measure_mode}-${run}.out"
    local stderr_file="${BENCH_TEMP_DIR}/${backend}-${measure_mode}-${run}.err"
    local timestamp start_ns measurement_end_ns end_ns process_id exit_status
    local elapsed_ms process_lifetime_ms result_count result_total_bytes requested_duration_ms
    local cpu_user_ms cpu_kernel_ms cpu_total_ms cpu_one_core cpu_machine
    local time_user_seconds time_kernel_seconds time_peak_rss
    local peak_rss_bytes peak_virtual_bytes binary_bytes stderr

    timestamp=$(date -u +'%Y-%m-%dT%H:%M:%SZ')
    start_ns=$(bench_now_ns)
    bench_reset_metrics
    bench_start_process \
        "${measure_mode}" \
        "${binary}" \
        "${result_file}" \
        "${stdout_file}" \
        "${stderr_file}"
    process_id=${BENCH_PROCESS_ID}

    if [[ "${measure_mode}" == "gui" ]]; then
        bench_monitor_for_duration "${process_id}" "${start_ns}"
        measurement_end_ns=$(bench_now_ns)
        bench_stop_process_tree "${process_id}"
    else
        bench_monitor_until_exit "${process_id}" "${start_ns}"
        measurement_end_ns=$(bench_now_ns)
        if [[ "${BENCH_TIMED_OUT}" == true ]]; then
            bench_stop_process_tree "${process_id}"
            BENCH_STOP_METHOD="timeout"
        fi
    fi

    if wait "${process_id}" 2>/dev/null; then
        exit_status=0
    else
        exit_status=$?
    fi
    end_ns=$(bench_now_ns)
    stderr=$(<"${stderr_file}")

    if [[ "${BENCH_TIMED_OUT}" == true ]]; then
        bench_fail "${backend} ${measure_mode} exceeded ${BENCH_TIMEOUT_SECONDS}s"
    fi
    if [[ "${measure_mode}" != "gui" && ${exit_status} -ne 0 ]]; then
        bench_fail \
            "${backend} ${measure_mode} failed with status ${exit_status}: ${stderr}"
    fi

    elapsed_ms=$(( (measurement_end_ns - start_ns) / 1000000 ))
    process_lifetime_ms=$(( (end_ns - start_ns) / 1000000 ))
    time_user_seconds=$(awk '/ real / { print $3; exit }' "${stderr_file}")
    time_kernel_seconds=$(awk '/ real / { print $5; exit }' "${stderr_file}")
    time_peak_rss=$(awk '/maximum resident set size/ { print $1; exit }' "${stderr_file}")
    cpu_user_ms=$(awk -v seconds="${time_user_seconds:-0}" 'BEGIN { printf "%.3f", seconds * 1000 }')
    cpu_kernel_ms=$(awk -v seconds="${time_kernel_seconds:-0}" 'BEGIN { printf "%.3f", seconds * 1000 }')
    cpu_total_ms=$(awk -v user="${cpu_user_ms}" -v kernel="${cpu_kernel_ms}" \
        'BEGIN { printf "%.3f", user + kernel }')
    cpu_one_core=$(awk -v cpu="${cpu_total_ms}" -v elapsed="${elapsed_ms}" \
        'BEGIN {
            if (elapsed > 0) printf "%.3f", cpu * 100 / elapsed
            else print "0.000"
        }')
    cpu_machine=$(awk -v cpu="${cpu_one_core}" -v processors="${BENCH_LOGICAL_PROCESSORS}" \
        'BEGIN { printf "%.3f", cpu / processors }')
    if [[ "${time_peak_rss:-0}" =~ ^[0-9]+$ && ${time_peak_rss:-0} -gt $((BENCH_PEAK_RSS_KIB * 1024)) ]]; then
        peak_rss_bytes=${time_peak_rss}
    else
        peak_rss_bytes=$((BENCH_PEAK_RSS_KIB * 1024))
    fi
    peak_virtual_bytes=$((BENCH_PEAK_VIRTUAL_KIB * 1024))
    binary_bytes=$(stat -f '%z' "${binary}")
    result_count=""
    result_total_bytes=""
    requested_duration_ms=""

    if [[ "${measure_mode}" == "scan" ]]; then
        [[ -f "${result_file}" ]] ||
            bench_fail "${backend} scan did not create ${result_file}"
        read -r result_count result_total_bytes < <(
            /usr/bin/osascript -l JavaScript "${BENCH_RESULTS_HELPER}" summarize "${result_file}"
        )
        if [[ "${run}" == 1 && "${backend}" == "ignore" ]]; then
            BENCH_FIRST_IGNORE_RESULT=${result_file}
        elif [[ "${run}" == 1 && "${backend}" == "spotlight" ]]; then
            BENCH_FIRST_SPOTLIGHT_RESULT=${result_file}
        fi
    elif [[ "${measure_mode}" == "gui" ]]; then
        requested_duration_ms=$(awk -v seconds="${BENCH_GUI_DURATION_SECONDS}" \
            'BEGIN { printf "%.3f", seconds * 1000 }')
    fi

    printf '%s,%s,%s,%d,%d,%d,%s,%s,%s,%s,%s,%d,%d,%d,%d,%d,%d,%s,%s,%d,%d,%d,%s,%s\n' \
        "${timestamp}" \
        "${measure_mode}" \
        "${backend}" \
        "${run}" \
        "${elapsed_ms}" \
        "${process_lifetime_ms}" \
        "${cpu_total_ms}" \
        "${cpu_user_ms}" \
        "${cpu_kernel_ms}" \
        "${cpu_one_core}" \
        "${cpu_machine}" \
        "${peak_rss_bytes}" \
        "${peak_virtual_bytes}" \
        0 \
        "${BENCH_PEAK_THREADS}" \
        "${BENCH_SAMPLE_COUNT}" \
        "${exit_status}" \
        "${BENCH_STOP_METHOD}" \
        "${result_count}" \
        "${binary_bytes}" \
        "${BENCH_LOGICAL_PROCESSORS}" \
        "${BENCH_SAMPLE_INTERVAL_MS}" \
        "${requested_duration_ms}" \
        "${result_total_bytes}" >>"${BENCH_OUTPUT}"

    printf '%s %s run %d: %d ms, %.2f MiB peak RSS, %.2f MiB virtual' \
        "${backend}" \
        "${measure_mode}" \
        "${run}" \
        "${elapsed_ms}" \
        "$(awk -v bytes="${peak_rss_bytes}" 'BEGIN { print bytes / 1048576 }')" \
        "$(awk -v bytes="${peak_virtual_bytes}" 'BEGIN { print bytes / 1048576 }')"
    [[ -z "${result_count}" ]] || printf ', %s results (%s bytes)' "${result_count}" "${result_total_bytes}"
    printf '\n'
}

mkdir -p -- "$(dirname -- "${BENCH_OUTPUT}")"
printf '%s\n' \
    'timestamp_utc,mode,backend,run,elapsed_ms,process_lifetime_ms,cpu_total_ms,cpu_user_ms,cpu_kernel_ms,cpu_percent_one_core,cpu_percent_machine,peak_rss_bytes,peak_virtual_bytes,peak_fds,peak_threads,samples,exit_status,stop_method,result_count,binary_bytes,logical_processors,sample_interval_ms,requested_gui_duration_ms,result_total_bytes' \
    >"${BENCH_OUTPUT}"

for backend in ignore spotlight; do
    if [[ "${backend}" == "ignore" ]]; then
        binary=${BENCH_IGNORE_BINARY}
    else
        binary=${BENCH_SPOTLIGHT_BINARY}
    fi

    if [[ "${BENCH_MODE}" == "all" || "${BENCH_MODE}" == "scan" ]]; then
        bench_warmup_scan "${backend}" "${binary}"
        for ((run = 1; run <= BENCH_SCAN_RUNS; run++)); do
            bench_measure "scan" "${backend}" "${binary}" "${run}"
        done
    fi

    if [[ "${BENCH_MODE}" == "all" || "${BENCH_MODE}" == "gui" ]]; then
        bench_warmup_gui "${backend}" "${binary}"
        for ((run = 1; run <= BENCH_GUI_RUNS; run++)); do
            bench_measure "gui-startup" "${backend}" "${binary}" "${run}"
            bench_measure "gui" "${backend}" "${binary}" "${run}"
        done
    fi
done

if [[ -n "${BENCH_FIRST_IGNORE_RESULT}" && -n "${BENCH_FIRST_SPOTLIGHT_RESULT}" ]]; then
    comparison_dir=$(dirname -- "${BENCH_OUTPUT}")
    /usr/bin/osascript -l JavaScript "${BENCH_RESULTS_HELPER}" compare \
        "${BENCH_FIRST_IGNORE_RESULT}" \
        "${BENCH_FIRST_SPOTLIGHT_RESULT}" \
        "${comparison_dir}"
    printf 'Result-set comparison: %s/{ignore-only,spotlight-only,intersection}.txt\n' \
        "${comparison_dir}"
fi

awk -F, '
    NR > 1 {
        key = $3 SUBSEP $2
        if (!(key in seen)) {
            seen[key] = 1
            order[++groups] = key
        }
        runs[key]++
        elapsed[key] += $5
        rss[key] += $12
        if (runs[key] == 1 || $5 < elapsed_min[key]) elapsed_min[key] = $5
        if (runs[key] == 1 || $5 > elapsed_max[key]) elapsed_max[key] = $5
        if (runs[key] == 1 || $12 < rss_min[key]) rss_min[key] = $12
        if (runs[key] == 1 || $12 > rss_max[key]) rss_max[key] = $12
    }
    END {
        print "\nSummary:"
        for (group_index = 1; group_index <= groups; group_index++) {
            key = order[group_index]
            split(key, parts, SUBSEP)
            printf "  %s, %s: elapsed %.1f ms mean (%d-%d); peak RSS %.2f MiB mean (%.2f-%.2f)\n",
                parts[1], parts[2],
                elapsed[key] / runs[key], elapsed_min[key], elapsed_max[key],
                rss[key] / runs[key] / 1048576,
                rss_min[key] / 1048576, rss_max[key] / 1048576
        }
    }
' "${BENCH_OUTPUT}"

printf 'Raw results: %s\n' "${BENCH_OUTPUT}"
