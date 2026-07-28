#!/usr/bin/env bash

set -euo pipefail

AUR_TAG=${1:-}
AUR_GITHUB_REPOSITORY=${GITHUB_REPOSITORY:-Tobiichi-Origuchi/CefDetectorLinux}
AUR_ASSET_DIR=${RELEASE_ASSET_DIR:-target/release/packager}
AUR_HOST="aur.archlinux.org"
AUR_EXPECTED_HOST_FINGERPRINT="SHA256:RFzBCUItH9LZS0cKB5UE6ceAYhBD5C8GeOBip8Z11+4"
AUR_RENDER_DIR=""
AUR_TEMPORARY_WORK_DIR=false
AUR_KEY_FILE=""

aur_usage() {
    echo "Usage: $0 <stable-v-prefixed-version> [--render-only <directory>]" >&2
}

if [[ ! "${AUR_TAG}" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
    aur_usage
    exit 2
fi
if (($# == 3)) && [[ "$2" == "--render-only" ]] && [[ -n "$3" ]]; then
    AUR_RENDER_DIR=$3
elif (($# != 1)); then
    aur_usage
    exit 2
fi

AUR_VERSION=${AUR_TAG#v}
AUR_DEFAULT_PACKAGE_FILE="cefdetector_${AUR_VERSION}_x86_64.tar.gz"
AUR_PLOCATE_PACKAGE_FILE="cefdetector-plocate_${AUR_VERSION}_x86_64.tar.gz"

for package_file in "${AUR_DEFAULT_PACKAGE_FILE}" "${AUR_PLOCATE_PACKAGE_FILE}"; do
    if [[ ! -f "${AUR_ASSET_DIR}/${package_file}" ]]; then
        echo "Release package not found: ${AUR_ASSET_DIR}/${package_file}" >&2
        exit 1
    fi
done

if [[ -n "${AUR_RENDER_DIR}" ]]; then
    mkdir -p -- "${AUR_RENDER_DIR}"
    AUR_WORK_DIR=$(cd -- "${AUR_RENDER_DIR}" && pwd)
else
    if [[ -z "${AUR_SSH_PRIVATE_KEY:-}" ]]; then
        echo "AUR_SSH_PRIVATE_KEY is required" >&2
        exit 2
    fi
    AUR_WORK_DIR=$(mktemp -d "${RUNNER_TEMP:-/tmp}/cefdetector-aur.XXXXXX")
    AUR_TEMPORARY_WORK_DIR=true
fi

AUR_KNOWN_HOSTS=""

aur_cleanup() {
    if [[ -n "${AUR_KEY_FILE}" ]]; then
        rm -f -- "${AUR_KEY_FILE}"
    fi
    if [[ "${AUR_TEMPORARY_WORK_DIR}" == true ]]; then
        rm -rf -- "${AUR_WORK_DIR}"
    fi
}
trap aur_cleanup EXIT

if [[ -z "${AUR_RENDER_DIR}" ]]; then
    AUR_KEY_FILE="${AUR_WORK_DIR}/aur_key"
    AUR_KNOWN_HOSTS="${AUR_WORK_DIR}/known_hosts"
    printf '%s\n' "${AUR_SSH_PRIVATE_KEY}" > "${AUR_KEY_FILE}"
    chmod 600 "${AUR_KEY_FILE}"

    ssh-keyscan -t ed25519 "${AUR_HOST}" > "${AUR_KNOWN_HOSTS}" 2>/dev/null
    AUR_HOST_FINGERPRINT=$(ssh-keygen -lf "${AUR_KNOWN_HOSTS}" -E sha256 | awk '{print $2}')
    if [[ "${AUR_HOST_FINGERPRINT}" != "${AUR_EXPECTED_HOST_FINGERPRINT}" ]]; then
        echo "Unexpected ${AUR_HOST} SSH fingerprint: ${AUR_HOST_FINGERPRINT}" >&2
        exit 1
    fi

    AUR_SSH_COMMAND="ssh -i ${AUR_KEY_FILE} -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=${AUR_KNOWN_HOSTS}"
fi

aur_render_package() {
    local package_name=$1
    local package_description=$2
    local package_file=$3
    local requires_plocate=$4
    local repository_dir=$5
    local local_package="${AUR_ASSET_DIR}/${package_file}"
    local package_url="https://github.com/${AUR_GITHUB_REPOSITORY}/releases/download/${AUR_TAG}/${package_file}"
    local package_sha256
    local pkgbuild_depends="'fontconfig' 'libglvnd' 'xdg-utils'"
    local srcinfo_dependencies=$'\tdepends = fontconfig\n\tdepends = libglvnd\n\tdepends = xdg-utils'

    if [[ "${requires_plocate}" == true ]]; then
        pkgbuild_depends+=" 'plocate'"
        srcinfo_dependencies+=$'\n\tdepends = plocate'
    fi

    package_sha256=$(sha256sum "${local_package}" | awk '{print $1}')

    cat > "${repository_dir}/PKGBUILD" <<EOF
pkgname=${package_name}
pkgver=${AUR_VERSION}
pkgrel=1
pkgdesc="${package_description}"
arch=('x86_64')
url="https://github.com/${AUR_GITHUB_REPOSITORY}"
license=('MIT')
depends=(${pkgbuild_depends})
provides=("cefdetector=\${pkgver}")
conflicts=('cefdetector')
source=("\${pkgname}-\${pkgver}.tar.gz::${package_url}")
sha256sums=('${package_sha256}')
noextract=("\${pkgname}-\${pkgver}.tar.gz")

package() {
    bsdtar -xf "\${srcdir}/\${pkgname}-\${pkgver}.tar.gz" -C "\${pkgdir}/"
}
EOF

    cat > "${repository_dir}/.SRCINFO" <<EOF
pkgbase = ${package_name}
	pkgdesc = ${package_description}
	pkgver = ${AUR_VERSION}
	pkgrel = 1
	url = https://github.com/${AUR_GITHUB_REPOSITORY}
	arch = x86_64
	license = MIT
${srcinfo_dependencies}
	provides = cefdetector=${AUR_VERSION}
	conflicts = cefdetector
	noextract = ${package_name}-${AUR_VERSION}.tar.gz
	source = ${package_name}-${AUR_VERSION}.tar.gz::${package_url}
	sha256sums = ${package_sha256}

pkgname = ${package_name}
EOF
}

aur_publish_package() {
    local package_name=$1
    local package_description=$2
    local package_file=$3
    local requires_plocate=$4
    local repository_dir="${AUR_WORK_DIR}/${package_name}"

    if [[ -n "${AUR_RENDER_DIR}" ]]; then
        mkdir -p -- "${repository_dir}"
    else
        GIT_SSH_COMMAND="${AUR_SSH_COMMAND}" \
            git -c init.defaultBranch=master clone \
            "ssh://aur@${AUR_HOST}/${package_name}.git" \
            "${repository_dir}"
        git -C "${repository_dir}" config user.name "github-actions[bot]"
        git -C "${repository_dir}" config user.email \
            "41898282+github-actions[bot]@users.noreply.github.com"
    fi

    aur_render_package \
        "${package_name}" \
        "${package_description}" \
        "${package_file}" \
        "${requires_plocate}" \
        "${repository_dir}"

    if [[ -n "${AUR_RENDER_DIR}" ]]; then
        echo "Rendered ${package_name} metadata in ${repository_dir}"
        return
    fi

    git -C "${repository_dir}" add PKGBUILD .SRCINFO
    if git -C "${repository_dir}" rev-parse --verify HEAD >/dev/null 2>&1 \
        && git -C "${repository_dir}" diff-index --quiet HEAD --; then
        echo "${package_name} is already up to date."
        return
    fi

    git -C "${repository_dir}" commit -m "Update to ${AUR_TAG}"
    GIT_SSH_COMMAND="${AUR_SSH_COMMAND}" \
        git -C "${repository_dir}" push origin HEAD:master
}

aur_publish_package \
    "cefdetector-bin" \
    "Check how many CEFs are on your Linux." \
    "${AUR_DEFAULT_PACKAGE_FILE}" \
    false

aur_publish_package \
    "cefdetector-plocate-bin" \
    "Check how many CEFs are on your Linux using the plocate index." \
    "${AUR_PLOCATE_PACKAGE_FILE}" \
    true
