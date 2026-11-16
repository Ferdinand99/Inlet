#!/usr/bin/env bash
# Builds the Windows packages from a release binary with Velopack.
#
#   packaging/windows/build-packages.sh <path to inlet.exe> <output directory>
#
# The web view helper, inlet-web.exe, must be in the same folder as inlet.exe; it goes
# next to inlet.exe in both packages.
#
# Writes, with the workspace version from Cargo.toml:
#
#   inlet-<version>-windows-x86_64-setup.exe   installer; per user, no admin rights
#   inlet-<version>-windows-x86_64.zip         portable copy
#   velopack/                                  the update feed inputs (see below)
#
# Setup.exe installs into %LocalAppData%\no.opland.inlet, adds a Start menu shortcut
# and an entry in "Apps & features", and starts no admin prompt. Nothing is signed.
#
# The velopack/ folder holds what the later auto-update work publishes next to the
# release: the full package (`*-full.nupkg`) and `releases.win.json`. Velopack builds
# deltas and the next feed from the previous release's files, so adding auto-update
# only means keeping those files; it does not change how the packages are built.
#
# Runs in Git Bash on a Windows runner (and in any bash with .NET 8 or later).
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <inlet.exe> <output directory>" >&2
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
helper="$(dirname "$binary")/inlet-web.exe"
if [ ! -s "$helper" ]; then
  echo "error: the web view helper is missing: $helper" >&2
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

for f in "$root/assets/brand/windows/inlet.ico" "$root/LICENSE" "$root/THIRD-PARTY-NOTICES"; do
  if [ ! -f "$f" ]; then
    echo "error: missing $f" >&2
    exit 1
  fi
done

# dotnet on Windows needs Windows paths, not the /d/a/... form Git Bash uses.
native() {
  if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s' "$1"; fi
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
stage="$work/app"
feed="$work/feed"
mkdir -p "$stage" "$feed" "$out_dir"

cp "$binary" "$stage/inlet.exe"
cp "$helper" "$stage/inlet-web.exe"
cp "$root/LICENSE" "$stage/LICENSE"
cp "$root/THIRD-PARTY-NOTICES" "$stage/THIRD-PARTY-NOTICES"

# --runtime: the binary is x86_64 only; without it vpk warns and guesses.
# --shortcuts: the Start menu only, no desktop shortcut.
# --aumid: the shortcut carries the same AppUserModelID as the running app (see
#   inlet_platform::APP_ID), so notifications and taskbar grouping match; Velopack's
#   default would be "velopack.no.opland.inlet".
# No signing options: the files are not code-signed yet.
"$root/packaging/velopack/vpk.sh" pack \
  --packId no.opland.inlet \
  --packVersion "$version" \
  --packTitle Inlet \
  --packAuthors "Inlet contributors" \
  --packDir "$(native "$stage")" \
  --mainExe inlet.exe \
  --icon "$(native "$root/assets/brand/windows/inlet.ico")" \
  --runtime win-x64 \
  --shortcuts StartMenuRoot \
  --aumid no.opland.inlet \
  --outputDir "$(native "$feed")"

ls -l "$feed" >&2

setup=$(find "$feed" -maxdepth 1 -name '*Setup.exe' | head -n 1)
portable=$(find "$feed" -maxdepth 1 -name '*Portable.zip' | head -n 1)
full=$(find "$feed" -maxdepth 1 -name '*-full.nupkg' | head -n 1)
for f in "$setup" "$portable" "$full"; do
  if [ -z "$f" ] || [ ! -s "$f" ]; then
    echo "error: vpk did not write a Setup.exe, a Portable.zip and a full package" >&2
    exit 1
  fi
done

cp "$setup" "$out_dir/inlet-$version-windows-x86_64-setup.exe"
cp "$portable" "$out_dir/inlet-$version-windows-x86_64.zip"
mkdir -p "$out_dir/velopack"
find "$feed" -maxdepth 1 -type f \( -name '*.nupkg' -o -name '*.json' -o -name 'RELEASES*' \) \
  -exec cp {} "$out_dir/velopack/" \;

echo "Built the Windows packages for version $version in $out_dir" >&2
