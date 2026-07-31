#!/bin/bash
set -euo pipefail

version="${1:?usage: package-macos.sh VERSION}"
if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  echo "Release macOS packages must be built on an arm64 macOS host" >&2
  exit 1
fi
if ! rustc -vV | grep -q '^host: aarch64-apple-darwin$'; then
  echo "Release Rust host must be aarch64-apple-darwin" >&2
  exit 1
fi

project_dir="$(cd "$(dirname "$0")/../.." && pwd)"
output_dir="${PACKAGE_OUTPUT_DIR:-${project_dir}/dist}"
binary_dir="${project_dir}/target/release/package-binaries-macos"
mkdir -p "${output_dir}" "${binary_dir}"

for backend in ignore spotlight; do
  if [[ "${backend}" == "spotlight" ]]; then
    cargo build --locked --release --no-default-features --features spotlight
  else
    cargo build --locked --release
  fi
  binary="${binary_dir}/cefdetector-${backend}"
  cp "${project_dir}/target/release/cefdetector" "${binary}"
  file "${binary}"
  lipo -info "${binary}"
  if ! file "${binary}" | grep -q 'arm64'; then
    echo "Packaged binary is not arm64: ${binary}" >&2
    exit 1
  fi
  if lipo -info "${binary}" | grep -q 'x86_64'; then
    echo "Packaged binary contains a forbidden x86_64 slice: ${binary}" >&2
    exit 1
  fi

  staging="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/cefdetector-package-${backend}"
  bundle="${staging}/CefDetector.app"
  rm -rf "${staging}"
  mkdir -p "${bundle}/Contents/MacOS" "${bundle}/Contents/Resources"
  sed "s/__VERSION__/${version}/g" "${project_dir}/packaging/macos/Info.plist" > "${bundle}/Contents/Info.plist"
  cp "${binary}" "${bundle}/Contents/MacOS/cefdetector"
  cp "${project_dir}/icons/icon.icns" "${bundle}/Contents/Resources/icon.icns"
  chmod 755 "${bundle}/Contents/MacOS/cefdetector"
  codesign --force --deep --sign - "${bundle}"
  codesign --verify --deep --strict --verbose=2 "${bundle}"
  codesign --display --verbose=4 "${bundle}" 2>&1 | grep -q '^Signature=adhoc$'
  archive="${output_dir}/cefdetector-${version}-macos-aarch64-${backend}.zip"
  rm -f "${archive}"
  ditto -c -k --sequesterRsrc --keepParent "${bundle}" "${archive}"
done
