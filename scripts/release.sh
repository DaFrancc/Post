#!/usr/bin/env bash
# Making a Post release, start to finish (design section 12, "App updates").
#
#   scripts/release.sh new 0.2.0     tag a version and push it: CI builds a draft
#   scripts/release.sh sign          check, sign and publish the newest draft
#   scripts/release.sh status        which releases are drafts, which published
#
# `new` opens your editor for the release notes (what apps show as "What's
# new"). A line "Min-Version: 0.1.0" in them marks older apps as too old for
# the server. CI then builds the release, which takes a few minutes.
#
# `sign` downloads the draft, checks every file against its manifest, asks for
# the release key's passphrase, signs, and publishes after you confirm. The
# key is ~/post-release.key unless POST_RELEASE_KEY names another file, and it
# never leaves this machine. `sign v0.2.0` picks a draft other than the newest.
#
# Needs the GitHub CLI (gh), signed in with write access to the repository.
set -euo pipefail

repo=${POST_REPO:-DaFrancc/Post}
key=${POST_RELEASE_KEY:-$HOME/post-release.key}
cd "$(dirname "$0")/.."

die() { echo "$*" >&2; exit 1; }

usage() {
  sed -n '3,6p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

newest_draft() {
  gh release list -R "$repo" --json tagName,isDraft,createdAt \
    -q '[.[] | select(.isDraft)] | sort_by(.createdAt) | last | .tagName // empty'
}

cmd_new() {
  local version=${1:-}
  [[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "usage: release.sh new X.Y.Z  (for example: release.sh new 0.2.0)"
  local tag=v$version
  git rev-parse -q --verify "refs/tags/$tag" >/dev/null && die "$tag already exists"
  [[ -z $(git status --porcelain --untracked-files=no) ]] || die "commit or stash your changes first: the release is built from what is committed"
  git fetch -q origin
  local branch
  branch=$(git rev-parse --abbrev-ref HEAD)
  [[ $(git rev-parse HEAD) == $(git rev-parse "origin/$branch" 2>/dev/null || true) ]] \
    || die "push $branch first, so CI builds the same commit you have"
  echo "Tagging $(git log -1 --format='%h %s') as $tag."
  echo "Your editor opens for the release notes."
  git tag -a "$tag" -e -m "Post $version

# Below, write what changed: the app shows it as \"What's new\".
# Lines starting with # are left out.
#
# To mark older apps as too old for the server, add a line like:
# Min-Version: 0.1.0"
  git push -q origin "$tag"
  echo "Pushed $tag. CI is building it: https://github.com/$repo/actions"
  echo "When it finishes, run: scripts/release.sh sign"
}

cmd_sign() {
  local tag=${1:-}
  if [[ -z $tag ]]; then
    tag=$(newest_draft)
    [[ -n $tag ]] || die "there is no draft release to sign (CI may still be building: scripts/release.sh status)"
  fi
  [[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "usage: release.sh sign [vX.Y.Z]"
  [[ -f $key ]] || die "no release key at $key (set POST_RELEASE_KEY to where it is)"
  [[ $(gh release view "$tag" -R "$repo" --json isDraft -q .isDraft) == true ]] \
    || die "$tag is not a draft release (already published, or not built yet)"

  local work
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  echo "Downloading $tag…"
  gh release download "$tag" -R "$repo" -D "$work"
  shopt -s nullglob
  local manifests=("$work"/manifest-*.json)
  [[ ${#manifests[@]} -gt 0 ]] || die "the draft has no manifests"

  cargo build -q --release -p post-release
  local tool=target/release/post-release m
  for m in "${manifests[@]}"; do
    echo
    echo "== $(basename "$m")"
    sed -n 's/^  "\(version\|platform\|min_version\)": /  \1: /p' "$m"
    "$tool" sign --key "$key" "$m"
    "$tool" check "$m"
  done

  echo
  read -rp "Publish $tag? Servers fetch it within the hour. [y/N] " answer
  [[ $answer == [yY] ]] || { echo "Not published; the draft is unchanged."; exit 1; }
  for m in "${manifests[@]}"; do gh release upload -R "$repo" "$tag" "$m.sig"; done
  gh release edit "$tag" -R "$repo" --draft=false --latest >/dev/null
  echo "$tag published."
}

cmd_status() {
  local list
  list=$(gh release list -R "$repo" --limit 10 --json tagName,isDraft,isLatest \
    -q '.[] | "\(.tagName)\t\(if .isDraft then "draft: sign it with scripts/release.sh sign" elif .isLatest then "published, latest" else "published" end)"')
  if [[ -n $list ]]; then column -t -s $'\t' <<<"$list"; else echo "No releases yet. Start one with: scripts/release.sh new X.Y.Z"; fi
  local running
  running=$(gh run list -R "$repo" --workflow release.yml --status in_progress --json displayTitle -q '.[].displayTitle' 2>/dev/null || true)
  [[ -z $running ]] || echo "CI still building: $running"
}

case ${1:-} in
  new) shift; cmd_new "$@" ;;
  sign) shift; cmd_sign "$@" ;;
  status) cmd_status ;;
  ""|help|-h|--help) usage 0 ;;
  *) usage 1 ;;
esac
