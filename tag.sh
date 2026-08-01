#!/usr/bin/env bash

set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
cd "${SCRIPT_DIR}"

REMOTE="origin"
DRY_RUN=false
NO_PUSH=false
ASSUME_YES=false
SIGN_TAG=false
VERSION_INPUT=""
TEMP_DIR=""
FILES_MODIFIED=false
COMMIT_CREATED=false

usage() {
  cat <<'EOF'
Usage: ./tag.sh [options] <version>

Prepare and publish a release tag. <version> may be written as 1.2.3 or
v1.2.3. SemVer prereleases such as v1.2.3-rc.1 are supported.

Options:
  -n, --dry-run       Show the release plan without changing anything
      --no-push       Create the release commit and tag locally only
  -r, --remote NAME   Push to this Git remote (default: origin)
  -s, --sign          Create a GPG-signed tag instead of an annotated tag
  -y, --yes           Skip the confirmation prompt
  -h, --help          Show this help

The script requires a clean working tree, updates Cargo.toml and Cargo.lock,
checks both Linux search backends, creates an annotated release tag, and
atomically pushes the current branch and tag.
EOF
}

die() {
  printf 'Error: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  local exit_status=$?

  if [[ -n "${TEMP_DIR}" ]]; then
    if [[ "${FILES_MODIFIED}" == true && "${COMMIT_CREATED}" == false ]]; then
      cp -p -- "${TEMP_DIR}/Cargo.toml" Cargo.toml
      cp -p -- "${TEMP_DIR}/Cargo.lock" Cargo.lock
      printf 'Restored Cargo.toml and Cargo.lock.\n' >&2
    fi
    rm -rf -- "${TEMP_DIR}"
  fi

  exit "${exit_status}"
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

while (($# > 0)); do
  case "$1" in
    -n | --dry-run)
      DRY_RUN=true
      ;;
    --no-push)
      NO_PUSH=true
      ;;
    -r | --remote)
      shift
      (($# > 0)) || die "--remote requires a name"
      REMOTE="$1"
      ;;
    -s | --sign)
      SIGN_TAG=true
      ;;
    -y | --yes)
      ASSUME_YES=true
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    -*)
      die "unknown option: $1"
      ;;
    *)
      [[ -z "${VERSION_INPUT}" ]] || die "only one version may be specified"
      VERSION_INPUT="$1"
      ;;
  esac
  shift
done

[[ -n "${VERSION_INPUT}" ]] || {
  usage >&2
  exit 2
}

RAW_VERSION="${VERSION_INPUT#v}"
TAG="v${RAW_VERSION}"
readonly SEMVER_PATTERN='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$'

[[ "${RAW_VERSION}" =~ ${SEMVER_PATTERN} ]] ||
  die "version must be valid SemVer without build metadata (for example, v1.2.3 or v1.2.3-rc.1)"

for command in awk cargo git mktemp; do
  command -v "${command}" >/dev/null 2>&1 || die "required command not found: ${command}"
done

git rev-parse --is-inside-work-tree >/dev/null 2>&1 ||
  die "tag.sh must be run from a Git working tree"

CURRENT_BRANCH="$(git symbolic-ref --quiet --short HEAD)" ||
  die "releases cannot be prepared from a detached HEAD"

[[ -z "$(git status --porcelain=v1 --untracked-files=all)" ]] ||
  die "the working tree must be clean before preparing a release"

git rev-parse --verify --quiet "refs/tags/${TAG}" >/dev/null &&
  die "local tag ${TAG} already exists"

