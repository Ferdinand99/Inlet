#!/usr/bin/env bash
# Tests the in-app update flow end to end against a fake release feed in a local folder.
#
#   scripts/docker-build.sh --exec packaging/velopack/update-test.sh
#
# (or directly on a Linux machine with a Rust toolchain). What it does:
#
#   1. Builds `inlet-update-probe`, a small program that runs Inlet's update code (the same
#      Checked source and update manager the app uses) against a folder.
#   2. Packs the probe as the app with Velopack, in the app's package id and channel:
#      version 0.9.1 (the installed copy), 0.9.2 (the update) and a few broken variants.
#   3. Runs the 0.9.1 AppImage against the folder: finds 0.9.2, downloads it, shows the
#      changelog, applies it, and checks that the AppImage then reports 0.9.2.
#   4. Checks what must not happen: no downgrade, no same-version "update", a package
#      whose hash does not match, a feed without SHA-256, another package id and another
#      channel are all refused and leave nothing waiting.
#
# Needs a .NET runtime (it is installed into a scratch folder when `dotnet` is missing; the
# installer script comes from dot.net), unzip and squashfs-tools (apt, when missing and run
# as root). Nothing is installed outside the scratch folder except those two packages.
# The scratch folder is removed at the end.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "ok: $*"; }

# ---- tools ---------------------------------------------------------------------------
missing=()
command -v unzip >/dev/null 2>&1 || missing+=(unzip)
command -v mksquashfs >/dev/null 2>&1 || missing+=(squashfs-tools)
if [ "${#missing[@]}" -gt 0 ]; then
  [ "$(id -u)" -eq 0 ] || fail "install ${missing[*]} first"
  apt-get update -qq >&2 && apt-get install -y -qq --no-install-recommends "${missing[@]}" >&2
fi
if ! command -v dotnet >/dev/null 2>&1; then
  curl -fsSL --retry 3 -o "$work/dotnet-install.sh" https://dot.net/v1/dotnet-install.sh
  bash "$work/dotnet-install.sh" --runtime dotnet --channel 8.0 --install-dir "$work/dotnet" >&2
  export DOTNET_ROOT="$work/dotnet" PATH="$work/dotnet:$PATH"
fi
export DOTNET_SYSTEM_GLOBALIZATION_INVARIANT=1

# ---- the probe -----------------------------------------------------------------------
cargo build --locked --profile ci -p inlet --features update-probe --bin inlet-update-probe >&2
probe="$root/target/ci/inlet-update-probe"
[ -x "$probe" ] || fail "no probe at $probe"
mkdir -p "$work/app"
cp "$probe" "$work/app/inlet"
strip "$work/app/inlet" 2>/dev/null || true

# ---- packages ------------------------------------------------------------------------
icon="$root/assets/brand/linux/hicolor/256x256/apps/no.opland.inlet.png"
pack() { # <version> <channel> <output dir> [notes]
  local notes_args=()
  if [ -n "${4:-}" ]; then
    printf '%s\n' "$4" > "$work/notes-$1.md"
    notes_args=(--releaseNotes "$work/notes-$1.md")
  fi
  "$root/packaging/velopack/vpk.sh" pack --packId no.opland.inlet --packVersion "$1" \
    --channel "$2" --packTitle Inlet --packAuthors test --packDir "$work/app" \
    --mainExe inlet --icon "$icon" --outputDir "$3" "${notes_args[@]}" >&2
}
CHANNEL=beta-linux
pack 0.9.0 "$CHANNEL" "$work/out-0.9.0" "Old release."
pack 0.9.1 "$CHANNEL" "$work/out-0.9.1" "Installed release."
pack 0.9.2 "$CHANNEL" "$work/out-0.9.2" "$(printf '### Features\n\n* **update:** the changelog shows in the settings ([abc1234](https://example.test/abc))\n')"
pack 0.9.1 stable-linux "$work/out-other-channel"

appimage_of() { find "$1" -maxdepth 1 -name '*.AppImage' | head -n 1; }
# The packages folder of an AppImage is the same for every copy (/var/tmp/velopack/<id>),
# so every fresh copy starts with it empty.
install_copy() { # <out dir> <name>: a fresh "installed" copy in its own folder
  rm -rf /var/tmp/velopack
  rm -rf "$work/install-$2" "$work/ext-$2"
  mkdir -p "$work/install-$2"
  cp "$(appimage_of "$1")" "$work/install-$2/inlet.AppImage"
  chmod +x "$work/install-$2/inlet.AppImage"
}
# There is no FUSE here, and an image run with --appimage-extract-and-run removes its files
# when it ends, which would pull the updater away. So the image is unpacked once and the
# program inside is run with APPIMAGE pointing at the image, which is how the AppImage
# runtime starts it.
run() { # <name> <args...>
  local name=$1
  shift
  if [ ! -d "$work/ext-$name" ]; then
    mkdir -p "$work/ext-$name"
    (cd "$work/ext-$name" && "$work/install-$name/inlet.AppImage" --appimage-extract >/dev/null)
  fi
  HOME="$work/home-$name" XDG_CONFIG_HOME="$work/home-$name/.config" \
    XDG_DATA_HOME="$work/home-$name/.local/share" APPIMAGE="$work/install-$name/inlet.AppImage" \
    "$work/ext-$name/squashfs-root/usr/bin/inlet" "$@"
}
feed_of() { # <out dir> <feed dir>: the folder a fake release would hold
  mkdir -p "$2"
  # The names release.yml collects (no.opland.inlet-*-<channel>-full.nupkg, releases.<channel>.json).
  cp "$1"/no.opland.inlet-*-"$CHANNEL"-full.nupkg "$1"/releases."$CHANNEL".json "$2/"
}

