#!/usr/bin/env bash
# Runs Velopack's packer (vpk) at a pinned version.
#
#   packaging/velopack/vpk.sh pack --packId ... (any vpk arguments)
#
# vpk is a .NET tool. The first call downloads its package from the Velopack release,
# checks the SHA-256 below and unpacks it into $VPK_HOME (default: a folder in the
# temporary directory); later calls reuse that. The package carries every file vpk
# needs, so nothing else is fetched. To move to another version, change both lines
# below: the SHA-256 is the one GitHub shows for the release asset `vpk.<version>.nupkg`
# of https://github.com/velopack/velopack/releases.
#
# The unpacking happens in a scratch folder that is renamed to $VPK_HOME only when it
# is complete, so an interrupted or concurrent run never leaves a half-unpacked tool
# behind.
#
# Needs a .NET runtime of version 8 or later (the runners have one) and curl.
set -euo pipefail

VPK_VERSION=1.2.161
VPK_SHA256=2b56ce117f803fc70c103cb423bd040e395e40370f6ff6e10818e9ff9c26a323

home=${VPK_HOME:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/vpk-$VPK_VERSION}
tool="$home/tools/net8.0/any/vpk.dll"

if [ ! -f "$tool" ]; then
  scratch="$home.part.$$"
  rm -rf "$scratch"
  mkdir -p "$scratch"
  trap 'rm -rf "$scratch"' EXIT
  nupkg="$scratch.zip"
  curl -fsSL --retry 3 -o "$nupkg" \
    "https://github.com/velopack/velopack/releases/download/$VPK_VERSION/vpk.$VPK_VERSION.nupkg"
  echo "$VPK_SHA256  $nupkg" | sha256sum -c - >&2
  if command -v unzip >/dev/null 2>&1; then
    unzip -q -o "$nupkg" -d "$scratch"
  elif command -v 7z >/dev/null 2>&1; then
    7z x -y -o"$scratch" "$nupkg" >/dev/null
  else
    # Git Bash on a Windows runner has neither; PowerShell can unpack a .zip. The paths
    # go in through the environment so no quoting has to survive the shell.
    NUPKG=$(cygpath -w "$nupkg") DEST=$(cygpath -w "$scratch") powershell -NoProfile -Command \
      'Expand-Archive -LiteralPath $env:NUPKG -DestinationPath $env:DEST -Force'
  fi
  rm -f "$nupkg"
  test -f "$scratch/tools/net8.0/any/vpk.dll" || {
    echo "error: vpk.dll is missing after unpacking" >&2
    exit 1
  }
  # Another run may have finished first; then its copy is as good as ours.
  mv "$scratch" "$home" 2>/dev/null || test -f "$tool"
fi

# The tool is built for .NET 8; let a newer runtime run it when 8 is not installed.
export DOTNET_ROLL_FORWARD=Major
export DOTNET_NOLOGO=1
export DOTNET_CLI_TELEMETRY_OPTOUT=1
# dotnet on Windows needs Windows paths, not the /d/a/... form Git Bash uses.
if command -v cygpath >/dev/null 2>&1; then tool=$(cygpath -m "$tool"); fi
# Not exec: the exit trap above has to run.
dotnet "$tool" "$@"
