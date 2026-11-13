#!/usr/bin/env bash
# Making a Post release, start to finish (design section 12, "App updates").
#
#   scripts/release.sh new app 0.3.0      tag an app version and push it: CI
#                                         builds a draft app release
#   scripts/release.sh new server 0.3.0   tag a server version and push it:
#                                         CI builds its server image
#   scripts/release.sh changes            what changed in each since its last release
#   scripts/release.sh sign               check, sign and publish the drafts, app and server
#   scripts/release.sh status             which releases are drafts, which published
#
# The app and the server have their own versions (tags app-vX.Y.Z and
# server-vX.Y.Z): release whichever changed. They agree through the
# protocol, not their numbers. `new` lists what changed since that side's
# last release, then opens your editor for the notes: for the app, what it
# shows as "What's new", where a line "Min-Version: 0.1.0" marks older apps
# as too old. CI then builds it, which takes a few minutes. Before tagging,
# run scripts/ci-local.sh if you want the same builds tried here first.
#
# `sign` takes each draft in turn: it downloads it, checks it (an app's files
# against its manifest; a server's commit against its tag), asks for the
# release key's passphrase, signs, and publishes after you confirm. A server
# release is signed under its own label and never becomes the latest
# release, which stays the app's. The key is ~/post-release.key unless
# POST_RELEASE_KEY names another file, and it never leaves this machine.
# `sign app-v0.3.0` signs just that one.
#
# Needs the GitHub CLI (gh), signed in with write access to the repository.
set -euo pipefail

repo=${POST_REPO:-DaFrancc/Post}
key=${POST_RELEASE_KEY:-$HOME/post-release.key}
cd "$(dirname "$0")/.."

die() { echo "$*" >&2; exit 1; }

usage() {
  sed -n '3,10p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

# What goes into each side, for listing what changed: its own code and the
# code it shares.
paths_for() {
  case $1 in
    app) echo app crates/post-core crates/post-ffi crates/post-proto crates/post-identity crates/post-mls crates/post-release Cargo.lock ;;
    # deploy/ too: servers run a server release's checkout, server.sh included.
    server) echo crates/post-server crates/post-proto crates/post-identity crates/post-mls crates/post-release deploy Cargo.lock ;;
  esac
}

# The newest release tag of a side; before tags were split, the newest vX.Y.Z.
last_tag() {
  local t
  t=$(git tag -l "$1-v[0-9]*" --sort=-v:refname | grep -E "^$1-v[0-9]+\.[0-9]+\.[0-9]+$" | head -1 || true)
  [[ -n $t ]] || t=$(git tag -l 'v[0-9]*' --sort=-v:refname | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -1 || true)
  printf '%s' "$t"
}

# Commits since a side's last release that touch it.
show_changes() {
  local kind=$1 since
  since=$(last_tag "$kind")
  # shellcheck disable=SC2046 # the paths are words on purpose
  if [[ -n $since ]]; then
    echo "$kind: changes since $since"
    git log --oneline "$since..HEAD" -- $(paths_for "$kind") | sed 's/^/  /' | grep . || echo "  none"
  else
    echo "$kind: no release yet"
  fi
}

cmd_changes() {
  git fetch -q --tags origin
  show_changes app
  echo
  show_changes server
}

