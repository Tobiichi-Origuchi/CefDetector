#!/bin/bash
set -euo pipefail

version="${1:?usage: package-macos.sh VERSION}"
if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  echo "Release macOS packages must be built on an arm64 macOS host" >&2
  exit 1
fi
rust_host="$(
  rustc -vV |
    awk '$1 == "host:" { print $2 }'
)"
if [[ "${rust_host}" != "aarch64-apple-darwin" ]]; then
  echo "Release Rust host must be aarch64-apple-darwin" >&2
  exit 1
fi

project_dir="$(cd "$(dirname "$0")/../.." && pwd)"
output_dir="${PACKAGE_OUTPUT_DIR:-${project_dir}/dist}"
binary_dir="${project_dir}/target/release/package-binaries-macos"
mkdir -p "${output_dir}" "${binary_dir}"

cargo build --locked --release
binary="${binary_dir}/cefdetector"
cp "${project_dir}/target/release/cefdetector" "${binary}"
file_description="$(file "${binary}")"
lipo_description="$(lipo -info "${binary}")"
printf '%s\n' "${file_description}" "${lipo_description}"
if [[ "${file_description}" != *arm64* ]]; then
  echo "Packaged binary is not arm64: ${binary}" >&2
  exit 1
fi
if [[ "${lipo_description}" == *x86_64* ]]; then
  echo "Packaged binary contains a forbidden x86_64 slice: ${binary}" >&2
  exit 1
fi

staging="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/cefdetector-package"
bundle="${staging}/CefDetector.app"
rm -rf "${staging}"
mkdir -p "${bundle}/Contents/MacOS" "${bundle}/Contents/Resources"
sed "s/__VERSION__/${version}/g" "${project_dir}/packaging/macos/Info.plist" > "${bundle}/Contents/Info.plist"
cp "${binary}" "${bundle}/Contents/MacOS/cefdetector"
cp "${project_dir}/icons/icon.icns" "${bundle}/Contents/Resources/icon.icns"
cp "${project_dir}/fonts/OFL-Inter.txt" "${bundle}/Contents/Resources/OFL-Inter.txt"
cp "${project_dir}/fonts/NOTICE.txt" "${bundle}/Contents/Resources/FONT-NOTICE.txt"
chmod 755 "${bundle}/Contents/MacOS/cefdetector"
codesign --force --deep --sign - "${bundle}"
codesign --verify --deep --strict --verbose=2 "${bundle}"
codesign_description="$(codesign --display --verbose=4 "${bundle}" 2>&1)"
printf '%s\n' "${codesign_description}"
if ! grep -Fxq 'Signature=adhoc' <<<"${codesign_description}"; then
  echo "Packaged application is not ad-hoc signed: ${bundle}" >&2
  exit 1
fi
archive="${output_dir}/cefdetector-${version}-macos-aarch64.zip"
rm -f "${archive}"
ditto -c -k --sequesterRsrc --keepParent "${bundle}" "${archive}"
