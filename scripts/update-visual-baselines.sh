#!/usr/bin/env bash
# Takes the screenshots of a CI run as the new baselines of the visual checks.
#
#   scripts/update-visual-baselines.sh [--other-run] <run id> [windows|macos|linux ...]
#
# Use it when a change to the look is meant: open the run's artifacts visual-<os>
# first (report.md, and the red pixels in diff/) and make sure every difference is
# wanted. The script shows where the run comes from (repository, branch, commit,
# workflow), downloads those artifacts with the GitHub CLI, copies their actual/*.png
# (already cut to the window and compressed by inlet-visual) to
# tests/visual/baseline/<os>/, and runs oxipng over them when it is installed. Scenes
# whose result in the run's report.md is "same" are skipped, so a baseline PR holds only
# real changes. It commits nothing: look at the new pictures, then commit them with the
# change that caused them.
#
# It refuses a run from a fork, from another workflow than build.yml, from another
# branch than the one checked out, or of another commit than HEAD, unless --other-run
# is given. It changes nothing when an artifact you asked for is missing.
#
# Baselines always come from CI, from the same pinned runner images that compare
# them; screenshots from another machine would differ in fonts and rendering.
# The run id is in the URL of the workflow run (…/actions/runs/<id>), or:
#   gh run list --workflow build.yml --branch <branch>
set -euo pipefail
cd "$(dirname "$0")/.."

usage() { sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }
other_run=0
args=()
for a in "$@"; do
  case "$a" in
    --other-run) other_run=1 ;;
    -h | --help) usage ;;
    *) args+=("$a") ;;
  esac
done
[ ${#args[@]} -ge 1 ] || usage
run="${args[0]}"
case "$run" in "" | *[!0-9]*) echo "run id must be a number" >&2; exit 2 ;; esac
oses=("${args[@]:1}")
[ ${#oses[@]} -gt 0 ] || oses=(windows macos linux)
for os in "${oses[@]}"; do
  case "$os" in windows | macos | linux) ;; *) echo "unknown platform $os" >&2; exit 2 ;; esac
done

# Where the run comes from.
repo="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
IFS=$'\t' read -r head_repo head_branch head_sha workflow conclusion < <(
  gh api "repos/$repo/actions/runs/$run" \
    --jq '[.head_repository.full_name, .head_branch, .head_sha, .path, (.conclusion // .status)] | @tsv'
)
branch="$(git rev-parse --abbrev-ref HEAD)"
head="$(git rev-parse HEAD)"
echo "run $run of $repo: $workflow ($conclusion)"
echo "  from $head_repo, branch $head_branch, commit $head_sha"
echo "  checked out here: branch $branch, commit $head"
problems=()
[ "$head_repo" = "$repo" ] || problems+=("the run comes from the fork $head_repo")
[ "$workflow" = ".github/workflows/build.yml" ] || problems+=("the run is of $workflow, not build.yml")
[ "$head_branch" = "$branch" ] || problems+=("the run is of branch $head_branch, not $branch")
[ "$head_sha" = "$head" ] || problems+=("the run is of commit $head_sha, not HEAD")
if [ ${#problems[@]} -gt 0 ]; then
  printf '  %s\n' "${problems[@]}" >&2
  if [ "$other_run" = 1 ]; then
    echo "--other-run: taking the screenshots anyway" >&2
  else
    echo "refused; pass --other-run if this is really the run you want" >&2
    exit 1
  fi
fi

# Everything first, so a missing artifact changes nothing.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
for os in "${oses[@]}"; do
  if ! gh run download "$run" -R "$repo" -n "visual-$os" -D "$tmp/$os"; then
    echo "no artifact visual-$os in run $run; nothing changed" >&2
    exit 1
  fi
  if ! compgen -G "$tmp/$os/actual/*.png" >/dev/null; then
    echo "visual-$os has no screenshots; nothing changed" >&2
    exit 1
  fi
done
# The scenes whose comparison result in report.md is "same" are left alone: their
# picture is pixel for pixel the baseline already, and copying it would only rewrite
# the file (another compression, another commit entry) in a baseline PR. A table row is
# "| <scene> | <result> | ..."; a scene that loosens its tolerance has a note after its
# name. A run without a report.md (an older run) takes every picture.
unchanged_scenes() {
  [ -f "$1" ] || return 0
  awk -F'|' '
    /^\| / {
      name = $2; result = $3
      sub(/^ +/, "", name); sub(/ \(tolerance loosened.*$/, "", name); sub(/ +$/, "", name)
      gsub(/^ +| +$/, "", result)
      if (result == "same") print name
    }' "$1"
}
copied=()
for os in "${oses[@]}"; do
  mkdir -p "tests/visual/baseline/$os"
  same="$(unchanged_scenes "$tmp/$os/report.md")"
  taken=0
  skipped=0
  for picture in "$tmp/$os"/actual/*.png; do
    scene="$(basename "$picture" .png)"
    if printf '%s\n' "$same" | grep -Fxq -- "$scene"; then
      skipped=$((skipped + 1))
      continue
    fi
    cp "$picture" "tests/visual/baseline/$os/"
    copied+=("tests/visual/baseline/$os/$scene.png")
    taken=$((taken + 1))
  done
  echo "$os: $taken baselines from run $run ($skipped unchanged, left alone)"
done
# Lossless, about a sixth smaller again; the pictures stay valid without it. Only the
# pictures just copied: the others keep their bytes.
if [ ${#copied[@]} -eq 0 ]; then
  echo "nothing changed: every scene already matches its baseline"
elif command -v oxipng >/dev/null 2>&1; then
  oxipng -q -o 4 --strip safe "${copied[@]}"
else
  echo "oxipng not found: the baselines are not compressed further (optional)" >&2
fi
git status --short tests/visual/baseline
