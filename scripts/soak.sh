#!/usr/bin/env bash
# The soak: the app runs on a recorded (synthetic) gateway under Xvfb, and its memory is
# read all the way. Checks the targets for long runs: under 150 MiB resident with three
# accounts and 30 open channels, no growth after the sixth (virtual) hour, and the time
# to the first frame.
#
# Runs in the builder image, started the way the other heavy jobs are:
#
#   scripts/docker-build.sh --exec scripts/soak.sh 24h            a day in real time (the night run)
#   scripts/docker-build.sh --exec scripts/soak.sh 24h 30m        a day played in 30 minutes (48 times faster)
#
# The settings below reach the script only through INLET_PASS_ENV, which names the
# variables docker-build.sh passes into the container, for example
#   INLET_PASS_ENV="SOAK_PHASES SOAK_RATE" SOAK_RATE=1 scripts/docker-build.sh --exec scripts/soak.sh 24h
#
# Set INLET_CPUS=2 INLET_MEMORY=4g INLET_EXPORT_ALWAYS=1 for the night run: the samples
# and logs are handed back in target/build-out/soak (without INLET_EXPORT_ALWAYS only
# after a run that passed). The summary goes to stderr; the exit status is 0 when every
# target held.
#
# Arguments: <recording length> [<time it takes>]. A number with h, m or s (24h, 90m,
# 600s). The second defaults to the first (real time).
#
# Phases (SOAK_PHASES, default "tests startup app"):
#   tests    (built with the ci profile at opt-level 3: the dev-dependencies cannot go in a
#            release build) the soak tests of the crate: a day of gateway traffic through the account
#            states (memory read after every virtual hour), and the scrolling frame times
#            in a channel with 5,000 messages (with INLET_SOAK_SCROLL_PAGES=100 it walks
#            the whole history)
#   startup  five starts of the app, time to the first frame
#   app      the long run
#
# SOAK_RATE says how busy the recording is (default 24). The message windows hold up to
# 500 messages each and take days to fill at a lively 1, so 24 fills them within
# six hours: what a full set of 30 windows weighs, and that it stays there.
# SOAK_STEADY_H is the virtual hour the growth check starts at (default 15). At rate 24 the
# windows take about twelve hours to fill, so the memory is flat only after that; at
# SOAK_RATE=60 they fill by hour 6 and SOAK_STEADY_H=6 holds.
# Other settings: SOAK_SAMPLE_SECS (seconds between memory samples; default a 120th of
# the run, between 5 and 60), SOAK_NAV_MS (milliseconds between two channel switches,
# default 5000, divided by the speed-up for a faster run, at least 250; 0 = nobody switches
# channels), SOAK_SEED, SOAK_OUT (the folder name under target/build-out, default soak).
#
# The binary is copied out of the shared build cache before the run starts and the
# build slot is given back then, so a night run does not keep other builds waiting.
set -uo pipefail

usage() { sed -n '2,/^set -uo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2; }
[ "$#" -ge 1 ] && [ "$1" != -h ] && [ "$1" != --help ] || { usage; exit 2; }

secs() {
  case "$1" in
    *h) echo $(( ${1%h} * 3600 )) ;;
    *m) echo $(( ${1%m} * 60 )) ;;
    *s) echo $(( ${1%s} )) ;;
    *[!0-9]*) echo "soak.sh: cannot read the duration $1" >&2; exit 2 ;;
    *) echo "$1" ;;
  esac
}
virtual_secs=$(secs "$1")
wall_secs=$(secs "${2:-$1}")
[ "$virtual_secs" -gt 0 ] && [ "$wall_secs" -gt 0 ] || { echo "soak.sh: the durations must be above zero" >&2; exit 2; }
speed=$(awk -v v="$virtual_secs" -v w="$wall_secs" 'BEGIN { printf "%.4f", v / w }')
hours=$(awk -v v="$virtual_secs" 'BEGIN { printf "%.4f", v / 3600 }')
phases="${SOAK_PHASES:-tests startup app}"
seed="${SOAK_SEED:-1}"
rate="${SOAK_RATE:-24}"
steady="${SOAK_STEADY_H:-15}"
nav_ms=$(awk -v n="${SOAK_NAV_MS:-5000}" -v s="$speed" 'BEGIN { x = n / (s > 1 ? s : 1); if (n > 0 && x < 250) x = 250; printf "%d", x }')
sample=${SOAK_SAMPLE_SECS:-$(( wall_secs / 120 < 5 ? 5 : (wall_secs / 120 > 60 ? 60 : wall_secs / 120) ))}

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
out="/out/${SOAK_OUT:-soak}"
mkdir -p "$out" /tmp/soak
say() { echo "soak: $*" >&2; }
failed=""
summary="$out/summary.txt"
: > "$summary"
note() { echo "$*" | tee -a "$summary" >&2; }

