#!/bin/bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "benchmark-macos.sh must run on macOS" >&2
  exit 1
fi

iterations="${BENCHMARK_ITERATIONS:-5}"
case "${iterations}" in
  ''|*[!0-9]*) echo "BENCHMARK_ITERATIONS must be a positive integer" >&2; exit 1 ;;
esac
if (( iterations < 1 )); then
  echo "BENCHMARK_ITERATIONS must be a positive integer" >&2
  exit 1
fi

project_dir="$(cd "$(dirname "$0")" && pwd)"
output_dir="${BENCHMARK_OUTPUT_DIR:-${project_dir}/benchmark-results-macos}"
binary_dir="${project_dir}/target/release/benchmark-binaries"
mkdir -p "${output_dir}" "${binary_dir}"

build_backend() {
  local backend="$1"
  if [[ "${backend}" == "spotlight" ]]; then
    cargo build --locked --release --no-default-features --features spotlight
  else
    cargo build --locked --release
  fi
  cp "${project_dir}/target/release/cefdetector" "${binary_dir}/cefdetector-${backend}"
}

for backend in ignore spotlight; do
  build_backend "${backend}"
done

csv="${output_dir}/runs.csv"
printf 'backend,iteration,wall_seconds,user_seconds,system_seconds,max_rss_bytes,result_count,total_size,exit_status\n' > "${csv}"

run_once() {
  local backend="$1"
  local iteration="$2"
  local measured="$3"
  local result_file="${output_dir}/${backend}-${iteration}.json"
  local timing_file="${output_dir}/${backend}-${iteration}.time"
  local status wall user system rss count total
  set +e
  /usr/bin/time -l "${binary_dir}/cefdetector-${backend}" --json > "${result_file}" 2> "${timing_file}"
  status=$?
  set -e
  wall="$(awk '/ real / { print $1; exit }' "${timing_file}")"
  user="$(awk '/ real / { print $3; exit }' "${timing_file}")"
  system="$(awk '/ real / { print $5; exit }' "${timing_file}")"
  rss="$(awk '/maximum resident set size/ { print $1; exit }' "${timing_file}")"
  read -r count total < <(/usr/bin/osascript -l JavaScript \
    "${project_dir}/.github/scripts/benchmark-macos-results.js" summarize "${result_file}")
  if [[ "${measured}" == "yes" ]]; then
    printf '%s,%s,%s,%s,%s,%s,%s,%s,%s\n' \
      "${backend}" "${iteration}" "${wall}" "${user:-}" "${system:-}" \
      "${rss:-}" "${count}" "${total}" "${status}" >> "${csv}"
  fi
  return "${status}"
}

for backend in ignore spotlight; do
  echo "Warming up ${backend}..."
  run_once "${backend}" warmup no
  for ((iteration = 1; iteration <= iterations; iteration++)); do
    echo "Measuring ${backend} (${iteration}/${iterations})..."
    run_once "${backend}" "${iteration}" yes
  done
done

/usr/bin/osascript -l JavaScript \
  "${project_dir}/.github/scripts/benchmark-macos-results.js" compare \
  "${output_dir}/ignore-1.json" "${output_dir}/spotlight-1.json" "${output_dir}"

echo "Benchmark results written to ${output_dir}"
