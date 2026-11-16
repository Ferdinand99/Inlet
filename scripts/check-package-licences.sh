#!/usr/bin/env bash
# Checks that what ships carries the licences it must carry.
#
#   scripts/check-package-licences.sh --source            the repository: licence files next
#                                                          to the bundled fonts, icons and Twemoji
#   scripts/check-package-licences.sh <directory>         a directory of release files (or one
#                                                          package folder): LICENSE and
#                                                          THIRD-PARTY-NOTICES with the texts of
#                                                          the bundled fonts, icons and Twemoji,
#                                                          and the same two files inside every
#                                                          .tar.gz in it
#
# THIRD-PARTY-NOTICES holds the full text of the licence of every bundled font (SIL OFL
# 1.1, IBM Plex), icon (MIT, Phosphor) and emoji drawing (CC-BY-4.0, Twemoji); it is made
# by scripts/third-party-notices.sh from about.hbs. Every package carries that file and
# LICENSE: the Windows and Linux downloads next to the binary or inside the archive, the
# macOS app in Contents/Resources (packaging/macos/build-app.sh refuses to build without
# them). This script stops a release that lost one of them.
#
# Runs with the bash 3.2 that macOS ships.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
failed=0
fail() {
  echo "error: $*" >&2
  failed=1
}

# Each marker is a line that only the licence text itself contains.
check_notices() {
  local file=$1
  [ -s "$file" ] || { fail "$file is missing or empty"; return; }
  grep -qF 'SIL OPEN FONT LICENSE Version 1.1' "$file" || fail "$file lacks the licence of the IBM Plex fonts (SIL OFL 1.1)"
  grep -qF 'Phosphor Icons' "$file" || fail "$file lacks the credit for the Phosphor icons"
  grep -qF 'Copyright (c) 2023 Phosphor Icons' "$file" || fail "$file lacks the licence of the Phosphor icons (MIT)"
  grep -qF 'Twemoji graphics (Creative Commons Attribution 4.0 International, CC-BY-4.0)' "$file" || fail "$file lacks the credit for Twemoji"
  grep -qF 'Section 1 -- Definitions.' "$file" || fail "$file lacks the text of the Twemoji licence (CC-BY-4.0)"
}

check_dir() {
  local dir=$1
  [ -s "$dir/LICENSE" ] || fail "$dir/LICENSE is missing or empty"
  check_notices "$dir/THIRD-PARTY-NOTICES"
  local archive listing
  for archive in "$dir"/*.tar.gz; do
    [ -e "$archive" ] || continue
    listing=$(tar -tzf "$archive")
    for name in LICENSE THIRD-PARTY-NOTICES; do
      # Entries look like "LICENSE" or "./LICENSE" (or sit inside one top folder).
      if ! printf '%s\n' "$listing" | grep -Eq "^(\./)?([^/]+/)?$name\$"; then
        fail "$archive does not contain $name"
      fi
    done
    # The notices inside the archive are the ones that were checked above.
    local inner
    inner=$(mktemp)
    tar -xzOf "$archive" "$(printf '%s\n' "$listing" | grep -E '(^|/)THIRD-PARTY-NOTICES$' | head -n 1)" > "$inner"
    if ! cmp -s "$inner" "$dir/THIRD-PARTY-NOTICES"; then
      fail "THIRD-PARTY-NOTICES inside $archive differs from the one next to it"
    fi
    rm -f "$inner"
  done
}

case "${1:-}" in
  --source)
    for f in \
      crates/inlet-app/assets/fonts/LICENSE-IBM-PLEX.txt \
      crates/inlet-app/assets/icons/LICENSE-PHOSPHOR.txt \
      crates/inlet-emoji/LICENSE-TWEMOJI \
      LICENSE; do
      [ -s "$root/$f" ] || fail "$f is missing or empty"
    done
    check_notices "$root/THIRD-PARTY-NOTICES"
    ;;
  "" | -*)
    echo "usage: $0 --source | <directory>" >&2
    exit 2
    ;;
  *)
    [ -d "$1" ] || { echo "error: $1 is not a directory" >&2; exit 2; }
    check_dir "$1"
    ;;
esac

if [ "$failed" -ne 0 ]; then
  exit 1
fi
echo "licence files are in place"