say "recording $virtual_secs s, played in $wall_secs s (x$speed), sample every $sample s, channel switch every $nav_ms ms"

# ---- Build ------------------------------------------------------------------------------
need_binary=0
case " $phases " in *" startup "*|*" app "*) need_binary=1 ;; esac
bin=/tmp/soak/inlet
if [ "$need_binary" = 1 ]; then
  say "building the demo binary (release)"
  cargo build --release --locked -p inlet --features demo || { say "the build failed"; exit 1; }
  cp target/release/inlet "$bin" || exit 1
fi

# ---- Phase: tests -----------------------------------------------------------------------
if [[ " $phases " == *" tests "* ]]; then
  say "phase tests"
  INLET_SOAK_SCROLL_PAGES="${INLET_SOAK_SCROLL_PAGES:-100}" \
  INLET_SOAK_TEST_RATE="$rate" INLET_SOAK_TEST_STEADY_HOUR="$steady" INLET_SOAK_TEST_HOURS="$(awk -v h="$hours" 'BEGIN { printf "%d", h < 1 ? 1 : h }')" \
    CARGO_PROFILE_CI_OPT_LEVEL=3 cargo test --profile ci --locked -p inlet --features demo --lib soak -- --nocapture --include-ignored --test-threads=1 \
    2>&1 | tee "$out/tests.log" >&2
  if [ "${PIPESTATUS[0]}" -ne 0 ]; then failed="$failed tests"; fi
  note "--- tests"
  grep -E '^(scroll|soak test|caps):' "$out/tests.log" | tee -a "$summary" >&2
fi

# The slot is given back now (the lock is shared with the entry script by file
# descriptor 8), as the binary is a copy: a night run must not keep builds waiting.
if [ "$need_binary" = 1 ] && [ -e /proc/$$/fd/8 ]; then
  flock -u 8 2>/dev/null && say "build slot given back"
fi

[ "$need_binary" = 1 ] || { [ -z "$failed" ] && exit 0 || exit 1; }

# ---- Xvfb -------------------------------------------------------------------------------
Xvfb :99 -screen 0 1280x860x24 -dpi 96 >/dev/null 2>&1 &
xvfb=$!
export DISPLAY=:99 GPUI_X11_SCALE_FACTOR=1
sleep 2
trap 'kill "$xvfb" 2>/dev/null; [ -n "${app:-}" ] && kill "$app" 2>/dev/null' EXIT

# ---- Phase: startup ---------------------------------------------------------------------
# Memory of a process as "total X MiB (anon Y, file Z, shmem W)", read from its status.
rss_parts() {
  awk '/^VmRSS:/ { t = $2 } /^RssAnon:/ { a = $2 } /^RssFile:/ { f = $2 } /^RssShmem:/ { s = $2 } END { printf "%d MiB (anon %d, file %d, shmem %d)", t / 1024, a / 1024, f / 1024, s / 1024 }' "/proc/$1/status"
}

