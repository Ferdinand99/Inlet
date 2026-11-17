#!/usr/bin/env bash
# Prints the changelog section of one version, for `vpk pack --releaseNotes`.
#
#   packaging/velopack/release-notes.sh <version> [CHANGELOG.md]
#
# release-please writes CHANGELOG.md with one `## [X.Y.Z](compare link) (date)` heading per
# release. The section is what the in-app updater shows as "What's new". Prints nothing
# (and still succeeds) when the file or the version is missing, as on a build between
# releases; the caller then packs without notes.
set -euo pipefail

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  echo "usage: $0 <version> [CHANGELOG.md]" >&2
  exit 2
fi

version=$1
changelog=${2:-$(cd "$(dirname "$0")/../.." && pwd)/CHANGELOG.md}
[ -f "$changelog" ] || exit 0

awk -v version="$version" '
  /^## / {
    if (found) exit
    # "## [0.2.0](...) (date)" or "## 0.2.0 (date)"
    heading = $0
    sub(/^## \[?/, "", heading)
    sub(/[] (].*$/, "", heading)
    found = (heading == version)
    next
  }
  found && (NF || started) { started = 1; print }
' "$changelog" | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}' | tr -d '\r'
