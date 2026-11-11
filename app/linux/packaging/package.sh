#!/usr/bin/env bash
# Pack a release build of the app into the tarball a release ships:
#
#   flutter build linux --release --dart-define=POST_VERSION=1.2.0
#   app/linux/packaging/package.sh 1.2.0 [OUT_DIR]   (default: app/build/dist)
#
# Gives OUT_DIR/post-1.2.0-linux-x86_64.tar.gz: one folder holding the app,
# its VERSION, install.sh, and the desktop entry and icons. The archive is
# made the same way every time (sorted names, fixed owner and timestamps,
# no gzip timestamp), so the same build always packs to the same bytes.
set -euo pipefail

version=${1:?usage: package.sh VERSION [OUT_DIR]}
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must be like 1.2.0" >&2; exit 1; }
here=$(cd "$(dirname "$0")" && pwd)
app=$(cd "$here/../.." && pwd)
out=$(realpath -m "${2:-$app/build/dist}")
bundle=$app/build/linux/x64/release/bundle
[[ -x $bundle/post ]] || { echo "no release build at $bundle; run: flutter build linux --release" >&2; exit 1; }

name=post-$version-linux-x86_64
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
dir=$work/$name
cp -a "$bundle" "$dir"
printf '%s\n' "$version" > "$dir/VERSION"
install -m755 "$here/install.sh" "$dir/install.sh"
install -Dm644 "$here/com.dafrancc.post.desktop" "$dir/share/com.dafrancc.post.desktop"
for icon in "$here"/icons/*.png; do install -Dm644 "$icon" "$dir/share/icons/$(basename "$icon")"; done

# Timestamps from the commit being released, when there is one.
epoch=${SOURCE_DATE_EPOCH:-$(git -C "$app" log -1 --format=%ct 2>/dev/null || echo 0)}
mkdir -p "$out"
tar --sort=name --mtime="@$epoch" --owner=0 --group=0 --numeric-owner --format=gnu \
  -C "$work" -cf - "$name" | gzip -9n > "$out/$name.tar.gz"
echo "$out/$name.tar.gz"
