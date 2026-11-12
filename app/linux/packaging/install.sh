#!/usr/bin/env bash
# Install Post for the current user from an unpacked release tarball.
#
#   tar xzf post-1.2.0-linux-x86_64.tar.gz
#   post-1.2.0-linux-x86_64/install.sh
#
# Post goes in ~/.local/share/post, one folder per version, with `current`
# pointing at the one to run (design section 12, "App updates"):
#
#   ~/.local/share/post/versions/1.2.0/   this folder, copied whole
#   ~/.local/share/post/current -> versions/1.2.0
#
# plus a desktop entry and icons, so Post shows in the app menu. Later
# versions install themselves the same way from inside the app; nothing
# here needs root.
#
#   install.sh --uninstall    remove Post (your messages stay, in
#                             ~/.local/share/com.dafrancc.post)
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
app_id=com.dafrancc.post
data=${XDG_DATA_HOME:-$HOME/.local/share}
root=$data/post
sizes=(32 48 64 128 256 512)

desktop_refresh() {
  command -v update-desktop-database >/dev/null && update-desktop-database -q "$data/applications" || true
  command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$data/icons/hicolor" 2>/dev/null || true
  command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 --noincremental >/dev/null 2>&1 || true
}

if [[ ${1:-} == --uninstall ]]; then
  rm -rf "$root"
  rm -f "$data/applications/$app_id.desktop"
  for s in "${sizes[@]}"; do rm -f "$data/icons/hicolor/${s}x${s}/apps/$app_id.png"; done
  desktop_refresh
  echo "Post removed. Your messages and keys are still in $data/$app_id; delete that folder to remove them too."
  exit 0
fi

version=$(<"$here/VERSION")
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "this folder has no valid VERSION file" >&2; exit 1; }
[[ -x $here/post ]] || { echo "no Post program next to this script" >&2; exit 1; }

# Copy the version in under a temporary name, then give it its own.
mkdir -p "$root/versions"
staging=$root/versions/.$version.installing
rm -rf "$staging"
cp -a "$here" "$staging"
rm -rf "$root/versions/$version"
mv "$staging" "$root/versions/$version"

# Point `current` at it in one step.
ln -sfn "versions/$version" "$root/.current.next"
mv -T "$root/.current.next" "$root/current"

for s in "${sizes[@]}"; do
  install -Dm644 "$here/share/icons/$s.png" "$data/icons/hicolor/${s}x${s}/apps/$app_id.png"
done
install -d "$data/applications"
sed "s|^Exec=.*|Exec=$root/current/post|" "$here/share/$app_id.desktop" > "$data/applications/$app_id.desktop"
chmod 644 "$data/applications/$app_id.desktop"
desktop_refresh

echo "Post $version installed. Start it from your app menu, or run $root/current/post"
