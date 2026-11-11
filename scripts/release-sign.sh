#!/usr/bin/env bash
# Sign and publish a release that CI built as a draft (design section 12,
# "App updates"):
#
#   scripts/release-sign.sh v1.2.0 --key ~/post-release.key
#
# Downloads the draft's files, checks each manifest against its files, asks
# for the release key's passphrase and signs each manifest, uploads the
# signatures and publishes the release. Servers fetch it from then on, and
# apps install it only because of these signatures. The key never leaves
# this machine.
#
# Needs the GitHub CLI (gh), signed in with write access to the repository.
set -euo pipefail

usage() { echo "usage: release-sign.sh vX.Y.Z --key KEY_FILE" >&2; exit 1; }
tag=${1:-}
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || usage
shift
key=
while [[ $# -gt 0 ]]; do
  case $1 in
    --key) key=${2:-}; shift 2 ;;
    *) usage ;;
  esac
done
[[ -f $key ]] || usage
key=$(realpath "$key")
repo=${POST_REPO:-DaFrancc/Post}
cd "$(dirname "$0")/.."

[[ $(gh release view "$tag" -R "$repo" --json isDraft -q .isDraft) == true ]] \
  || { echo "$tag is not a draft release (already published, or not built yet)" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
gh release download "$tag" -R "$repo" -D "$work"
shopt -s nullglob
manifests=("$work"/manifest-*.json)
[[ ${#manifests[@]} -gt 0 ]] || { echo "the draft has no manifests" >&2; exit 1; }

cargo build -q --release -p post-release
tool=target/release/post-release
for m in "${manifests[@]}"; do
  echo "== $(basename "$m")"
  sed -n 's/^  "\(version\|platform\|min_version\)": /  \1: /p' "$m"
  "$tool" sign --key "$key" "$m"
  "$tool" check "$m"
done

read -rp "Publish $tag? [y/N] " answer
[[ $answer == [yY] ]] || { echo "not published; the draft is unchanged"; exit 1; }
for m in "${manifests[@]}"; do gh release upload "$tag" -R "$repo" "$m.sig"; done
gh release edit "$tag" -R "$repo" --draft=false --latest
echo "$tag published. Servers will pick it up within the hour."
