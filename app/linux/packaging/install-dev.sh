#!/usr/bin/env bash
# Install Postern's desktop entry and icon for the current user, pointing at
# a local build, so the desktop shows Postern's name and icon for its window.
#
# On Wayland a window cannot set its own icon: the desktop finds it through
# the desktop entry named after the app ID (org.postern.postern). Packages
# (AppImage, Flatpak, distribution packages) install these themselves; this
# is for running a development build.
#
#   app/linux/packaging/install-dev.sh [debug|release]   (default: debug)
#   app/linux/packaging/install-dev.sh --uninstall
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
app_id=org.postern.postern
data=${XDG_DATA_HOME:-$HOME/.local/share}
sizes=(32 48 64 128 256 512)

if [[ ${1:-} == --uninstall ]]; then
  rm -f "$data/applications/$app_id.desktop"
  for s in "${sizes[@]}"; do rm -f "$data/icons/hicolor/${s}x${s}/apps/$app_id.png"; done
  echo "removed Postern's desktop entry and icons"
  exit 0
fi

mode=${1:-debug}
binary="$here/../../build/linux/x64/$mode/bundle/postern"
binary=$(realpath -m "$binary")
[[ -x $binary ]] || { echo "no $mode build at $binary; run: flutter build linux --$mode" >&2; exit 1; }

for s in "${sizes[@]}"; do
  install -Dm644 "$here/icons/$s.png" "$data/icons/hicolor/${s}x${s}/apps/$app_id.png"
done
install -d "$data/applications"
sed "s|^Exec=.*|Exec=$binary|" "$here/$app_id.desktop" > "$data/applications/$app_id.desktop"
chmod 644 "$data/applications/$app_id.desktop"

# Refresh caches where the tools exist; desktops also notice on their own.
command -v update-desktop-database >/dev/null && update-desktop-database -q "$data/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$data/icons/hicolor" 2>/dev/null || true
command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 --noincremental >/dev/null 2>&1 || true

echo "installed: $data/applications/$app_id.desktop -> $binary"
