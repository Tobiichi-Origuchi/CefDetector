#!/usr/bin/env bash

set -euo pipefail

AUR_TAG=${1:-}
AUR_GITHUB_REPOSITORY=${GITHUB_REPOSITORY:-Tobiichi-Origuchi/CefDetectorLinux}
AUR_ASSET_DIR=${RELEASE_ASSET_DIR:-target/release/packager}
AUR_HOST="aur.archlinux.org"
AUR_EXPECTED_HOST_FINGERPRINT="SHA256:RFzBCUItH9LZS0cKB5UE6ceAYhBD5C8GeOBip8Z11+4"

if [[ ! "${AUR_TAG}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Usage: $0 <stable-v-prefixed-version>" >&2
    exit 2
fi
if [[ -z "${AUR_SSH_PRIVATE_KEY:-}" ]]; then
    echo "AUR_SSH_PRIVATE_KEY is required" >&2
    exit 2
fi

AUR_VERSION=${AUR_TAG#v}
AUR_PACKAGE_FILE="cefdetector_${AUR_VERSION}_x86_64.tar.gz"
AUR_LOCAL_PACKAGE="${AUR_ASSET_DIR}/${AUR_PACKAGE_FILE}"
AUR_PACKAGE_URL="https://github.com/${AUR_GITHUB_REPOSITORY}/releases/download/${AUR_TAG}/${AUR_PACKAGE_FILE}"

if [[ ! -f "${AUR_LOCAL_PACKAGE}" ]]; then
    echo "Release package not found: ${AUR_LOCAL_PACKAGE}" >&2
    exit 1
fi

AUR_WORK_DIR=$(mktemp -d "${RUNNER_TEMP:-/tmp}/cefdetector-aur.XXXXXX")
AUR_KEY_FILE="${AUR_WORK_DIR}/aur_key"
AUR_KNOWN_HOSTS="${AUR_WORK_DIR}/known_hosts"
AUR_REPOSITORY_DIR="${AUR_WORK_DIR}/cefdetector-bin"

aur_cleanup() {
    rm -f "${AUR_KEY_FILE}"
    rm -rf "${AUR_WORK_DIR}"
}
trap aur_cleanup EXIT

printf '%s\n' "${AUR_SSH_PRIVATE_KEY}" > "${AUR_KEY_FILE}"
chmod 600 "${AUR_KEY_FILE}"

ssh-keyscan -t ed25519 "${AUR_HOST}" > "${AUR_KNOWN_HOSTS}" 2>/dev/null
AUR_HOST_FINGERPRINT=$(ssh-keygen -lf "${AUR_KNOWN_HOSTS}" -E sha256 | awk '{print $2}')
if [[ "${AUR_HOST_FINGERPRINT}" != "${AUR_EXPECTED_HOST_FINGERPRINT}" ]]; then
    echo "Unexpected ${AUR_HOST} SSH fingerprint: ${AUR_HOST_FINGERPRINT}" >&2
    exit 1
fi

AUR_SSH_COMMAND="ssh -i ${AUR_KEY_FILE} -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=${AUR_KNOWN_HOSTS}"
GIT_SSH_COMMAND="${AUR_SSH_COMMAND}" \
    git clone "ssh://aur@${AUR_HOST}/cefdetector-bin.git" "${AUR_REPOSITORY_DIR}"

git -C "${AUR_REPOSITORY_DIR}" config user.name "github-actions[bot]"
git -C "${AUR_REPOSITORY_DIR}" config user.email "41898282+github-actions[bot]@users.noreply.github.com"

AUR_SHA256=$(sha256sum "${AUR_LOCAL_PACKAGE}" | awk '{print $1}')

cat > "${AUR_REPOSITORY_DIR}/PKGBUILD" <<EOF
pkgname=cefdetector-bin
pkgver=${AUR_VERSION}
pkgrel=1
pkgdesc="Check how many CEFs are on your Linux."
arch=('x86_64')
url="https://github.com/${AUR_GITHUB_REPOSITORY}"
license=('MIT')
depends=('fontconfig' 'libglvnd' 'xdg-utils')
provides=('cefdetector')
conflicts=('cefdetector')
source=("\${pkgname}-\${pkgver}.tar.gz::${AUR_PACKAGE_URL}")
sha256sums=('${AUR_SHA256}')
noextract=("\${pkgname}-\${pkgver}.tar.gz")

package() {
    bsdtar -xf "\${srcdir}/\${pkgname}-\${pkgver}.tar.gz" -C "\${pkgdir}/"
}
EOF

cat > "${AUR_REPOSITORY_DIR}/.SRCINFO" <<EOF
pkgbase = cefdetector-bin
	pkgdesc = Check how many CEFs are on your Linux.
	pkgver = ${AUR_VERSION}
	pkgrel = 1
	url = https://github.com/${AUR_GITHUB_REPOSITORY}
	arch = x86_64
	license = MIT
	depends = fontconfig
	depends = libglvnd
	depends = xdg-utils
	provides = cefdetector
	conflicts = cefdetector
	source = cefdetector-bin-${AUR_VERSION}.tar.gz::${AUR_PACKAGE_URL}
	noextract = cefdetector-bin-${AUR_VERSION}.tar.gz
	sha256sums = ${AUR_SHA256}

pkgname = cefdetector-bin
EOF

git -C "${AUR_REPOSITORY_DIR}" add PKGBUILD .SRCINFO
if git -C "${AUR_REPOSITORY_DIR}" diff-index --quiet HEAD --; then
    echo "AUR package is already up to date."
    exit 0
fi

git -C "${AUR_REPOSITORY_DIR}" commit -m "Update to ${AUR_TAG}"
GIT_SSH_COMMAND="${AUR_SSH_COMMAND}" \
    git -C "${AUR_REPOSITORY_DIR}" push origin HEAD:master
