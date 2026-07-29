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
BENCH_OUTPUT="benchmark-linux.csv"
BENCH_BUILD=true

BENCH_ARTIFACT_DIR="${BENCH_SCRIPT_DIR}/target/release/benchmark-linux"
BENCH_IGNORE_BINARY="${BENCH_ARTIFACT_DIR}/cefdetector-ignore"
BENCH_PLOCATE_BINARY="${BENCH_ARTIFACT_DIR}/cefdetector-plocate"
BENCH_TEMP_DIR=""

BENCH_CURRENT_RSS_KIB=0
BENCH_CURRENT_PRIVATE_KIB=0
BENCH_CURRENT_FDS=0
BENCH_CURRENT_THREADS=0
BENCH_CURRENT_USER_TICKS=0
BENCH_CURRENT_KERNEL_TICKS=0
BENCH_PEAK_RSS_KIB=0
BENCH_PEAK_PRIVATE_KIB=0
BENCH_PEAK_FDS=0
BENCH_PEAK_THREADS=0
BENCH_PEAK_USER_TICKS=0
BENCH_PEAK_KERNEL_TICKS=0
BENCH_SAMPLE_COUNT=0
BENCH_TIMED_OUT=false
BENCH_STOP_METHOD="process-exit"
BENCH_PROCESS_ID=""
BENCH_SAMPLE_INTERVAL_SECONDS=""
BENCH_GUI_DURATION_NS=0
BENCH_TIMEOUT_NS=0
BENCH_CLOCK_TICKS=0
BENCH_LOGICAL_PROCESSORS=0
BENCH_TREE_PIDS=()

bench_usage() {
    cat <<'EOF'
Usage: ./benchmark.sh [all|scan|gui] [OPTIONS]

Benchmarks both Linux search backends. The plocate command and an up-to-date
database are required for plocate scan measurements.

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
      --sample-interval-ms N   /proc sampling interval in ms (default: 10)
  -i, --interval SEC           Compatibility alias using seconds
      --timeout SEC            Scan/startup timeout (default: 300)
  -o, --output FILE            CSV output (default: benchmark-linux.csv)
      --no-build               Reuse backend binaries built by this script
  -h, --help                   Show this help

Examples:
  ./benchmark.sh
  ./benchmark.sh scan --scan-runs 10 --scan-warmup-runs 2
  ./benchmark.sh gui --duration 10 --output gui-linux.csv
  ./benchmark.sh all --no-build
EOF
}

bench_fail() {
    printf 'benchmark: %s\n' "$*" >&2
    exit 2
}

