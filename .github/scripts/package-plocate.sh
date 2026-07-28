#!/usr/bin/env bash

set -euo pipefail

PLOCATE_VERSION=${1:-}
PLOCATE_OUTPUT_DIR=${PACKAGER_OUTPUT_DIR:-target/release/packager}
PLOCATE_TARGET_DIR=${PLOCATE_CARGO_TARGET_DIR:-target/plocate}

if [[ ! "${PLOCATE_VERSION}" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$ ]]; then
    echo "Usage: $0 <version>" >&2
    exit 2
fi

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
PROJECT_DIR=$(cd -- "${SCRIPT_DIR}/../.." && pwd)
cd "${PROJECT_DIR}"

PACKAGE_VERSION=$(cargo pkgid --locked | sed -E 's/.*@//')
if [[ "${PACKAGE_VERSION}" != "${PLOCATE_VERSION}" ]]; then
    echo "Cargo version ${PACKAGE_VERSION} does not match ${PLOCATE_VERSION}" >&2
    exit 1
fi

PLOCATE_DEFAULT_ARCHIVE="${PLOCATE_OUTPUT_DIR}/cefdetector_${PLOCATE_VERSION}_x86_64.tar.gz"
PLOCATE_ARCHIVE="${PLOCATE_OUTPUT_DIR}/cefdetector-plocate_${PLOCATE_VERSION}_x86_64.tar.gz"
PLOCATE_ARCHIVE_TEMP="${PLOCATE_ARCHIVE}.tmp"

if [[ ! -f "${PLOCATE_DEFAULT_ARCHIVE}" ]]; then
    echo "Default package archive not found: ${PLOCATE_DEFAULT_ARCHIVE}" >&2
    exit 1
fi

PLOCATE_WORK_DIR=$(mktemp -d "${RUNNER_TEMP:-/tmp}/cefdetector-plocate.XXXXXX")

plocate_cleanup() {
    rm -f -- "${PLOCATE_ARCHIVE_TEMP}"
    rm -rf -- "${PLOCATE_WORK_DIR}"
}
trap plocate_cleanup EXIT

echo "Building the plocate backend..."
CARGO_TARGET_DIR="${PLOCATE_TARGET_DIR}" \
    cargo build --locked --release --no-default-features --features plocate

tar -xzf "${PLOCATE_DEFAULT_ARCHIVE}" -C "${PLOCATE_WORK_DIR}"
install -Dm755 \
    "${PLOCATE_TARGET_DIR}/release/cefdetector" \
    "${PLOCATE_WORK_DIR}/usr/bin/cefdetector"

if [[ -z "${SOURCE_DATE_EPOCH:-}" ]]; then
    SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
fi
if [[ ! "${SOURCE_DATE_EPOCH}" =~ ^[0-9]+$ ]]; then
    echo "SOURCE_DATE_EPOCH must be a Unix timestamp" >&2
    exit 1
fi

tar \
    --sort=name \
    --mtime="@${SOURCE_DATE_EPOCH}" \
    --owner=0 \
    --group=0 \
    --numeric-owner \
    -czf "${PLOCATE_ARCHIVE_TEMP}" \
    -C "${PLOCATE_WORK_DIR}" \
    usr
mv -- "${PLOCATE_ARCHIVE_TEMP}" "${PLOCATE_ARCHIVE}"

echo "Created ${PLOCATE_ARCHIVE}"