CURRENT_VERSION="$(
  awk '
    /^\[package\]$/ { in_package = 1; next }
    /^\[/ { in_package = 0 }
    in_package && $1 == "version" {
      value = $3
      gsub(/"/, "", value)
      print value
      exit
    }
  ' Cargo.toml
)"
[[ -n "${CURRENT_VERSION}" ]] || die "could not read the package version from Cargo.toml"

if [[ "${DRY_RUN}" == true ]]; then
  printf 'Release plan:\n'
  printf '  Version: %s -> %s\n' "${CURRENT_VERSION}" "${RAW_VERSION}"
  printf '  Branch:  %s\n' "${CURRENT_BRANCH}"
  printf '  Tag:     %s (%s)\n' "${TAG}" "$([[ "${SIGN_TAG}" == true ]] && printf signed || printf annotated)"
  if [[ "${NO_PUSH}" == true ]]; then
    printf '  Push:    disabled\n'
  else
    printf '  Push:    branch and tag to %s atomically\n' "${REMOTE}"
  fi
  printf '  Checks:  format, tests, Clippy, and release builds for ignore and plocate\n'
  exit 0
fi

if [[ "${NO_PUSH}" == false ]]; then
  git remote get-url "${REMOTE}" >/dev/null 2>&1 ||
    die "Git remote does not exist: ${REMOTE}"

  if git ls-remote --exit-code --tags "${REMOTE}" "refs/tags/${TAG}" >/dev/null 2>&1; then
    die "remote tag ${TAG} already exists on ${REMOTE}"
  else
    remote_status=$?
    [[ ${remote_status} -eq 2 ]] ||
      die "could not check whether ${TAG} exists on ${REMOTE}"
  fi
fi

if [[ "${CURRENT_VERSION}" != "${RAW_VERSION}" ]]; then
  TEMP_DIR="$(mktemp -d -t cefdetector-release.XXXXXXXX)"
  cp -p -- Cargo.toml "${TEMP_DIR}/Cargo.toml"
  cp -p -- Cargo.lock "${TEMP_DIR}/Cargo.lock"

  if ! awk -v version="${RAW_VERSION}" '
    BEGIN { changed = 0 }
    /^\[package\]$/ {
      in_package = 1
      print
      next
    }
    /^\[/ { in_package = 0 }
    in_package && !changed && $1 == "version" {
      print "version = \"" version "\""
      changed = 1
      next
    }
    { print }
    END {
      if (!changed) {
        exit 1
      }
    }
  ' Cargo.toml >"${TEMP_DIR}/Cargo.toml.updated"; then
    die "could not update the package version in Cargo.toml"
  fi

  mv -- "${TEMP_DIR}/Cargo.toml.updated" Cargo.toml
  FILES_MODIFIED=true

  printf 'Updating Cargo.lock for %s...\n' "${RAW_VERSION}"
  cargo metadata --format-version 1 --offline >/dev/null
fi

PACKAGE_ID="$(cargo pkgid --locked)"
PACKAGE_VERSION="${PACKAGE_ID##*@}"
[[ "${PACKAGE_VERSION}" == "${RAW_VERSION}" ]] ||
  die "Cargo package version ${PACKAGE_VERSION} does not match ${RAW_VERSION}"

printf 'Running release checks...\n'
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --no-default-features --features gui,plocate --all-targets
cargo clippy --locked --no-default-features --features gui,plocate --all-targets -- -D warnings
cargo build --locked --release
cargo build --locked --release --no-default-features --features gui,plocate
git diff --check

if [[ "${CURRENT_VERSION}" != "${RAW_VERSION}" ]]; then
  printf '\nVersion changes:\n'
  git diff -- Cargo.toml Cargo.lock
fi

if [[ "${ASSUME_YES}" == false ]]; then
  [[ -t 0 ]] || die "confirmation requires a terminal; pass --yes to continue non-interactively"
  printf '\nCreate %s on %s and %s? [y/N] ' "${TAG}" "${CURRENT_BRANCH}" \
    "$([[ "${NO_PUSH}" == true ]] && printf 'keep it local' || printf 'push it')"
  read -r reply
  [[ "${reply}" =~ ^[Yy]([Ee][Ss])?$ ]] || {
    printf 'Release cancelled.\n'
    exit 0
  }
fi

if [[ "${CURRENT_VERSION}" != "${RAW_VERSION}" ]]; then
  git add -- Cargo.toml Cargo.lock
  git commit -m "Release ${TAG}"
  COMMIT_CREATED=true
fi

if [[ "${SIGN_TAG}" == true ]]; then
  git tag --sign --message "Release ${TAG}" "${TAG}"
else
  git tag --annotate --message "Release ${TAG}" "${TAG}"
fi

if [[ "${NO_PUSH}" == true ]]; then
  printf 'Created %s locally on %s.\n' "${TAG}" "${CURRENT_BRANCH}"
  printf 'Publish it with:\n'
  printf '  git push --atomic %q HEAD:refs/heads/%q refs/tags/%q:refs/tags/%q\n' \
    "${REMOTE}" "${CURRENT_BRANCH}" "${TAG}" "${TAG}"
  exit 0
fi

printf 'Pushing %s and %s to %s atomically...\n' "${CURRENT_BRANCH}" "${TAG}" "${REMOTE}"
git push --atomic "${REMOTE}" \
  "HEAD:refs/heads/${CURRENT_BRANCH}" \
  "refs/tags/${TAG}:refs/tags/${TAG}"

printf 'Published %s. The GitHub release workflow will now build the packages.\n' "${TAG}"