bench_cleanup() {
    if [[ -n "${BENCH_TEMP_DIR}" && -d "${BENCH_TEMP_DIR}" ]]; then
        rm -rf -- "${BENCH_TEMP_DIR}"
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

for command in awk date env getconf install mktemp sleep stat; do
    bench_require_command "${command}"
done
if [[ "${BENCH_BUILD}" == true ]]; then
    bench_require_command cargo
fi

[[ -r /proc/self/status ]] ||
    bench_fail "Linux /proc is required for process sampling"
if [[ "${BENCH_MODE}" != "scan" &&
    -z "${DISPLAY:-}" &&
    -z "${WAYLAND_DISPLAY:-}" ]]; then
    bench_fail "GUI measurements require DISPLAY or WAYLAND_DISPLAY"
fi

BENCH_SAMPLE_INTERVAL_SECONDS=$(
    awk -v milliseconds="${BENCH_SAMPLE_INTERVAL_MS}" \
        'BEGIN { printf "%.6f", milliseconds / 1000 }'
)
BENCH_GUI_DURATION_NS=$(
    awk -v seconds="${BENCH_GUI_DURATION_SECONDS}" \
        'BEGIN { printf "%.0f", seconds * 1000000000 }'
)
BENCH_TIMEOUT_NS=$((10#${BENCH_TIMEOUT_SECONDS} * 1000000000))
BENCH_CLOCK_TICKS=$(getconf CLK_TCK)
BENCH_LOGICAL_PROCESSORS=$(getconf _NPROCESSORS_ONLN)
bench_positive_integer "${BENCH_CLOCK_TICKS}" ||
    bench_fail "getconf returned an invalid CLK_TCK value"
bench_positive_integer "${BENCH_LOGICAL_PROCESSORS}" ||
    bench_fail "getconf returned an invalid processor count"

bench_build_binaries() {
    mkdir -p -- "${BENCH_ARTIFACT_DIR}"

    printf 'Building locked release binary for the ignore backend...\n'
    cargo build --locked --release
    install -m755 target/release/cefdetector "${BENCH_IGNORE_BINARY}"

    printf 'Building locked release binary for the plocate backend...\n'
    cargo build --locked --release --no-default-features --features plocate
    install -m755 target/release/cefdetector "${BENCH_PLOCATE_BINARY}"
}

if [[ "${BENCH_BUILD}" == true ]]; then
    bench_build_binaries
fi
for binary in "${BENCH_IGNORE_BINARY}" "${BENCH_PLOCATE_BINARY}"; do
    [[ -x "${binary}" ]] ||
        bench_fail "benchmark binary not found: ${binary} (run without --no-build)"
done

BENCH_TEMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/cefdetector-benchmark.XXXXXXXX")

bench_now_ns() {
    date +%s%N
}

bench_collect_process_tree() {
    local root_pid=$1
    local process_id children_file
    local -a pending=("${root_pid}")
    local -a children=()
    local -A seen=()
    BENCH_TREE_PIDS=()

    while ((${#pending[@]} > 0)); do
        process_id=${pending[0]}
        pending=("${pending[@]:1}")
        [[ "${process_id}" =~ ^[0-9]+$ ]] || continue
        [[ -z "${seen[${process_id}]:-}" ]] || continue
        seen["${process_id}"]=1
        [[ -r "/proc/${process_id}/stat" ]] || continue
        BENCH_TREE_PIDS+=("${process_id}")

        children_file="/proc/${process_id}/task/${process_id}/children"
        children=()
        if [[ -r "${children_file}" ]]; then
            read -r -a children <"${children_file}" 2>/dev/null || true
            ((${#children[@]} == 0)) || pending+=("${children[@]}")
        fi
    done
}

bench_process_alive() {
    local process_id=$1
    local stat_line stat_fields
    [[ -r "/proc/${process_id}/stat" ]] || return 1
    IFS= read -r stat_line <"/proc/${process_id}/stat" 2>/dev/null || return 1
    stat_fields=${stat_line##*) }
    [[ "${stat_fields%% *}" != "Z" ]]
}

bench_sample_process_tree() {
    local root_pid=$1
    local process_id status_file smaps_file stat_file key value
    local status_fd smaps_fd stat_fd
    local rss_kib private_kib threads fds user_ticks kernel_ticks
    local stat_line stat_fields
    local -a fields=()
    local -a fd_entries=()

    BENCH_CURRENT_RSS_KIB=0
    BENCH_CURRENT_PRIVATE_KIB=0
    BENCH_CURRENT_FDS=0
    BENCH_CURRENT_THREADS=0
    BENCH_CURRENT_USER_TICKS=0
    BENCH_CURRENT_KERNEL_TICKS=0

    bench_collect_process_tree "${root_pid}"
    for process_id in "${BENCH_TREE_PIDS[@]}"; do
        status_file="/proc/${process_id}/status"
        smaps_file="/proc/${process_id}/smaps_rollup"
        stat_file="/proc/${process_id}/stat"
        rss_kib=0
        private_kib=0
        threads=0
        fds=0
        user_ticks=0
        kernel_ticks=0

        if { exec {status_fd}<"${status_file}"; } 2>/dev/null; then
            while read -r key value _ 2>/dev/null; do
                case "${key}" in
                    VmRSS:)
                        rss_kib=${value:-0}
                        ;;
                    Threads:)
                        threads=${value:-0}
                        ;;
                esac
            done <&"${status_fd}"
            exec {status_fd}<&-
        fi

        if { exec {smaps_fd}<"${smaps_file}"; } 2>/dev/null; then
            while read -r key value _ 2>/dev/null; do
                case "${key}" in
                    Private_Clean: | Private_Dirty: | Private_Hugetlb:)
                        private_kib=$((private_kib + ${value:-0}))
                        ;;
                esac
            done <&"${smaps_fd}"
            exec {smaps_fd}<&-
        fi

        fd_entries=()
        if [[ -d "/proc/${process_id}/fd" ]]; then
            shopt -s nullglob
            fd_entries=("/proc/${process_id}/fd"/*)
            shopt -u nullglob
            fds=${#fd_entries[@]}
        fi

        if { exec {stat_fd}<"${stat_file}"; } 2>/dev/null; then
            IFS= read -r stat_line <&"${stat_fd}" 2>/dev/null || stat_line=""
            exec {stat_fd}<&-
            if [[ -n "${stat_line}" ]]; then
                stat_fields=${stat_line##*) }
                read -r -a fields <<<"${stat_fields}"
                user_ticks=${fields[11]:-0}
                kernel_ticks=${fields[12]:-0}
            fi
        fi

        BENCH_CURRENT_RSS_KIB=$((BENCH_CURRENT_RSS_KIB + rss_kib))
        BENCH_CURRENT_PRIVATE_KIB=$((BENCH_CURRENT_PRIVATE_KIB + private_kib))
        BENCH_CURRENT_FDS=$((BENCH_CURRENT_FDS + fds))
        BENCH_CURRENT_THREADS=$((BENCH_CURRENT_THREADS + threads))
        BENCH_CURRENT_USER_TICKS=$((BENCH_CURRENT_USER_TICKS + user_ticks))
        BENCH_CURRENT_KERNEL_TICKS=$((BENCH_CURRENT_KERNEL_TICKS + kernel_ticks))
    done

    ((BENCH_CURRENT_RSS_KIB <= BENCH_PEAK_RSS_KIB)) ||
        BENCH_PEAK_RSS_KIB=${BENCH_CURRENT_RSS_KIB}
    ((BENCH_CURRENT_PRIVATE_KIB <= BENCH_PEAK_PRIVATE_KIB)) ||
        BENCH_PEAK_PRIVATE_KIB=${BENCH_CURRENT_PRIVATE_KIB}
    ((BENCH_CURRENT_FDS <= BENCH_PEAK_FDS)) ||
        BENCH_PEAK_FDS=${BENCH_CURRENT_FDS}
    ((BENCH_CURRENT_THREADS <= BENCH_PEAK_THREADS)) ||
        BENCH_PEAK_THREADS=${BENCH_CURRENT_THREADS}
    ((BENCH_CURRENT_USER_TICKS <= BENCH_PEAK_USER_TICKS)) ||
        BENCH_PEAK_USER_TICKS=${BENCH_CURRENT_USER_TICKS}
    ((BENCH_CURRENT_KERNEL_TICKS <= BENCH_PEAK_KERNEL_TICKS)) ||
        BENCH_PEAK_KERNEL_TICKS=${BENCH_CURRENT_KERNEL_TICKS}
    BENCH_SAMPLE_COUNT=$((BENCH_SAMPLE_COUNT + 1))
}

bench_reset_metrics() {
    BENCH_PEAK_RSS_KIB=0
    BENCH_PEAK_PRIVATE_KIB=0
    BENCH_PEAK_FDS=0
    BENCH_PEAK_THREADS=0
    BENCH_PEAK_USER_TICKS=0
    BENCH_PEAK_KERNEL_TICKS=0
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
    for ((index = ${#BENCH_TREE_PIDS[@]} - 1; index >= 0; index--)); do
        process_id=${BENCH_TREE_PIDS[index]}
        kill -TERM "${process_id}" 2>/dev/null || true
    done

    for ((attempt = 0; attempt < 50; attempt++)); do
        bench_process_alive "${root_pid}" || return 0
        sleep 0.02
    done

    BENCH_STOP_METHOD="signal-kill"
    bench_collect_process_tree "${root_pid}"
    for ((index = ${#BENCH_TREE_PIDS[@]} - 1; index >= 0; index--)); do
        process_id=${BENCH_TREE_PIDS[index]}
        kill -KILL "${process_id}" 2>/dev/null || true
    done
    return 0
}

bench_start_process() {
    local measure_mode=$1
    local binary=$2
    local result_file=$3
    local stdout_file=$4
    local stderr_file=$5
    local -a arguments=()

    if [[ "${measure_mode}" == "scan" ]]; then
        arguments=(--json --output "${result_file}")
    fi

    if [[ "${measure_mode}" == "gui-startup" ]]; then
        CEFDETECTOR_GUI_SMOKE_TEST=1 \
            "${binary}" "${arguments[@]}" >"${stdout_file}" 2>"${stderr_file}" &
    else
        env -u CEFDETECTOR_GUI_SMOKE_TEST \
            "${binary}" "${arguments[@]}" >"${stdout_file}" 2>"${stderr_file}" &
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
        bench_stop_process_tree "${process_id}"
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
    local elapsed_ms process_lifetime_ms result_count requested_duration_ms
    local cpu_user_ms cpu_kernel_ms cpu_total_ms cpu_one_core cpu_machine
    local peak_rss_bytes peak_private_bytes binary_bytes stderr

    timestamp=$(date -u +'%Y-%m-%dT%H:%M:%S.%NZ')
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
            BENCH_STOP_METHOD="timeout"
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
    cpu_user_ms=$(
        awk -v ticks="${BENCH_PEAK_USER_TICKS}" -v hz="${BENCH_CLOCK_TICKS}" \
            'BEGIN { printf "%.3f", ticks * 1000 / hz }'
    )
    cpu_kernel_ms=$(
        awk -v ticks="${BENCH_PEAK_KERNEL_TICKS}" -v hz="${BENCH_CLOCK_TICKS}" \
            'BEGIN { printf "%.3f", ticks * 1000 / hz }'
    )
    cpu_total_ms=$(
        awk -v user="${cpu_user_ms}" -v kernel="${cpu_kernel_ms}" \
            'BEGIN { printf "%.3f", user + kernel }'
    )
    cpu_one_core=$(
        awk -v cpu="${cpu_total_ms}" -v elapsed="${elapsed_ms}" \
            'BEGIN {
                if (elapsed > 0) printf "%.3f", cpu * 100 / elapsed
                else print "0.000"
            }'
    )
    cpu_machine=$(
        awk -v cpu="${cpu_one_core}" -v processors="${BENCH_LOGICAL_PROCESSORS}" \
            'BEGIN { printf "%.3f", cpu / processors }'
    )
    peak_rss_bytes=$((BENCH_PEAK_RSS_KIB * 1024))
    peak_private_bytes=$((BENCH_PEAK_PRIVATE_KIB * 1024))
    binary_bytes=$(stat -c '%s' "${binary}")
    result_count=""
    requested_duration_ms=""

    if [[ "${measure_mode}" == "scan" ]]; then
        [[ -f "${result_file}" ]] ||
            bench_fail "${backend} scan did not create ${result_file}"
        result_count=$(
            awk '/"file":[[:space:]]/{ count++ } END { print count + 0 }' \
                "${result_file}"
        )
    elif [[ "${measure_mode}" == "gui" ]]; then
        requested_duration_ms=$(
            awk -v seconds="${BENCH_GUI_DURATION_SECONDS}" \
                'BEGIN { printf "%.3f", seconds * 1000 }'
        )
    fi

    printf '%s,%s,%s,%d,%d,%d,%s,%s,%s,%s,%s,%d,%d,%d,%d,%d,%d,%s,%s,%d,%d,%d,%s\n' \
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
        "${peak_private_bytes}" \
        "${BENCH_PEAK_FDS}" \
        "${BENCH_PEAK_THREADS}" \
        "${BENCH_SAMPLE_COUNT}" \
        "${exit_status}" \
        "${BENCH_STOP_METHOD}" \
        "${result_count}" \
        "${binary_bytes}" \
        "${BENCH_LOGICAL_PROCESSORS}" \
        "${BENCH_SAMPLE_INTERVAL_MS}" \
        "${requested_duration_ms}" >>"${BENCH_OUTPUT}"

    printf '%s %s run %d: %d ms, %.2f MiB peak RSS, %.2f MiB private' \
        "${backend}" \
        "${measure_mode}" \
        "${run}" \
        "${elapsed_ms}" \
        "$(awk -v bytes="${peak_rss_bytes}" 'BEGIN { print bytes / 1048576 }')" \
        "$(awk -v bytes="${peak_private_bytes}" 'BEGIN { print bytes / 1048576 }')"
    [[ -z "${result_count}" ]] || printf ', %s results' "${result_count}"
    printf '\n'
}

mkdir -p -- "$(dirname -- "${BENCH_OUTPUT}")"
printf '%s\n' \
    'timestamp_utc,mode,backend,run,elapsed_ms,process_lifetime_ms,cpu_total_ms,cpu_user_ms,cpu_kernel_ms,cpu_percent_one_core,cpu_percent_machine,peak_rss_bytes,peak_private_bytes,peak_fds,peak_threads,samples,exit_status,stop_method,result_count,binary_bytes,logical_processors,sample_interval_ms,requested_gui_duration_ms' \
    >"${BENCH_OUTPUT}"

declare -A BENCH_BACKENDS=(
    [ignore]="${BENCH_IGNORE_BINARY}"
    [plocate]="${BENCH_PLOCATE_BINARY}"
)

for backend in ignore plocate; do
    binary=${BENCH_BACKENDS[${backend}]}

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
