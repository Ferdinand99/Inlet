#!/usr/bin/env bash
# Builds the Flatpak bundle from a release binary.
#
#   packaging/linux/flatpak/build-flatpak.sh <path to the inlet binary> <output directory>
#
# Writes inlet-<version>-linux-x86_64.flatpak, with the workspace version from Cargo.toml.
# When the embedded web helper `inlet-web` sits next to the binary, it goes into the
# bundle too.
#
# The binary is packed as it is (see flatpak/no.opland.inlet.yml): the script stages it
# with the desktop entry, the metainfo (stamped with the version and the commit date),
# the icons and the licence files, validates the metadata, and runs flatpak-builder on
# the staged folder. The GNOME runtime and SDK come from Flathub.
#
# Needs bash, awk, git, flatpak, flatpak-builder, appstreamcli (with the SVG pixbuf loader,
# librsvg2-common, which its icon check uses) and, optionally,
# desktop-file-validate. On a runner whose kernel restricts unprivileged user namespaces
# (Ubuntu 24.04), flatpak's sandbox needs the AppArmor restriction lifted first; the
# workflow does that.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <inlet binary> <output directory>" >&2
  exit 2
fi

binary=$1
out_dir=$2
here=$(cd "$(dirname "$0")" && pwd)
linux_dir=$(cd "$here/.." && pwd)
root=$(cd "$here/../../.." && pwd)
app_id=no.opland.inlet

if [ ! -f "$binary" ]; then
  echo "error: binary not found: $binary" >&2
  exit 1
fi
binary=$(cd "$(dirname "$binary")" && pwd)/$(basename "$binary")
helper="$(dirname "$binary")/inlet-web"

for tool in flatpak flatpak-builder appstreamcli; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: $tool is not installed" >&2
    exit 1
  fi
done

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

# The release date: the commit's date (SOURCE_DATE_EPOCH wins), else today.
epoch=${SOURCE_DATE_EPOCH:-$(git -C "$root" log -1 --format=%ct 2>/dev/null || true)}
epoch=${epoch:-$(date +%s)}
date=$(date -u -d "@$epoch" +%Y-%m-%d)

icons="$root/assets/brand/linux/hicolor"
for f in "$linux_dir/$app_id.desktop" "$linux_dir/$app_id.metainfo.xml" "$here/$app_id.yml" \
         "$icons/256x256/apps/$app_id.png" "$icons/scalable/apps/$app_id.svg" \
         "$root/LICENSE" "$root/THIRD-PARTY-NOTICES"; do
  if [ ! -f "$f" ]; then
    echo "error: missing $f" >&2
    exit 1
  fi
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
stage="$work/stage"
mkdir -p "$stage/icons" "$out_dir"

install -m 755 "$binary" "$stage/inlet"
if [ -f "$helper" ]; then
  install -m 755 "$helper" "$stage/inlet-web"
  echo "Packing the web helper $helper" >&2
fi
install -m 644 "$linux_dir/$app_id.desktop" "$stage/$app_id.desktop"
install -m 644 "$root/LICENSE" "$root/THIRD-PARTY-NOTICES" "$stage/"
cp -r "$icons" "$stage/icons/hicolor"
install -m 644 "$here/$app_id.yml" "$stage/$app_id.yml"

# Stamp the release entry. The checked-in file carries a placeholder version and date.
sed -E "s#<release version=\"[^\"]*\" date=\"[^\"]*\"#<release version=\"$version\" date=\"$date\"#" \
  "$linux_dir/$app_id.metainfo.xml" > "$stage/$app_id.metainfo.xml"
if ! grep -qF "<release version=\"$version\" date=\"$date\"" "$stage/$app_id.metainfo.xml"; then
  echo "error: could not stamp the release entry in the metainfo" >&2
  exit 1
fi

# The checks fail the build: the metadata must be valid before it ships. The screenshot
# URL is not fetched (no network in the check).
appstreamcli validate --no-net --explain "$stage/$app_id.metainfo.xml" >&2
if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "$stage/$app_id.desktop" >&2
fi

# The runtime and SDK come from Flathub (for the current user only).
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo

build="$work/build"
repo="$work/repo"
if ! flatpak-builder --user --force-clean --disable-rofiles-fuse \
  --state-dir="$work/state" --install-deps-from=flathub --default-branch=stable \
  --repo="$repo" "$build" "$stage/$app_id.yml" >&2; then
  # flatpak-builder only says that the metadata compose failed; show why.
  echo "flatpak-builder failed; what the metadata compose sees:" >&2
  id >&2 || true
  ls -lAR "$build/files/share" >&2 || true
  appstreamcli compose --verbose --prefix=/ --origin=flatpak \
    --print-report=full --components="$app_id,$app_id.desktop" --data-dir="$work/diag/xmls" \
    --icons-dir="$work/diag/icons" --result-root="$work/diag" "$build/files" >&2 || true
  exit 1
fi

bundle="$out_dir/inlet-$version-linux-x86_64.flatpak"
flatpak build-bundle "$repo" "$bundle" "$app_id" stable \
  --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo >&2

ls -l "$bundle" >&2
echo "Built the Flatpak bundle for version $version in $out_dir" >&2