cmd_new() {
  local kind=${1:-} version=${2:-}
  [[ $kind == app || $kind == server ]] && [[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
    || die "usage: release.sh new app|server X.Y.Z  (for example: release.sh new app 0.3.0)"
  local tag=$kind-v$version
  git fetch -q --tags origin
  git rev-parse -q --verify "refs/tags/$tag" >/dev/null && die "$tag already exists"
  show_changes "$kind"
  echo
  # The release is built from the pushed commit, so local changes never get
  # in; say which ones, in case something meant for it is not committed.
  local changed
  changed=$(git status --porcelain --untracked-files=no)
  if [[ -n $changed ]]; then
    echo "These changes are not committed, so they will not be in the release:"
    sed 's/^/  /' <<<"$changed"
    read -rp "Go on without them? [y/N] " answer
    [[ $answer == [yY] ]] || exit 1
  fi
  git fetch -q origin
  local branch
  branch=$(git rev-parse --abbrev-ref HEAD)
  [[ $(git rev-parse HEAD) == $(git rev-parse "origin/$branch" 2>/dev/null || true) ]] \
    || die "push $branch first, so CI builds the same commit you have"
  echo "Tagging $(git log -1 --format='%h %s') as $tag."
  echo "Your editor opens for the release notes."
  if [[ $kind == app ]]; then
    git tag -a "$tag" -e -m "Post $version

# Below, write what changed: the app shows it as \"What's new\".
# Lines starting with # are left out.
#
# To mark older apps as too old for the server, add a line like:
# Min-Version: 0.1.0"
  else
    git tag -a "$tag" -e -m "Post server $version

# Below, write what changed, for whoever runs a server.
# Lines starting with # are left out."
  fi
  git push -q origin "$tag"
  echo "Pushed $tag. CI is building it: https://github.com/$repo/actions"
  if [[ $kind == app ]]; then
    echo "When it finishes, run: scripts/release.sh sign"
  else
    echo "When it finishes, servers take it with: deploy/server.sh update"
  fi
}

all_drafts() {
  gh release list -R "$repo" --json tagName,isDraft,createdAt \
    -q '[.[] | select(.isDraft)] | sort_by(.createdAt) | .[].tagName'
}

# Sign every draft (one passphrase each), or the one named.
cmd_sign() {
  local tags
  if [[ -n ${1:-} ]]; then
    tags=$1
  else
    tags=$(all_drafts)
    [[ -n $tags ]] || die "there is no draft release to sign (CI may still be building: scripts/release.sh status)"
  fi
  [[ -f $key ]] || die "no release key at $key (set POST_RELEASE_KEY to where it is)"
  cargo build -q --release -p post-release
  git fetch -q --tags origin
  local tag
  for tag in $tags; do
    sign_one "$tag"
  done
}

sign_one() {
  local tag=$1
  # App releases (the first ones were tagged vX.Y.Z) and server releases.
  [[ $tag =~ ^(app-|server-)?v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "usage: release.sh sign [app-vX.Y.Z | server-vX.Y.Z]"
  [[ $(gh release view "$tag" -R "$repo" --json isDraft -q .isDraft) == true ]] \
    || die "$tag is not a draft release (already published, or not built yet)"

  local work
  work=$(mktemp -d)
  echo
  echo "Downloading $tag…"
  gh release download "$tag" -R "$repo" -D "$work"
  shopt -s nullglob
  local manifests=("$work"/manifest-*.json)
  [[ ${#manifests[@]} -gt 0 ]] || { rm -rf -- "$work"; die "the draft has no manifests"; }

  local tool=target/release/post-release m
  for m in "${manifests[@]}"; do
    echo "== $(basename "$m")"
    if [[ $tag == server-v* ]]; then
      # What you sign: the image and the commit. The commit must be the
      # tag's here too, so a tag moved on GitHub is noticed.
      local want
      want=$(git rev-parse "$tag^{commit}")
      sed -n 's/^  "\(version\|image_digest\|commit\)": /  \1: /p' "$m"
      grep -q "\"commit\": \"$want\"" "$m" || { rm -rf -- "$work"; die "the manifest's commit is not $tag's here ($want)"; }
      "$tool" sign-server --key "$key" "$m"
      "$tool" check-server "$m"
    else
      sed -n 's/^  "\(version\|platform\|min_version\)": /  \1: /p' "$m"
      "$tool" sign --key "$key" "$m"
      "$tool" check "$m"
    fi
  done

  echo
  local where="Servers fetch it within the hour."
  [[ $tag == server-v* ]] && where="Servers take it with deploy/server.sh update."
  read -rp "Publish $tag? $where [y/N] " answer
  if [[ $answer != [yY] ]]; then
    rm -rf -- "$work"
    echo "Not published; the draft is unchanged."
    return 0
  fi
  for m in "${manifests[@]}"; do gh release upload -R "$repo" "$tag" "$m.sig"; done
  # The latest release is always the app's: servers hand it out to apps.
  if [[ $tag == server-v* ]]; then
    gh release edit "$tag" -R "$repo" --draft=false --latest=false >/dev/null
  else
    gh release edit "$tag" -R "$repo" --draft=false --latest >/dev/null
  fi
  rm -rf -- "$work"
  echo "$tag published."
}

cmd_status() {
  local list
  list=$(gh release list -R "$repo" --limit 10 --json tagName,isDraft,isLatest \
    -q '.[] | "\(.tagName)\t\(if .isDraft then "draft: sign it with scripts/release.sh sign" elif .isLatest then "published, latest" else "published" end)"')
  if [[ -n $list ]]; then column -t -s $'\t' <<<"$list"; else echo "No releases yet. Start one with: scripts/release.sh new app X.Y.Z"; fi
  local running
  running=$(gh run list -R "$repo" --workflow release.yml --status in_progress --json displayTitle -q '.[].displayTitle' 2>/dev/null || true)
  [[ -z $running ]] || echo "CI still building: $running"
}

case ${1:-} in
  new) shift; cmd_new "$@" ;;
  changes) cmd_changes ;;
  sign) shift; cmd_sign "$@" ;;
  status) cmd_status ;;
  ""|help|-h|--help) usage 0 ;;
  *) usage 1 ;;
esac
