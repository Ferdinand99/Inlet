#!/usr/bin/env bash
# Starts the AppImage under Xvfb and checks that it prints its version.
#
#   packaging/linux/smoke-test.sh <AppImage> <version>
#
# The AppImage is run with --appimage-extract-and-run, so the test needs neither FUSE
# nor a desktop. xvfb-run provides the virtual display the app's libraries expect.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <AppImage> <version>" >&2
  exit 2
fi

appimage=$(readlink -f "$1")
version=$2

if [ ! -x "$appimage" ]; then
  echo "error: $appimage is missing or not executable" >&2
  exit 1
fi
if ! command -v xvfb-run >/dev/null 2>&1; then
  echo "error: xvfb-run is not installed" >&2
  exit 1
fi

# The AppImage must be one executable file that carries a squashfs image.
file_type=$(head -c 4 "$appimage" | od -An -c | tr -d ' ')
if [ "$file_type" != '177ELF' ]; then
  echo "error: $appimage does not start with an ELF header" >&2
  exit 1
fi

out=$(APPIMAGE_EXTRACT_AND_RUN=1 xvfb-run -a "$appimage" --version)
echo "AppImage prints: $out"
if [ "$out" != "inlet $version" ]; then
  echo "error: expected 'inlet $version'" >&2
  exit 1
fi

# The packed files: the desktop entry, an icon and the licence.
list=$(mktemp -d)
trap 'rm -rf "$list"' EXIT
(cd "$list" && "$appimage" --appimage-extract >/dev/null)
root="$list/squashfs-root"
for f in no.opland.inlet.desktop no.opland.inlet.png AppRun usr/bin/inlet usr/bin/UpdateNix \
         usr/share/doc/inlet/LICENSE usr/share/icons/hicolor/256x256/apps/no.opland.inlet.png; do
  test -e "$root/$f" || { echo "error: the AppImage has no $f" >&2; exit 1; }
done
grep -qx 'MimeType=x-scheme-handler/inlet;' "$root/no.opland.inlet.desktop"
echo 'Linux package smoke test passed'