first_frames() { # label, then the variables the app gets
  local label="$1"; shift
  local times=() i ms idle=""
  for i in 1 2 3 4 5; do
    local cfg; cfg=$(mktemp -d)
    env XDG_CONFIG_HOME="$cfg" "$@" RUST_LOG=warn "$bin" 2> "$out/startup-$label-$i.log" &
    local pid=$!
    local waited=0
    until grep -q 'soak: first frame' "$out/startup-$label-$i.log" 2>/dev/null || [ "$waited" -ge 200 ]; do
      sleep 0.1; waited=$((waited + 1))
    done
    ms=$(sed -n 's/.*first frame \([0-9]*\) ms after the process started.*/\1/p' "$out/startup-$label-$i.log" | head -1)
    # Left to settle, then its memory (the last run's is the one reported).
    sleep 4
    kill -0 "$pid" 2>/dev/null && idle=$(rss_parts "$pid")
    kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
    rm -rf "$cfg"
    [ -n "$ms" ] && times+=("$ms")
  done
  if [ "${#times[@]}" -eq 0 ]; then
    note "startup $label: no first-frame line (the app did not draw)"
    failed="$failed startup-$label"
    return
  fi
  local sorted median worst
  sorted=$(printf '%s\n' "${times[@]}" | sort -n)
  median=$(printf '%s\n' "$sorted" | awk '{ a[NR] = $1 } END { print a[int((NR + 1) / 2)] }')
  worst=$(printf '%s\n' "$sorted" | tail -1)
  note "startup $label: first frame after $(printf '%s ' "${times[@]}")ms; median $median ms, worst $worst ms"
  [ -n "$idle" ] && note "  idle 5 s later: $idle"
  if [ "$median" -ge 1000 ]; then failed="$failed startup-$label"; note "  over the 1000 ms target"; fi
}
if [[ " $phases " == *" startup "* ]]; then
  say "phase startup"
  note "--- startup (to the first frame, from the start of the process; Xvfb with software rendering)"
  first_frames sign-in INLET_SOAK_STARTUP=1
  first_frames demo INLET_DEMO=1 INLET_SOAK_STARTUP=1
  first_frames soak INLET_DEMO=1 INLET_SOAK=1 INLET_SOAK_HOURS=0
fi

