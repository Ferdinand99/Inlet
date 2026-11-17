#!/usr/bin/env bash
# Installs the Flatpak bundle for the current user and checks it.
#
#   packaging/linux/flatpak/smoke-test.sh <bundle> <version>
#
# Checks that the app runs inside the sandbox and prints its version, that the
# permissions are the narrow set the manifest asks for (no file system access, no
# system bus, no extra sockets), and that the desktop entry, the metainfo and the icon
# are exported. Needs flatpak, a running D-Bus session and a Flathub remote or an
# installed GNOME runtime (the bundle names Flathub as the place to get it).
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <bundle> <version>" >&2
  exit 2
fi

bundle=$(readlink -f "$1")
version=$2
app_id=no.opland.inlet

if [ ! -f "$bundle" ]; then
  echo "error: $bundle is missing" >&2
  exit 1
fi

flatpak uninstall --user -y --noninteractive "$app_id" >/dev/null 2>&1 || true
flatpak install --user -y --noninteractive --bundle "$bundle" >&2

out=$(flatpak run "$app_id" --version)
echo "Flatpak prints: $out"
if [ "$out" != "inlet $version" ]; then
  echo "error: expected 'inlet $version'" >&2
  exit 1
fi

# The permissions. Anything beyond this list is a widening that needs a decision.
perms=$(flatpak info --user --show-permissions "$app_id")
echo "$perms"
for line in 'shared=network;ipc;' 'devices=dri;'; do
  if ! printf '%s\n' "$perms" | grep -qxF "$line"; then
    echo "error: the permissions lack '$line'" >&2
    exit 1
  fi
done
# `x11` is stored next to `fallback-x11`; Flatpak gives the app X11 only without Wayland.
sockets=$(printf '%s\n' "$perms" | sed -n 's/^sockets=//p')
for socket in wayland fallback-x11 pulseaudio; do
  case ";$sockets" in
    *";$socket;"*) ;;
    *) echo "error: the permissions lack the socket $socket" >&2; exit 1 ;;
  esac
done
for socket in ${sockets//;/ }; do
  case "$socket" in
    x11 | wayland | fallback-x11 | pulseaudio) ;;
    *) echo "error: unexpected socket $socket" >&2; exit 1 ;;
  esac
done
if printf '%s\n' "$perms" | grep -qE '^(filesystems|features|persistent|environment|\[Environment|\[System Bus Policy)'; then
  echo "error: the permissions include file system, feature, environment or system bus access" >&2
  exit 1
fi
# The session bus: only these names, and only the tray name may be owned.
policy=$(printf '%s\n' "$perms" | sed -n '/^\[Session Bus Policy\]/,/^$/p' | grep '=' | sort)
expected=$(printf '%s\n' \
  'org.freedesktop.Notifications=talk' \
  'org.freedesktop.ScreenSaver=talk' \
  'org.freedesktop.secrets=talk' \
  'org.gnome.Mutter.IdleMonitor=talk' \
  'org.kde.StatusNotifierItem-2-1=own' \
  'org.kde.StatusNotifierWatcher=talk' | sort)
if [ "$policy" != "$expected" ]; then
  echo "error: the session bus policy differs from the expected one:" >&2
  diff <(printf '%s\n' "$expected") <(printf '%s\n' "$policy") >&2 || true
  exit 1
fi

# What the app exports to the host, and what is inside.
exports="$HOME/.local/share/flatpak/exports/share"
for f in "applications/$app_id.desktop" "metainfo/$app_id.metainfo.xml" \
         "icons/hicolor/256x256/apps/$app_id.png" "icons/hicolor/scalable/apps/$app_id.svg"; do
  test -e "$exports/$f" || { echo "error: the app does not export $f" >&2; exit 1; }
done
grep -qx 'MimeType=x-scheme-handler/inlet;' "$exports/applications/$app_id.desktop"
location=$(flatpak info --user --show-location "$app_id")
for f in files/bin/inlet files/share/licenses/$app_id/LICENSE files/share/licenses/$app_id/THIRD-PARTY-NOTICES; do
  test -e "$location/$f" || { echo "error: the app has no $f" >&2; exit 1; }
done

# The app starts in the sandbox and keeps running: a window under Xvfb for a few seconds.
# (timeout ends it with status 124; any other status means it stopped on its own.)
if command -v xvfb-run >/dev/null 2>&1; then
  status=0
  timeout 10 xvfb-run -a flatpak run "$app_id" >/dev/null 2>&1 || status=$?
  if [ "$status" -ne 124 ]; then
    echo "error: the app did not keep running in the sandbox (status $status)" >&2
    exit 1
  fi
  echo 'The app kept running for 10 s under Xvfb'
fi

echo 'Flatpak smoke test passed'