feed="$work/feed"
feed_of "$work/out-0.9.2" "$feed"

# ---- the update ----------------------------------------------------------------------
install_copy "$work/out-0.9.1" main
out=$(run main version)
[ "$out" = "version 0.9.1" ] || fail "installed version: $out"
ok "the installed copy is 0.9.1"

out=$(run main pending)
[ "$out" = "pending none" ] || fail "nothing should wait yet: $out"

out=$(run main check "$feed")
echo "$out"
echo "$out" | grep -qx 'ready 0.9.2' || fail "no update found: $out"
echo "$out" | grep -q '^notes .*Features.*the changelog shows in the settings (abc1234)' \
  || fail "the changelog is missing or still has links: $out"
echo "$out" | grep -q 'example.test' && fail "a link address leaked into the changelog"
ok "0.9.2 was found, downloaded and its changelog is plain text"

out=$(run main pending)
[ "$out" = "pending 0.9.2" ] || fail "the update should wait: $out"
ok "the downloaded update waits for a restart"

# Apply: the updater replaces the AppImage; the file then reports the new version.
run main apply >/dev/null
for _ in $(seq 1 60); do
  rm -rf "$work/ext-main"
  out=$(run main version 2>/dev/null || true)
  [ "$out" = "version 0.9.2" ] && break
  sleep 1
done
[ "$out" = "version 0.9.2" ] || fail "after the update the app reports: $out"
ok "applying the update left 0.9.2 installed"

# ---- what must not happen ------------------------------------------------------------
expect_nothing() { # <name> <feed dir> <what>
  local out
  out=$(run "$1" check "$2" 2>&1 || true)
  echo "$out" | grep -q '^ready' && fail "$3: an update was accepted: $out"
  out=$(run "$1" pending)
  [ "$out" = "pending none" ] || fail "$3: something waits: $out"
  ok "$3"
}

# A newer copy never goes back to an older release.
install_copy "$work/out-0.9.1" downgrade
feed_of "$work/out-0.9.0" "$work/feed-old"
out=$(run downgrade check "$work/feed-old")
echo "$out" | grep -qx 'up-to-date 0.9.1' || fail "no downgrade: $out"
ok "an older release is not offered (no downgrade)"

feed_of "$work/out-0.9.1" "$work/feed-same"
out=$(run downgrade check "$work/feed-same")
echo "$out" | grep -qx 'up-to-date 0.9.1' || fail "same version: $out"
ok "the same version is not an update"

# A package that is not what the feed says (one byte changed, same size).
install_copy "$work/out-0.9.1" tamper
feed_of "$work/out-0.9.2" "$work/feed-tampered"
pkg=$(find "$work/feed-tampered" -name '*-full.nupkg')
size=$(stat -c %s "$pkg")
printf '\xff' | dd of="$pkg" bs=1 seek=$((size / 2)) conv=notrunc status=none
out=$(run tamper check "$work/feed-tampered" 2>&1 || true)
echo "$out" | grep -q '^error' || fail "tampered package was accepted: $out"
echo "$out" | grep -qi 'checksum' || fail "expected a checksum error: $out"
expect_nothing tamper "$work/feed-tampered" "a package with a wrong hash is refused"

# A feed that names no SHA-256, or another package.
install_copy "$work/out-0.9.1" nohash
feed_of "$work/out-0.9.2" "$work/feed-nohash"
sed -i 's/"SHA256": *"[0-9a-fA-F]*"/"SHA256": ""/I' "$work/feed-nohash/releases.$CHANNEL.json"
grep -q '"SHA256": *""' "$work/feed-nohash/releases.$CHANNEL.json" || fail "test setup: SHA256 not blanked"
expect_nothing nohash "$work/feed-nohash" "an entry without SHA-256 is refused"

install_copy "$work/out-0.9.1" otherid
feed_of "$work/out-0.9.2" "$work/feed-otherid"
sed -i 's/"PackageId": *"no.opland.inlet"/"PackageId": "other.app"/I' "$work/feed-otherid/releases.$CHANNEL.json"
grep -q 'other.app' "$work/feed-otherid/releases.$CHANNEL.json" || fail "test setup: PackageId not changed"
expect_nothing otherid "$work/feed-otherid" "an entry for another package is refused"

# A copy packed for another channel does not update at all.
install_copy "$work/out-other-channel" otherchan
out=$(run otherchan check "$feed" 2>&1 || true)
echo "$out" | grep -q 'not offered for the channel' || fail "other channel: $out"
expect_nothing otherchan "$feed" "a copy on another channel is not offered updates"

echo "Update test passed"
