# SPDX-License-Identifier: CC0-1.0
"""Writes the sound effects Inlet ships, one 48 kHz mono 16-bit WAV file per sound.

Every sound is synthesised here from plain sine tones (with a quiet octave partial for a
bell-like tone, and short sweeps for the camera and stream sounds); they contain no
recording and no sample from any other source. Run with Python 3 from this folder:

    python make_sounds.py

The file names are the sound IDs of the app (`SoundKind::id`), so a new sound only needs
a line in SOUNDS and in the app's table.
"""
import math
import struct
import wave

RATE = 48000

# Note frequencies in Hz.
C4, E4, F4, G4, A4 = 261.63, 329.63, 349.23, 392.00, 440.00
C5, E5, G5, A5, B5 = 523.25, 659.25, 783.99, 880.00, 987.77
C6, E6 = 1046.50, 1318.51


def bell(u: float, f: float, amp: float, decay: float, partial: float = 0.18) -> float:
    """One struck tone `u` seconds after its start: 4 ms attack, exponential decay."""
    if u < 0:
        return 0.0
    env = min(1.0, u / 0.004) * math.exp(-u * decay)
    return amp * env * (
        math.sin(2 * math.pi * f * u) + partial * math.sin(4 * math.pi * f * u)
    )


def sweep(u: float, f0: float, f1: float, length: float, amp: float, decay: float) -> float:
    """A tone whose pitch glides from f0 to f1 over `length` seconds."""
    if u < 0 or u > length:
        return 0.0
    # Phase of a linear chirp: 2 pi (f0 u + (f1 - f0) u^2 / (2 length)).
    phase = 2 * math.pi * (f0 * u + (f1 - f0) * u * u / (2 * length))
    env = min(1.0, u / 0.006) * math.exp(-u * decay) * min(1.0, (length - u) / 0.02)
    return amp * env * math.sin(phase)


def render(duration: float, voice) -> bytes:
    n = int(RATE * duration)
    out = bytearray()
    for i in range(n):
        v = voice(i / RATE)
        # The last 20 ms fade to silence, so no sound ends in a click.
        tail = (n - i) / RATE
        if tail < 0.02:
            v *= tail / 0.02
        v = max(-1.0, min(1.0, v))
        out += struct.pack("<h", int(round(v * 32767 * 0.9)))
    return bytes(out)


def notes(*steps, decay: float = 9.0, amp: float = 0.28, partial: float = 0.18):
    """A voice from (start in seconds, frequency[, amplitude]) steps."""

    def voice(t: float) -> float:
        total = 0.0
        for step in steps:
            start, f = step[0], step[1]
            a = step[2] if len(step) > 2 else amp
            total += bell(t - start, f, a, decay, partial)
        return total

    return voice


def ring(t: float) -> float:
    """The incoming call: four short pairs of tones, then a pause (2 s, loops)."""
    total = 0.0
    for k in range(4):
        base = k * 0.38
        total += bell(t - base, A5, 0.26, 11.0, 0.12)
        total += bell(t - base - 0.11, E6, 0.22, 11.0, 0.12)
    return total


def merge(*voices):
    return lambda t: sum(v(t) for v in voices)


def at(start: float, voice):
    return lambda t: voice(t - start)


# (file name, duration in seconds, voice)
SOUNDS = [
    # A new message, in a community or a DM: A5, then E6.
    ("message", 0.42, notes((0.000, A5, 0.30), (0.075, E6, 0.26))),
    # A message in the channel that is open: one soft, short tick.
    ("same-channel-message", 0.20, notes((0.0, C6, 0.20), decay=18.0, partial=0.10)),
    ("incoming-ring", 2.0, ring),
    # Microphone: down when muted, up when unmuted.
    ("mute", 0.22, notes((0.0, E5, 0.26), (0.07, A4, 0.26), decay=14.0)),
    ("unmute", 0.22, notes((0.0, A4, 0.26), (0.07, E5, 0.26), decay=14.0)),
    # Deafen: lower and slower than mute.
    ("deaf", 0.30, notes((0.0, A4, 0.28), (0.09, E4, 0.28), (0.18, C4, 0.24), decay=12.0)),
    ("undeaf", 0.30, notes((0.0, C4, 0.24), (0.09, E4, 0.28), (0.18, A4, 0.28), decay=12.0)),
    # Someone joins or leaves the voice channel you are in.
    ("user-join", 0.40, notes((0.0, C5), (0.08, E5), (0.16, G5), decay=10.0)),
    ("user-leave", 0.40, notes((0.0, G5), (0.08, E5), (0.16, C5), decay=10.0)),
    # Someone is moved to another voice channel.
    ("user-move", 0.34, notes((0.0, G4, 0.26), (0.10, C5, 0.26), (0.20, G5, 0.20), decay=11.0)),
    # A viewer joins or leaves your stream: quieter, higher.
    ("viewer-join", 0.30, notes((0.0, E5, 0.20), (0.09, B5, 0.20), decay=12.0)),
    ("viewer-leave", 0.30, notes((0.0, B5, 0.20), (0.09, E5, 0.20), decay=12.0)),
    # The voice connection dropped: three falling notes with a long tail.
    ("voice-disconnect", 0.60, notes((0.0, E5, 0.28), (0.12, C5, 0.28), (0.24, A4, 0.28), decay=7.0)),
    # Camera: a quick glide up when it turns on, down when it turns off.
    ("camera-on", 0.26, lambda t: sweep(t, 500.0, 1100.0, 0.20, 0.30, 5.0)),
    ("camera-off", 0.26, lambda t: sweep(t, 1100.0, 500.0, 0.20, 0.30, 5.0)),
    # Screen share: a sweep and a bright pair on start, the reverse on stop.
    (
        "screen-share-start",
        0.46,
        merge(
            lambda t: sweep(t, 400.0, 900.0, 0.18, 0.22, 6.0),
            notes((0.16, G5, 0.20), (0.22, C6, 0.20), decay=9.0),
        ),
    ),
    (
        "screen-share-stop",
        0.46,
        merge(
            notes((0.0, C6, 0.20), (0.06, G5, 0.20), decay=9.0),
            at(0.18, lambda t: sweep(t, 900.0, 400.0, 0.18, 0.22, 6.0)),
        ),
    ),
]


def main() -> None:
    for name, duration, voice in SOUNDS:
        with wave.open(f"{name}.wav", "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(RATE)
            w.writeframes(render(duration, voice))


if __name__ == "__main__":
    main()
