#!/bin/bash
set -euo pipefail

version="${1:?usage: package-macos-local.sh VERSION}"
if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Local macOS bundles must be built on macOS" >&2
  exit 1
fi

project_dir="$(cd "$(dirname "$0")/../.." && pwd)"
output_dir="${PACKAGE_OUTPUT_DIR:-${project_dir}/target/local-macos-packages}"
architecture="$(uname -m)"
mkdir -p "${output_dir}"

cargo build --locked --release
bundle="${output_dir}/CefDetector.app"
rm -rf "${bundle}"
mkdir -p "${bundle}/Contents/MacOS" "${bundle}/Contents/Resources"
sed "s/__VERSION__/${version}/g" "${project_dir}/packaging/macos/Info.plist" > "${bundle}/Contents/Info.plist"
cp "${project_dir}/target/release/cefdetector" "${bundle}/Contents/MacOS/cefdetector"
cp "${project_dir}/icons/icon.icns" "${bundle}/Contents/Resources/icon.icns"
chmod 755 "${bundle}/Contents/MacOS/cefdetector"
codesign --force --deep --sign - "${bundle}"
codesign --verify --deep --strict --verbose=2 "${bundle}"
echo "Built local ${architecture} bundle: ${bundle}"