# ---- Phase: app -------------------------------------------------------------------------
if [[ " $phases " == *" app "* ]]; then
  say "phase app"
  cfg=$(mktemp -d)
  csv="$out/samples.csv"
  echo "wall_s,virtual_h,rss_kb,hwm_kb,threads,cpu_s,anon_kb,file_kb" > "$csv"
  env XDG_CONFIG_HOME="$cfg" RUST_LOG=warn INLET_DEMO=1 INLET_DEMO_LIVE_SECS=1000000000 \
    INLET_SOAK=1 INLET_SOAK_HOURS="$hours" INLET_SOAK_SPEED="$speed" INLET_SOAK_SEED="$seed" INLET_SOAK_RATE="$rate" \
    INLET_SOAK_NAV_MS="$nav_ms" INLET_SOAK_LOG_SECS="$(( sample * 5 ))" \
    "$bin" 2> "$out/app.log" &
  app=$!
  sleep 5
  # Without a window manager nothing has the focus.
  win=$(xdotool search --name '^Inlet' | head -1)
  [ -n "$win" ] && xdotool windowfocus "$win" 2>/dev/null
  start=$(date +%s)
  clk=$(getconf CLK_TCK)
  ended=""
  while :; do
    now=$(date +%s)
    el=$(( now - start ))
    if ! kill -0 "$app" 2>/dev/null; then ended="exited"; break; fi
    rss=$(awk '/^VmRSS:/ { print $2 }' "/proc/$app/status")
    hwm=$(awk '/^VmHWM:/ { print $2 }' "/proc/$app/status")
    anon=$(awk '/^RssAnon:/ { print $2 }' "/proc/$app/status")
    file=$(awk '/^RssFile:/ { print $2 }' "/proc/$app/status")
    thr=$(awk '/^Threads:/ { print $2 }' "/proc/$app/status")
    cpu=$(awk -v c="$clk" '{ printf "%.1f", ($14 + $15) / c }' "/proc/$app/stat")
    vh=$(awk -v e="$el" -v s="$speed" 'BEGIN { printf "%.3f", e * s / 3600 }')
    echo "$el,$vh,$rss,$hwm,$thr,$cpu,$anon,$file" >> "$csv"
    [ "$el" -ge "$wall_secs" ] && break
    sleep "$sample"
  done
  if [ "$ended" = exited ]; then
    note "the app exited before the run was over (after $(( $(date +%s) - start )) s)"
    tail -n 20 "$out/app.log" >&2
    failed="$failed app-exited"
  fi
  kill "$app" 2>/dev/null; wait "$app" 2>/dev/null; app=""
  if grep -qE 'panicked|already (mutably )?borrowed' "$out/app.log"; then
    note "a panic or a borrow error is in the log"
    grep -E -A3 'panicked|already (mutably )?borrowed' "$out/app.log" | head -20 | tee -a "$summary" >&2
    failed="$failed panic"
  fi
  rm -rf "$cfg"

  note "--- app: $(( $(wc -l < "$csv") - 1 )) samples, recording $hours h in $wall_secs s"
  # Peak, the level at the sixth virtual hour, the highest after it, and the slope after
  # it (least squares, KiB per virtual hour).
  verdict=$(awk -F, -v hours="$hours" -v steady="$steady" '
    NR == 1 { next }
    { n++; h[n] = $2; r[n] = $3; if ($4 > peak) peak = $4; if ($7 > apeak) apeak = $7; if ($3 > top) top = $3 }
    END {
      if (n < 3) { print "NOSAMPLES"; exit }
      base = 0; m = 0
      for (i = 1; i <= n; i++) if (h[i] >= steady) {
        if (!m) { base = r[i]; baseh = h[i] }
        m++; x[m] = h[i]; y[m] = r[i]; if (r[i] > after) after = r[i]
      }
      slope = 0
      if (m >= 4) {
        for (i = 1; i <= m; i++) { sx += x[i]; sy += y[i] }
        mx = sx / m; my = sy / m
        for (i = 1; i <= m; i++) { num += (x[i] - mx) * (y[i] - my); den += (x[i] - mx) ^ 2 }
        if (den > 0) slope = num / den
      }
      printf "anon_peak_kb=%d peak_kb=%d final_kb=%d base_kb=%d base_h=%.1f after_max_kb=%d slope_kb_h=%.1f post6=%d\n", apeak, peak, r[n], base, baseh, after, slope, m
    }' "$csv")
  note "$verdict"
  if [ "$verdict" = NOSAMPLES ]; then
    failed="$failed app-nosamples"
  else
    eval "$verdict"
    note "peak resident $(( peak_kb / 1024 )) MiB (target: under 150 MiB; of it at most $(( anon_peak_kb / 1024 )) MiB anonymous, the rest is shared libraries and the like), at the end $(( final_kb / 1024 )) MiB"
    if [ "$peak_kb" -ge 153600 ]; then failed="$failed app-over-150MiB"; note "  over the target"; fi
    if [ "$post6" -ge 4 ]; then
      note "after virtual hour $steady: $(( base_kb / 1024 )) MiB at hour $base_h, highest $(( after_max_kb / 1024 )) MiB, slope $slope_kb_h KiB per hour"
      if [ "$(( after_max_kb - base_kb ))" -gt 8192 ] || awk -v s="$slope_kb_h" 'BEGIN { exit !(s > 256) }'; then
        failed="$failed app-growth"; note "  memory grows after hour $steady"
      fi
    else
      note "the run is shorter than 6 virtual hours (or has too few samples): the growth check is skipped"
    fi
  fi
  grep -E '^soak: (up_s|first frame|the recording)' "$out/app.log" | tail -n 5 | tee -a "$summary" >&2
fi

if [ -n "$failed" ]; then note "FAILED:$failed"; exit 1; fi
note "every target held"
