#!/usr/bin/env bash

set -Eeuo pipefail

PACKAGE_VERSION=${1:-}
PACKAGE_OUTPUT_DIR=${PACKAGE_OUTPUT_DIR:-dist}
PACKAGE_BINARY_DIR=${PACKAGE_BINARY_DIR:-target/release/package-binaries}
PACKAGE_WORK_DIR=""
PACKAGE_TEMP_ARCHIVES=()

package_usage() {
    printf 'Usage: %s <version>\n' "$0" >&2
}

package_fail() {
    printf 'package-linux: %s\n' "$*" >&2
    exit 1
}

package_cleanup() {
    local archive

    for archive in "${PACKAGE_TEMP_ARCHIVES[@]}"; do
        [[ -z "${archive}" ]] || rm -f -- "${archive}"
    done
    if [[ -n "${PACKAGE_WORK_DIR}" && -d "${PACKAGE_WORK_DIR}" ]]; then
        rm -rf -- "${PACKAGE_WORK_DIR}"
    fi
}

trap package_cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [[ ! "${PACKAGE_VERSION}" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$ ]]; then
    package_usage
    exit 2
fi

for command in awk cargo git gzip install mktemp rustc tar uname; do
    command -v "${command}" >/dev/null 2>&1 ||
        package_fail "required command not found: ${command}"
done

[[ "$(uname -s)" == "Linux" && "$(uname -m)" == "x86_64" ]] ||
    package_fail "this script only creates Linux x86_64 packages"

PACKAGE_SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
readonly PACKAGE_SCRIPT_DIR
PACKAGE_PROJECT_DIR=$(cd -- "${PACKAGE_SCRIPT_DIR}/../.." && pwd)
readonly PACKAGE_PROJECT_DIR
cd "${PACKAGE_PROJECT_DIR}"

PACKAGE_CARGO_VERSION=$(cargo pkgid --locked)
PACKAGE_CARGO_VERSION=${PACKAGE_CARGO_VERSION##*@}
[[ "${PACKAGE_CARGO_VERSION}" == "${PACKAGE_VERSION}" ]] ||
    package_fail \
        "Cargo version ${PACKAGE_CARGO_VERSION} does not match ${PACKAGE_VERSION}"

PACKAGE_RUST_HOST=$(
    rustc -vV |
        awk '$1 == "host:" { print $2 }'
)
[[ "${PACKAGE_RUST_HOST}" == "x86_64-unknown-linux-gnu" ]] ||
    package_fail \
        "expected Rust host x86_64-unknown-linux-gnu, found ${PACKAGE_RUST_HOST:-unknown}"

if [[ -z "${SOURCE_DATE_EPOCH:-}" ]]; then
    SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
fi
[[ "${SOURCE_DATE_EPOCH}" =~ ^[0-9]+$ ]] ||
    package_fail "SOURCE_DATE_EPOCH must be a Unix timestamp"
export SOURCE_DATE_EPOCH

mkdir -p -- "${PACKAGE_OUTPUT_DIR}" "${PACKAGE_BINARY_DIR}"
PACKAGE_WORK_PARENT=${RUNNER_TEMP:-${TMPDIR:-/tmp}}
PACKAGE_WORK_DIR=$(
    mktemp -d "${PACKAGE_WORK_PARENT%/}/cefdetector-linux-package.XXXXXXXX"
)

package_stage_common_files() {
    local stage_dir=$1

    install -Dm644 \
        packaging/cefdetector.desktop \
        "${stage_dir}/usr/share/applications/cefdetector.desktop"
    install -Dm644 \
        icons/32x32.png \
        "${stage_dir}/usr/share/icons/hicolor/32x32/apps/cefdetector.png"
    install -Dm644 \
        icons/128x128.png \
        "${stage_dir}/usr/share/icons/hicolor/128x128/apps/cefdetector.png"
    install -Dm644 \
        icons/128x128@2x.png \
        "${stage_dir}/usr/share/icons/hicolor/256x256@2/apps/cefdetector.png"
    install -Dm644 \
        completions/cefdetector.bash \
        "${stage_dir}/usr/share/bash-completion/completions/cefdetector"
    install -Dm644 \
        completions/cefdetector.zsh \
        "${stage_dir}/usr/share/zsh/vendor-completions/_cefdetector"
    install -Dm644 \
        completions/cefdetector.fish \
        "${stage_dir}/usr/share/fish/vendor_completions.d/cefdetector.fish"
    install -Dm644 \
        LICENSE \
        "${stage_dir}/usr/share/licenses/cefdetector/LICENSE"
    install -Dm644 \
        fonts/OFL-Inter.txt \
        "${stage_dir}/usr/share/licenses/cefdetector/OFL-Inter.txt"
    install -Dm644 \
        README.md \
        "${stage_dir}/usr/share/doc/cefdetector/README.md"
    install -Dm644 \
        fonts/NOTICE.txt \
        "${stage_dir}/usr/share/doc/cefdetector/FONT-NOTICE.txt"
}

package_archive() {
    local stage_dir="${PACKAGE_WORK_DIR}/package"
    local archive_name
    local archive_path
    local temporary_archive

    printf 'Building the merged Linux backends...\n'
    cargo build --locked --release

    install -Dm755 \
        target/release/cefdetector \
        "${PACKAGE_BINARY_DIR}/cefdetector"
    install -Dm755 \
        target/release/cefdetector \
        "${stage_dir}/usr/bin/cefdetector"
    package_stage_common_files "${stage_dir}"

    archive_name="cefdetector-${PACKAGE_VERSION}-linux-x86_64.tar.gz"
    archive_path="${PACKAGE_OUTPUT_DIR}/${archive_name}"
    temporary_archive="${archive_path}.tmp"
    PACKAGE_TEMP_ARCHIVES+=("${temporary_archive}")

    tar \
        --format=gnu \
        --sort=name \
        --mtime="@${SOURCE_DATE_EPOCH}" \
        --owner=0 \
        --group=0 \
        --numeric-owner \
        -C "${stage_dir}" \
        -cf - \
        usr |
        gzip -n >"${temporary_archive}"
    tar -tzf "${temporary_archive}" >/dev/null
    mv -- "${temporary_archive}" "${archive_path}"
    printf 'Created %s\n' "${archive_path}"
}

umask 022
package_archive
