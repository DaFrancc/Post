#!/usr/bin/env python3
"""Synthesize Post's default sound pack (app/assets/sounds/*.wav).

Every sound is made here from sine tones, so Post owns its defaults
outright and anyone can rebuild them:

    python3 app/tool/make_sounds.py

The names are the sound events the app plays (lib/state/sounds.dart).
"""

import math
import struct
import wave
from pathlib import Path

RATE = 22050
OUT = Path(__file__).resolve().parent.parent / "assets" / "sounds"


def note(freq, length, start=0.0, attack=0.008, decay=6.0, volume=0.5, harmonics=((2, 0.25), (3, 0.08)), glide=1.0):
    """A soft bell-like note: sine plus a little of its harmonics, a quick
    attack, an exponential fall. `glide` bends the pitch by that ratio over
    the note."""
    out = []
    for i in range(int(length * RATE)):
        t = i / RATE
        f = freq * (glide ** (t / length))
        env = min(1.0, t / attack) * math.exp(-decay * t)
        s = math.sin(2 * math.pi * f * t)
        for h, a in harmonics:
            s += a * math.sin(2 * math.pi * f * h * t)
        out.append(volume * env * s / (1 + sum(a for _, a in harmonics)))
    return start, out


def mix(length, *notes):
    buf = [0.0] * int(length * RATE)
    for start, samples in notes:
        off = int(start * RATE)
        for i, s in enumerate(samples):
            if off + i < len(buf):
                buf[off + i] += s
    return buf


def save(name, samples):
    OUT.mkdir(parents=True, exist_ok=True)
    peak = max(1e-9, max(abs(s) for s in samples))
    gain = min(1.0, 0.9 / peak)
    with wave.open(str(OUT / f"{name}.wav"), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(b"".join(struct.pack("<h", int(32767 * s * gain)) for s in samples))


C5, E5, G5, A5, C6, E6 = 523.25, 659.25, 783.99, 880.0, 1046.5, 1318.5

# Someone calling you: a rising three-note chime, then a pause. Loops.
save("ring_incoming", mix(2.4, note(E5, 0.9, 0.0, decay=4), note(G5, 0.9, 0.18, decay=4), note(C6, 1.2, 0.36, decay=3.5)))
# Calling someone: one soft, low pair, then a longer pause. Loops.
save("ring_outgoing", mix(3.0, note(A5 / 2, 1.0, 0.0, decay=3, volume=0.4), note(C5, 1.0, 0.0, decay=3, volume=0.3)))
# Someone joins: up a fifth.
save("join", mix(0.45, note(C5, 0.3, 0.0, decay=9), note(G5, 0.35, 0.1, decay=8)))
# Someone leaves: down a fifth.
save("leave", mix(0.45, note(G5, 0.3, 0.0, decay=9), note(C5, 0.35, 0.1, decay=8)))
# You leave the call: down through the chord.
save("disconnect", mix(0.6, note(G5, 0.25, 0.0, decay=10), note(E5, 0.25, 0.09, decay=10), note(C5, 0.4, 0.18, decay=7)))
# Mute and unmute: short blips, low then high.
save("mute", mix(0.12, note(330, 0.12, attack=0.003, decay=30, harmonics=())))
save("unmute", mix(0.12, note(660, 0.12, attack=0.003, decay=30, harmonics=())))
# A message: a soft drop of a note.
save("message", mix(0.3, note(A5, 0.3, attack=0.004, decay=14, glide=0.85)))
# A mention: two bright notes.
save("mention", mix(0.4, note(A5, 0.2, 0.0, decay=12), note(E6, 0.3, 0.09, decay=10)))

print(f"wrote {len(list(OUT.glob('*.wav')))} sounds to {OUT}")
