#!/usr/bin/env bash
# Generate the `latest.json` manifest the Tauri updater reads.
#
# Assumes `npm run tauri:build` has already run with TAURI_SIGNING_PRIVATE_KEY
# set, which produces these signed updater artifacts in src-tauri/target/:
#   <arch>/release/bundle/macos/Claude Memory Manager.app.tar.gz
#   <arch>/release/bundle/macos/Claude Memory Manager.app.tar.gz.sig
#
# Usage:
#   scripts/generate-latest-json.sh <version> [release-notes]
# Example:
#   scripts/generate-latest-json.sh 0.2.2 "Bug fixes and auto-updater."

set -euo pipefail

VERSION="${1:?version required, e.g. 0.2.2}"
NOTES="${2:-}"

REPO="byronfichardt/claude-memory-manager"
BASE_URL="https://github.com/${REPO}/releases/download/v${VERSION}"
ASSET="Claude.Memory.Manager.app.tar.gz"
PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

TARGET_DIR="src-tauri/target"

host_target() {
  case "$(uname -m)" in
    arm64|aarch64) printf 'aarch64-apple-darwin' ;;
    x86_64)        printf 'x86_64-apple-darwin' ;;
    *) echo "error: unsupported host arch $(uname -m)" >&2; return 1 ;;
  esac
}

find_bundle() {
  local arch="$1"
  local candidate
  local arch_specific="${TARGET_DIR}/${arch}/release/bundle/macos/Claude Memory Manager.app.tar.gz"
  if [ -f "${arch_specific}" ]; then
    printf '%s' "${arch_specific}"
    return 0
  fi
  # Fallback path (no --target passed to tauri build) belongs to the host arch only.
  local host
  host="$(host_target)" || return 1
  if [ "${arch}" = "${host}" ]; then
    candidate="${TARGET_DIR}/release/bundle/macos/Claude Memory Manager.app.tar.gz"
    if [ -f "${candidate}" ]; then
      printf '%s' "${candidate}"
      return 0
    fi
  fi
  return 1
}

platform_entry() {
  local arch="$1"
  local tauri_target="$2"
  local bundle
  if ! bundle="$(find_bundle "${tauri_target}")"; then
    echo "warn: no bundle for ${arch} (${tauri_target}); skipping" >&2
    return 1
  fi
  local sig_file="${bundle}.sig"
  if [ ! -f "${sig_file}" ]; then
    echo "error: missing signature file ${sig_file} — was TAURI_SIGNING_PRIVATE_KEY set during build?" >&2
    return 1
  fi
  local signature
  signature="$(cat "${sig_file}")"
  jq -n \
    --arg sig "${signature}" \
    --arg url "${BASE_URL}/${ASSET}" \
    '{signature: $sig, url: $url}'
}

platforms_json="{}"
for pair in "darwin-aarch64:aarch64-apple-darwin" "darwin-x86_64:x86_64-apple-darwin"; do
  arch="${pair%%:*}"
  target="${pair##*:}"
  if entry="$(platform_entry "${arch}" "${target}")"; then
    platforms_json="$(jq --arg arch "${arch}" --argjson entry "${entry}" \
      '. + {($arch): $entry}' <<<"${platforms_json}")"
  fi
done

if [ "${platforms_json}" = "{}" ]; then
  echo "error: no platform artifacts found under ${TARGET_DIR}. Did you run 'npm run tauri:build'?" >&2
  exit 1
fi

jq -n \
  --arg version "${VERSION}" \
  --arg notes "${NOTES}" \
  --arg pub_date "${PUB_DATE}" \
  --argjson platforms "${platforms_json}" \
  '{version: $version, notes: $notes, pub_date: $pub_date, platforms: $platforms}' \
  >latest.json

echo "wrote latest.json for v${VERSION}" >&2
