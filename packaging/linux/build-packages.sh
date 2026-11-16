#!/usr/bin/env bash
# Builds the Linux packages from a release binary.
#
#   packaging/linux/build-packages.sh <path to the inlet binary> <output directory>
#
# Writes, with the workspace version from Cargo.toml:
#
#   inlet-<version>-linux-x86_64.AppImage   Velopack's Linux package: an AppImage that
#                                           carries the updater the later auto-update
#                                           work uses
#   inlet-<version>-linux-x86_64.tar.gz     the binary, LICENSE and THIRD-PARTY-NOTICES
#   velopack/                               the update feed inputs (see below)
#
# The AppImage is assembled from the files in this folder: the desktop entry
# no.opland.inlet.desktop (the app id is no.opland.inlet, which the desktop matches to
# the window's icon), the icons from assets/brand/linux and AppRun. Velopack
# packs the finished AppDir with its own AppImage runtime and adds its updater.
#
# The velopack/ folder holds what the later auto-update work publishes next to the
# release: the full package (`*-full.nupkg`) and `releases.linux.json`. Adding
# auto-update only means keeping those files; it does not change how the packages are
# built.
#
# Needs bash, awk, tar, squashfs-tools (mksquashfs) and a .NET runtime of version 8 or
# later, which the runner has.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <inlet binary> <output directory>" >&2
  exit 2
fi

binary=$1
out_dir=$2
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)

if [ ! -f "$binary" ]; then
  echo "error: binary not found: $binary" >&2
  exit 1
fi

# The version comes from [workspace.package] in the root Cargo.toml.
version=$(awk '
  /^\[/ { in_section = ($0 == "[workspace.package]") ; next }
  in_section && $1 == "version" {
    sub(/^[^"]*"/, ""); sub(/".*$/, ""); print; exit
  }
' "$root/Cargo.toml")
if [ -z "$version" ]; then
  echo "error: no version in [workspace.package] of $root/Cargo.toml" >&2
  exit 1
fi

icons="$root/assets/brand/linux/hicolor"
for f in "$here/no.opland.inlet.desktop" "$here/AppRun" "$icons/256x256/apps/no.opland.inlet.png" \
         "$icons/scalable/apps/no.opland.inlet.svg" "$root/LICENSE" "$root/THIRD-PARTY-NOTICES"; do
  if [ ! -f "$f" ]; then
    echo "error: missing $f" >&2
    exit 1
  fi
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
appdir="$work/Inlet.AppDir"
feed="$work/feed"
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" "$appdir/usr/share/doc/inlet" \
         "$feed" "$out_dir"

install -m 755 "$binary" "$appdir/usr/bin/inlet"
install -m 755 "$here/AppRun" "$appdir/AppRun"
install -m 644 "$here/no.opland.inlet.desktop" "$appdir/no.opland.inlet.desktop"
install -m 644 "$here/no.opland.inlet.desktop" "$appdir/usr/share/applications/no.opland.inlet.desktop"
install -m 644 "$root/LICENSE" "$root/THIRD-PARTY-NOTICES" "$appdir/usr/share/doc/inlet/"

# Icons: the 256 px PNG is the image's own icon (what file managers show); the full
# set goes where desktops look for an installed app's icon.
install -m 644 "$icons/256x256/apps/no.opland.inlet.png" "$appdir/no.opland.inlet.png"
install -m 644 "$icons/256x256/apps/no.opland.inlet.png" "$appdir/.DirIcon"
(cd "$icons" && find . -type f -path '*/apps/no.opland.inlet.*') | while IFS= read -r f; do
  mkdir -p "$appdir/usr/share/icons/hicolor/$(dirname "$f")"
  install -m 644 "$icons/$f" "$appdir/usr/share/icons/hicolor/$f"
done

# Velopack packs a folder named *.AppDir as it is, so these files are what ships.
"$root/packaging/velopack/vpk.sh" pack \
  --packId no.opland.inlet \
  --packVersion "$version" \
  --packTitle Inlet \
  --packAuthors "Inlet contributors" \
  --packDir "$appdir" \
  --mainExe inlet \
  --outputDir "$feed"

ls -l "$feed" >&2

appimage=$(find "$feed" -maxdepth 1 -name '*.AppImage' | head -n 1)
full=$(find "$feed" -maxdepth 1 -name '*-full.nupkg' | head -n 1)
for f in "$appimage" "$full"; do
  if [ -z "$f" ] || [ ! -s "$f" ]; then
    echo "error: vpk did not write an AppImage and a full package" >&2
    exit 1
  fi
done

cp "$appimage" "$out_dir/inlet-$version-linux-x86_64.AppImage"
chmod 755 "$out_dir/inlet-$version-linux-x86_64.AppImage"
mkdir -p "$out_dir/velopack"
find "$feed" -maxdepth 1 -type f \( -name '*.nupkg' -o -name '*.json' -o -name 'RELEASES*' \) \
  -exec cp {} "$out_dir/velopack/" \;

# The tarball: the binary and the licence files, as before.
tar_stage="$work/tar"
mkdir -p "$tar_stage"
install -m 755 "$binary" "$tar_stage/inlet"
install -m 644 "$root/LICENSE" "$root/THIRD-PARTY-NOTICES" "$tar_stage/"
tar -czf "$out_dir/inlet-$version-linux-x86_64.tar.gz" -C "$tar_stage" inlet THIRD-PARTY-NOTICES LICENSE

echo "Built the Linux packages for version $version in $out_dir" >&2
