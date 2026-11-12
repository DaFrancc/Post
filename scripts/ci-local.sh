#!/usr/bin/env bash
# Run CI's builds here first, so a broken build is found before it reaches
# GitHub (and before a release tag that cannot be taken back).
#
#   scripts/ci-local.sh [release|server|all] [--worktree] [--version X.Y.Z]
#
#   release   .github/workflows/release.yml itself, run by act (the GitHub
#             CLI extension nektos/gh-act) in the runner image act uses for
#             ubuntu-22.04, built from GitHub's own runner scripts. It stops
#             before the draft release: nothing is published.
#   server    the server image, built as server-image.yml builds it
#             (podman build with deploy/Containerfile). It is built inside
#             its own base images, so the runner it is built on does not
#             matter.
#   all       both (the default)
#
# It builds the committed HEAD, from a fresh clone, which is what CI sees;
# --worktree builds the tracked files as they are on disk instead, to try a
# change before committing it. The release build's files land in
# app/build/ci-local/. act talks to Podman through its Docker-compatible
# socket, which this starts if it is not running.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo"

# act's image for GitHub's ubuntu-22.04 runner: the release build's.
runner_base=ghcr.io/catthehacker/ubuntu:act-22.04
# The same, with /var/run a folder rather than a link to /run: act copies
# actions into /var/run/act, and Podman (unlike Docker) refuses to copy
# through a link ("path escapes from parent").
runner_image=localhost/post-act-runner:22.04

what=all worktree=false version=0.0.1
while (($#)); do
  case $1 in
    release | server | all) what=$1 ;;
    --worktree) worktree=true ;;
    --version) version=${2:?--version needs X.Y.Z}; shift ;;
    -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument $1 (see --help)" >&2; exit 1 ;;
  esac
  shift
done
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "--version must be like 1.2.0" >&2; exit 1; }
command -v podman >/dev/null || { echo "needs Podman" >&2; exit 1; }

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }

if ! $worktree && [[ -n $(git status --porcelain --untracked-files=no) ]]; then
  echo "Uncommitted changes are not built (CI would not see them either); --worktree builds them."
fi
label=$($worktree && echo "the working tree" || git log -1 --format='%h %s')

# A fresh clone for this run, removed however it ends: only what is in the
# repository, none of the files lying around this checkout.
work=$(mktemp -d -p "${TMPDIR:-/var/tmp}" post-ci.XXXXXX)
trap 'rm -rf -- "$work"' EXIT
git clone -q --no-hardlinks "$repo" "$work/src"
if $worktree; then
  # Tracked files as they are now, over the clone; deleted ones go.
  git ls-files -z | while IFS= read -r -d '' f; do
    if [[ -e $f ]]; then
      mkdir -p "$work/src/$(dirname "$f")"
      cp -p -- "$f" "$work/src/$f"
    else
      rm -f -- "$work/src/$f"
    fi
  done
fi

run_release() {
  gh act --version >/dev/null 2>&1 || { echo "needs act: gh extension install nektos/gh-act" >&2; exit 1; }
  local sock=${XDG_RUNTIME_DIR:?}/podman/podman.sock
  if [[ ! -S $sock ]]; then
    say "Starting Podman's Docker-compatible socket for act (until logout)"
    systemctl --user start podman.socket
  fi
  say "Runner image (act's ubuntu-22.04, for Podman)"
  podman pull -q "$runner_base" >/dev/null
  printf 'FROM %s\nRUN rm /var/run && mkdir /var/run\n' "$runner_base" | podman build -q -t "$runner_image" -f - >/dev/null
  say "release.yml on $label"
  # The tag push CI builds from, as act's event.
  printf '{"ref": "refs/tags/app-v%s"}\n' "$version" >"$work/event.json"
  (
    cd "$work/src"
    # --pull=false: the runner image is the one just built here. No Docker
    # socket in the job: the release build runs no containers.
    DOCKER_HOST=unix://$sock gh act push \
      -W .github/workflows/release.yml -j linux-x86_64 \
      -e "$work/event.json" \
      -P ubuntu-22.04="$runner_image" --pull=false \
      --container-daemon-socket - \
      --bind --rm
  )
  local out=$repo/app/build/ci-local
  mkdir -p "$out"
  cp "$work/src/dist/"* "$out/"
  say "Release build passed: $out"
}

run_server() {
  say "Server image of $label"
  podman build -q -f "$work/src/deploy/Containerfile" -t localhost/postd:ci-local "$work/src" >/dev/null
  say "Server image passed: localhost/postd:ci-local"
}

case $what in
  release) run_release ;;
  server) run_server ;;
  all) run_release; run_server ;;
esac
