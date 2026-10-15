#!/usr/bin/env python3
"""Single source of truth for Hoppy's music: one original 16-bit-style chiptune per theme.

Everything is synthesised here from hand-written note data (no samples): band-limited
pulse waves (12.5/25/50 % duty) with ADSR, delayed vibrato and a detuned "chorus" voice,
2-operator FM leads/bells/e-piano (Mega Drive flavour), triangle bass, and drums made from
pitch-dropping sines and filtered noise, plus a tempo-synced echo. Per theme it renders:

  idle      calm loop, an exact number of bars, seamless (tails and echo wrap around)
  transfer  same hook, faster tempo, driving drums and arpeggiated bass, seamless
  done      short success jingle (not looped)

Outputs (generated, committed, never edit by hand):

  apps/android/app/src/main/res/raw/music_<theme>_<idle|transfer|done>.ogg   Vorbis 22050 Hz mono
  assets/music/desktop_<idle|transfer|done>.wav   Purikura only, PCM u8 mono 16 kHz (PlaySoundW)
  assets/music/manifest.json                      lengths, tempo, bars (for integrators)
  assets/music/README.md                          what/how/licence
  site/music/portal.{ogg,mp3,json}                download-portal loop (quieter, 32 kHz) + player data

Usage:  python3 tools/gen_music.py [--theme NAME|portal] [--out DIR] [--check]
        (or: python3 tools/dev.py music)
  --out DIR   write under DIR (same relative layout) instead of the repo
  --check     re-render and verify the committed outputs are up to date (WAV: bytes;
              OGG: decoded length + correlation, since Vorbis streams are not byte-stable)
Needs: pip install numpy soundfile.   Deterministic: fixed seeds, no wall-clock input.

Notation (tracker-like, one string per part, bars separated by `|`; a bar holds 8 or
16 equal steps):
  lead/notes  `C5`, `F#4`, `Bb5` = note on;  `-` = hold;  `.` = rest
  bass        R 3 5 6 7 8 9 = root / third / fifth / sixth / seventh / octave / ninth of
              the current chord (slash chords use the slash note as R)
  arp         1..9 = n-th chord tone upwards from `low`; `c` = whole chord
  drums       16 chars per bar per lane: X accent, x normal, g ghost, . nothing
Compositions are original (invariant: original art only). Licence: MIT, like the repo.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import math
import re
import sys
import wave
from dataclasses import dataclass, field, replace
from pathlib import Path

try:
    import numpy as np
    import soundfile as sf
except ImportError:  # pragma: no cover
    sys.exit("✗ pip install numpy soundfile")

ROOT = Path(__file__).resolve().parent.parent
RAW = "apps/android/app/src/main/res/raw"
DESK = "assets/music"
ANDROID_SR = 22050
DESKTOP_SR = 16000
OVERSAMPLE = 2                         # synthesise at 2x, band-limit + decimate in one FFT
PEAK = 10 ** (-3 / 20)                 # -3 dBFS ceiling
# Loudness: app tracks are matched at -19 dB RMS (Part.rms_db); the portal loop is quieter.
MP3_QUALITY = 0.55                     # libsndfile compression_level for LAME VBR (0 best .. 1 smallest)
WEB_SR = 32000
VORBIS_QUALITY = 0.30                  # libsndfile compression_level = 1 - quality
DESKTOP_TRANSFER_BARS = 8              # desktop transfer loop is shortened to fit ~250 KB
THEMES = ("purikura", "showa", "manga", "yume", "night")

# ------------------------------------------------------------------------- pitch & chords
NOTE_RE = re.compile(r"^([A-G])([#b]?)(-?\d)$")
PC = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}


def midi(name: str) -> int:
    m = NOTE_RE.match(name)
    if not m:
        raise ValueError(f"bad note {name!r}")
    acc = {"#": 1, "b": -1, "": 0}[m.group(2)]
    return 12 * (int(m.group(3)) + 1) + PC[m.group(1)] + acc


def hz(m: float) -> float:
    return 440.0 * 2 ** ((m - 69) / 12)


QUALITIES = {  # longest suffix first when matching
    "maj9": [0, 4, 7, 11, 14], "maj7": [0, 4, 7, 11], "m9": [0, 3, 7, 10, 14],
    "m7": [0, 3, 7, 10], "m6": [0, 3, 7, 9], "7sus4": [0, 5, 7, 10], "sus4": [0, 5, 7],
    "add9": [0, 4, 7, 14], "13": [0, 4, 7, 10, 14, 21], "9": [0, 4, 7, 10, 14],
    "7": [0, 4, 7, 10], "6": [0, 4, 7, 9], "dim": [0, 3, 6], "m": [0, 3, 7], "": [0, 4, 7],
}
CHORD_RE = re.compile(r"^([A-G][#b]?)([^/]*)(?:/([A-G][#b]?))?$")


@dataclass(frozen=True)
class Chord:
    root: int          # pitch class
    bass: int          # pitch class used for R (slash chords)
    ivs: tuple         # intervals from root

    def interval(self, deg: str) -> int:
        ivs = set(i % 12 for i in self.ivs)
        if deg == "R":
            return (self.bass - self.root) % 12
        if deg == "3":
            return 3 if 3 in ivs else 4 if 4 in ivs else 5
        if deg == "5":
            return 6 if 6 in ivs and 7 not in ivs else 7
        if deg == "7":
            return 10 if 10 in ivs else 11 if 11 in ivs else 12
        return {"6": 9, "8": 12, "9": 14}[deg]


def chord(sym: str) -> Chord:
    m = CHORD_RE.match(sym)
    if not m or m.group(2) not in QUALITIES:
        raise ValueError(f"bad chord {sym!r}")
    root = midi(m.group(1) + "0") % 12
    bass = midi(m.group(3) + "0") % 12 if m.group(3) else root
    return Chord(root, bass, tuple(QUALITIES[m.group(2)]))


def bars_of(text: str) -> list[list[str]]:
    return [b.split() for b in text.strip().strip("|").split("|")]


def join(*parts: str) -> str:
    return " | ".join(p.strip().strip("|").strip() for p in parts)


def rests(n: int, steps: int = 8) -> str:
    return join(*[" ".join("." * steps)] * n)


def harmonize(text: str, scale: list[int], degrees: int) -> str:
    """Shift every note of a line by `degrees` steps of `scale` (pitch classes)."""
    pool = [n for n in range(128) if n % 12 in scale]
    out = []
    for bar in bars_of(text):
        toks = []
        for t in bar:
            if t in "-.":
                toks.append(t)
                continue
            m = midi(t)
            i = max(k for k, n in enumerate(pool) if n <= m)
            toks.append(f"#{pool[i + degrees] + (m - pool[i])}")  # '#<midi>' = raw note number
        out.append(" ".join(toks))
    return join(*out)


def note_value(tok: str) -> int:
    return int(tok[1:]) if tok.startswith("#") else midi(tok)


# ------------------------------------------------------------------------- instruments
@dataclass(frozen=True)
class Inst:
    wave: str = "pulse"          # pulse | tri | sine | fm
    duty: float = 0.5
    a: float = 0.005             # ADSR (seconds, sustain level)
    d: float = 0.12
    s: float = 0.6
    r: float = 0.05
    gate: float = 0.92           # fraction of the written length that sounds
    vib: tuple = (0.0, 5.5, 0.2)  # cents, Hz, delay s
    detune: float = 0.0          # cents of a second (chorus) voice; 0 = none
    ratio: float = 1.0           # FM modulator ratio
    index: float = 0.0           # FM index at note-on
    index_end: float = 0.0       # FM index it decays to
    index_t: float = 0.1         # FM index decay time constant
    slide: tuple = (0.0, 0.0)    # pitch slide from (semitones, seconds) into the note
    gain: float = 0.3


PULSE12 = Inst(duty=0.125, d=0.08, s=0.35, r=0.04, gain=0.22)
PULSE25 = Inst(duty=0.25, vib=(14, 5.6, 0.18), detune=7, gain=0.28)
PULSE50 = Inst(duty=0.5, vib=(12, 5.5, 0.2), detune=6, gain=0.24)
TRI_BASS = Inst(wave="tri", a=0.003, d=0.2, s=0.8, r=0.03, gain=0.62)
FM_BELL = Inst(wave="fm", ratio=3.5, index=2.2, index_end=0.2, index_t=0.12, a=0.003, d=0.5,
               s=0.0, r=0.3, gate=1.0, gain=0.22)
FM_EP = Inst(wave="fm", ratio=1.0, index=1.4, index_end=0.25, index_t=0.25, a=0.004, d=0.7,
             s=0.3, r=0.12, gate=0.95, gain=0.16, detune=5)
FM_FLUTE = Inst(wave="fm", ratio=1.0, index=0.7, index_end=0.45, index_t=0.3, a=0.03, d=0.3,
                s=0.75, r=0.18, vib=(16, 5.2, 0.25), detune=9, gain=0.30)
FM_LEAD = Inst(wave="fm", ratio=1.0, index=1.8, index_end=0.9, index_t=0.15, a=0.008, d=0.25,
               s=0.65, r=0.1, vib=(15, 5.4, 0.2), detune=6, gain=0.27)
FM_BASS = Inst(wave="fm", ratio=1.0, index=2.4, index_end=0.6, index_t=0.06, a=0.003, d=0.25,
               s=0.55, r=0.04, gain=0.55)
FM_KOTO = Inst(wave="fm", ratio=3.0, index=1.6, index_end=0.1, index_t=0.05, a=0.002, d=0.25,
               s=0.0, r=0.15, gate=1.0, gain=0.20)


def _polyblep(t: np.ndarray, dt: np.ndarray) -> np.ndarray:
    y = np.zeros_like(t)
    lo = t < dt
    x = t[lo] / dt[lo]
    y[lo] = x + x - x * x - 1
    hi = t > 1 - dt
    x = (t[hi] - 1) / dt[hi]
    y[hi] = x * x + x + x + 1
    return y


def _osc(inst: Inst, f: np.ndarray, sr: int, phase0: float, t: np.ndarray) -> np.ndarray:
    dt = np.minimum(f / sr, 0.49)
    ph = (phase0 + np.cumsum(dt) - dt[0]) % 1.0
    if inst.wave == "pulse":
        ph2 = (ph + 1 - inst.duty) % 1.0
        saw1 = 2 * ph - 1 - _polyblep(ph, dt)
        saw2 = 2 * ph2 - 1 - _polyblep(ph2, dt)
        return 0.5 * (saw1 - saw2)
    if inst.wave == "tri":
        return 2 * np.abs(2 * ph - 1) - 1
    if inst.wave == "sine":
        return np.sin(2 * np.pi * ph)
    # 2-op FM: carrier phase modulated by a sine at ratio * f with a decaying index
    idx = inst.index_end + (inst.index - inst.index_end) * np.exp(-t / inst.index_t)
    phm = np.cumsum(dt * inst.ratio)
    return np.sin(2 * np.pi * ph + idx * np.sin(2 * np.pi * phm))


def synth_note(inst: Inst, note: float, dur: float, vel: float, sr: int) -> np.ndarray:
    on = max(dur * inst.gate, 0.02)
    n = int((on + inst.r * 5) * sr)
    t = np.arange(n) / sr
    cents = np.zeros(n)
    depth, rate, delay = inst.vib
    if depth:
        ramp = np.clip((t - delay) / 0.15, 0, 1)
        cents += depth * ramp * np.sin(2 * np.pi * rate * t)
    if inst.slide[0]:
        cents += inst.slide[0] * 100 * np.exp(-t / max(inst.slide[1], 1e-3))
    f = hz(note) * 2 ** (cents / 1200)
    y = _osc(inst, f, sr, 0.0, t)
    if inst.detune:
        y = 0.6 * y + 0.4 * _osc(inst, f * 2 ** (inst.detune / 1200), sr, 0.37, t)
    env = np.where(t < inst.a, t / inst.a, inst.s + (1 - inst.s) * np.exp(-(t - inst.a) / inst.d))
    k = int(on * sr)
    if k < n:
        env[k:] = env[k] * np.exp(-(t[k:] - t[k]) / inst.r)
    return y * env * vel * inst.gain


# ------------------------------------------------------------------------- drums
def _shape_noise(n: int, sr: int, lo: float, hi: float, seed: int) -> np.ndarray:
    x = np.random.default_rng(seed).standard_normal(n)
    spec = np.fft.rfft(x)
    fr = np.fft.rfftfreq(n, 1 / sr)
    spec *= 1 / np.sqrt(1 + (lo / np.maximum(fr, 1)) ** 4) / np.sqrt(1 + (fr / hi) ** 4)
    y = np.fft.irfft(spec, n)
    return y / (np.abs(y).max() + 1e-9)


def drum(kind: str, sr: int) -> np.ndarray:
    seed = sum(map(ord, kind)) * 7919
    if kind == "kick":
        t = np.arange(int(0.32 * sr)) / sr
        f = 48 + 120 * np.exp(-t / 0.028)
        y = np.sin(2 * np.pi * np.cumsum(f) / sr) * np.exp(-t / 0.13)
        return 0.9 * y + 0.15 * _shape_noise(len(t), sr, 1500, 6000, seed) * np.exp(-t / 0.004)
    if kind == "snare":
        t = np.arange(int(0.22 * sr)) / sr
        tone = np.sin(2 * np.pi * 185 * t) * np.exp(-t / 0.04)
        return 0.45 * tone + 0.6 * _shape_noise(len(t), sr, 1200, 7000, seed) * np.exp(-t / 0.07)
    if kind in ("hat", "ohat"):
        t = np.arange(int((0.05 if kind == "hat" else 0.3) * sr)) / sr
        dec = 0.018 if kind == "hat" else 0.09
        return 0.35 * _shape_noise(len(t), sr, 7000, 14000, seed) * np.exp(-t / dec)
    if kind == "shaker":
        t = np.arange(int(0.09 * sr)) / sr
        env = np.minimum(t / 0.012, 1) * np.exp(-t / 0.03)
        return 0.3 * _shape_noise(len(t), sr, 4500, 10000, seed) * env
    if kind == "clap":
        t = np.arange(int(0.2 * sr)) / sr
        env = sum(np.exp(-np.maximum(t - o, 0) / 0.006) * (t >= o) for o in (0, 0.011, 0.022))
        env = env + 0.6 * np.exp(-np.maximum(t - 0.03, 0) / 0.05) * (t >= 0.03)
        return 0.4 * _shape_noise(len(t), sr, 900, 5000, seed) * env
    if kind == "wood":
        t = np.arange(int(0.08 * sr)) / sr
        return 0.5 * (np.sin(2 * np.pi * 1250 * t) + 0.4 * np.sin(2 * np.pi * 1890 * t)) * np.exp(-t / 0.018)
    if kind == "tom":
        t = np.arange(int(0.3 * sr)) / sr
        f = 115 + 90 * np.exp(-t / 0.05)
        return 0.75 * np.sin(2 * np.pi * np.cumsum(f) / sr) * np.exp(-t / 0.11)
    if kind == "brush":   # soft brushed snare: slow-ish attack, airy band of noise
        t = np.arange(int(0.25 * sr)) / sr
        env = np.minimum(t / 0.01, 1) * np.exp(-t / 0.07)
        return 0.4 * _shape_noise(len(t), sr, 1800, 7000, seed) * env
    if kind == "crash":
        t = np.arange(int(1.4 * sr)) / sr
        return 0.3 * _shape_noise(len(t), sr, 3500, 13000, seed) * np.exp(-t / 0.45)
    raise ValueError(kind)


VEL = {"X": 1.0, "x": 0.75, "g": 0.4}

# ------------------------------------------------------------------------- arrangement


@dataclass(frozen=True)
class Track:
    kind: str                  # notes | bass | arp
    inst: Inst
    line: str
    gain: float = 1.0
    send: float = 0.0          # echo send
    low: int = 48              # lowest MIDI note for bass roots / arp voicing
    transpose: int = 0


@dataclass(frozen=True)
class Part:
    bpm: float
    chords: str
    tracks: tuple
    drums: dict                       # lane -> list of 16-step bar patterns (cycled)
    drum_gain: dict = field(default_factory=dict)
    fills: dict = field(default_factory=dict)   # lane -> (every N bars, pattern) on the Nth bar
    swing: float = 0.5                # 0.5 straight, 0.667 triplet shuffle
    swing_grid: float = 1.0           # beats per swing pair: 1.0 = 8ths, 0.5 = 16ths
    echo: tuple = (0.75, 0.32, 0.3)   # delay beats, feedback, wet
    bars: int = 8
    beats_per_bar: int = 4
    seconds: float = 0.0              # >0: one-shot jingle of this length
    lowpass: float = 0.0              # master low-pass Hz (0 = default for the sample rate)
    rms_db: float = -19.0             # loudness target (simple RMS, dBFS)


@dataclass(frozen=True)
class Theme:
    name: str
    key: str
    mood: str
    scale: tuple
    idle: Part
    transfer: Part
    done: Part


def _swing(b: float, part: Part) -> float:
    if part.swing == 0.5:
        return b
    g = part.swing_grid
    i, f = divmod(b / g, 1.0)
    s = part.swing
    f = f / 0.5 * s if f < 0.5 else s + (f - 0.5) / 0.5 * (1 - s)
    return (i + f) * g


def _events(line: str, bpb: int):
    """Yield (start_beat, length_beats, token) from a notation line."""
    cur = None
    for bi, bar in enumerate(bars_of(line)):
        step = bpb / len(bar)
        for si, tok in enumerate(bar):
            b = bi * bpb + si * step
            if tok == "-":
                if cur:
                    cur[1] += step
                continue
            if cur:
                yield tuple(cur)
                cur = None
            if tok != ".":
                cur = [b, step, tok]
    if cur:
        yield tuple(cur)


def _chord_at(chords: list[list[Chord]], beat: float, bpb: int) -> Chord:
    bar = chords[int(beat // bpb) % len(chords)]
    return bar[min(int((beat % bpb) / (bpb / len(bar))), len(bar) - 1)]


def _above(pc: int, low: int) -> int:
    return low + (pc - low) % 12


def _voicing(c: Chord, low: int, count: int) -> list[int]:
    pcs = sorted({(c.root + i) % 12 for i in c.ivs})
    tones = sorted(n for n in range(low, low + 48) if n % 12 in pcs)
    return tones[:count]


def _resolve(tr: Track, tok: str, ch: Chord) -> list[int]:
    if tr.kind == "notes":
        return [note_value(tok) + tr.transpose]
    if tr.kind == "bass":
        if tok == "R":
            return [_above(ch.bass, tr.low)]
        return [_above(ch.root, tr.low) + ch.interval(tok)]
    npcs = len({i % 12 for i in ch.ivs})
    v = _voicing(ch, tr.low, 12)
    return v[:npcs] if tok == "c" else [v[int(tok) - 1]]


def render(theme, which: str, sr: int, bars: int | None = None) -> np.ndarray:
    """Render one piece at `sr`. Loops are circular: exact length, tails wrapped."""
    part: Part = getattr(theme, which)
    if bars:
        part = replace(part, bars=bars)
    loop = part.seconds == 0
    bpb = part.beats_per_bar
    total_beats = part.bars * bpb
    isr = sr * OVERSAMPLE
    if loop:
        n_out = round(total_beats * 60 / part.bpm * sr)
    else:
        n_out = round(part.seconds * sr)
    n_int = n_out * OVERSAMPLE
    spb = (n_int / total_beats) if loop else 60 / part.bpm * isr   # internal samples per beat
    pad = int(3.0 * isr)
    chords = [[chord(s) for s in bar] for bar in bars_of(part.chords)]

    def pos(beat: float) -> int:
        return int(round(_swing(beat, part) * spb))

    dry = np.zeros(n_int + pad)
    wet = np.zeros(n_int + pad)
    for tr in part.tracks:
        for b, ln, tok in _events(tr.line, bpb):
            if b >= total_beats:
                break
            ch = _chord_at(chords, b, bpb)
            start, end = pos(b), pos(b + ln)
            vel = 1.0 if abs(b - round(b)) < 1e-9 else 0.86
            for note in _resolve(tr, tok, ch):
                y = synth_note(tr.inst, note, (end - start) / isr, vel * tr.gain, isr)
                y = y[: len(dry) - start]
                dry[start:start + len(y)] += y
                if tr.send:
                    wet[start:start + len(y)] += y * tr.send
    hits = {k: drum(k, isr) for k in part.drums}
    for lane, pats in part.drums.items():
        g = part.drum_gain.get(lane, 0.5)
        for bar in range(part.bars):
            pat = pats[bar % len(pats)].replace(" ", "")
            every = part.fills.get(lane)
            if every and (bar + 1) % every[0] == 0:
                pat = every[1].replace(" ", "")
            for si, chx in enumerate(pat):
                if chx in VEL:
                    start = pos(bar * bpb + si * bpb / len(pat))
                    y = hits[lane][: len(dry) - start]
                    dry[start:start + len(y)] += y * g * VEL[chx]

    if loop:  # wrap everything that rings past the loop end back onto its start
        for buf in (dry, wet):
            buf[:pad] += buf[n_int:n_int + pad]
        n_fft = n_int
        dry, wet = dry[:n_int], wet[:n_int]
    else:
        n_fft = n_int + pad
    # Echo as a circular (loop) or zero-padded (jingle) filter: one FFT multiply.
    fr = np.fft.rfftfreq(n_fft, 1 / isr)
    delay = part.echo[0] * 60 / part.bpm * isr
    damp = 1 / np.sqrt(1 + (fr / 3500) ** 2)
    h = np.zeros(len(fr), dtype=complex)
    for k in range(1, 7):
        h += part.echo[1] ** (k - 1) * (damp ** k) * np.exp(-2j * np.pi * fr * k * delay / isr)
    spec = np.fft.rfft(dry) + np.fft.rfft(wet) * h * part.echo[2]
    # Master: DC/rumble high-pass, gentle low-pass, band-limit and decimate to `sr`.
    lp = part.lowpass or (8500 if sr >= 22050 else 6200)
    spec *= 1 / np.sqrt(1 + (fr / lp) ** 4) / np.sqrt(1 + (28 / np.maximum(fr, 1e-3)) ** 4)
    spec[0] = 0
    m = n_fft // OVERSAMPLE
    out = np.fft.irfft(spec[: m // 2 + 1], m) / OVERSAMPLE
    if not loop:
        out = out[:n_out]
        fade = int(0.25 * sr)
        out[-fade:] *= 0.5 + 0.5 * np.cos(np.linspace(0, np.pi, fade))
        out[: int(0.002 * sr)] *= np.linspace(0, 1, int(0.002 * sr))
    return out


def loudness(x: np.ndarray, loop: bool, sr: int) -> float:
    if loop:
        return float(np.sqrt(np.mean(x ** 2)))
    w = int(sr * 1.0)
    c = np.concatenate([[0], np.cumsum(x ** 2)])
    return float(np.sqrt(np.max(c[w:] - c[:-w]) / w))


def limit(x: np.ndarray, loop: bool, sr: int, thr: float) -> np.ndarray:
    """Transparent-ish peak limiter: block-max envelope, min-dilated then box-smoothed (so the
    smoothed gain is never above what a peak needs), circular for loops."""
    blk = max(sr // 1000, 8)
    n = -(-len(x) // blk) * blk
    env = np.pad(np.abs(x), (0, n - len(x))).reshape(-1, blk).max(axis=1)
    need = np.minimum(1.0, thr / np.maximum(env, 1e-9))
    r = 12                                         # ~12 ms either side
    mode = "wrap" if loop else "edge"
    padded = np.pad(need, r, mode=mode)
    dil = np.lib.stride_tricks.sliding_window_view(padded, 2 * r + 1).min(axis=1)
    padded = np.pad(dil, r, mode=mode)
    smooth = np.lib.stride_tricks.sliding_window_view(padded, 2 * r + 1).mean(axis=1)
    centers = np.arange(len(smooth)) * blk + blk / 2
    if loop:  # interpolate around the seam too
        centers = np.concatenate([[centers[-1] - n], centers, [centers[0] + n]])
        smooth = np.concatenate([[smooth[-1]], smooth, [smooth[0]]])
    g = np.interp(np.arange(len(x)), centers, smooth)
    return x * g


def normalize(x: np.ndarray, loop: bool, sr: int, rms_db: float = -19.0) -> np.ndarray:
    """Match loudness (RMS) across themes, limit peaks, guarantee the -3 dBFS ceiling."""
    x = limit(x * (10 ** (rms_db / 20) / loudness(x, loop, sr)), loop, sr, PEAK * 0.97)
    return x * min(1.0, PEAK / np.abs(x).max())


# ------------------------------------------------------------------------- the music
D_MAJ = [2, 4, 6, 7, 9, 11, 1]
G_MAJ = [7, 9, 11, 0, 2, 4, 6]
C_MAJ = [0, 2, 4, 5, 7, 9, 11]
F_LYD = [5, 7, 9, 11, 0, 2, 4]
D_MIN = [2, 4, 5, 7, 9, 10, 0]

# Kick / snare / hat building blocks (16 steps per bar)
FOUR = "x...x...x...x..."
BACK = "....x.......x..."
HATS16 = "gxXxgxXxgxXxgxXx"
CRASH8 = ["x" + "." * 15] + ["." * 16] * 7      # crash on the first bar of every 8


def purikura() -> Theme:
    chords = "D | F#m | G | A | Bm | G | Em7 A7 | D"
    hook = """
        A5  .  F#5 .  A5 B5 A5  .   |
        C#6 .  A5  .  F#5 - E5  .   |
        D5  .  G5  .  B5 .  D6  C#6 |
        B5  A5 -   -  E5 .  .   .   |
        A5  .  F#5 .  A5 B5 A5  .   |
        D6  .  B5  .  G5 -  B5  .   |
        G5  .  B5  .  E6 -  C#6 .   |
        D6  -  -   .  .  .  F#5 G5
    """
    idle = Part(
        bpm=124, chords=chords,
        tracks=(Track("notes", PULSE25, hook, send=0.6),
                Track("bass", TRI_BASS, "R . 8 . R . 8 .", low=38),
                Track("arp", replace(PULSE12, gate=0.4), ". c . c . c . c", gain=0.55, low=62)),
        drums={"kick": ["x.......x......."], "snare": [BACK], "hat": ["..x...x...x...x."]},
        drum_gain={"kick": 0.55, "snare": 0.22, "hat": 0.25},
    )
    transfer = Part(
        bpm=172, chords=join(chords, chords), bars=16,
        tracks=(Track("notes", PULSE25, join(hook, hook), send=0.5),
                Track("notes", replace(PULSE12, s=0.5, detune=8), join(rests(8), harmonize(hook, D_MAJ, -2)),
                      gain=0.9, send=0.4),
                Track("bass", TRI_BASS, "R 8 5 8 R 8 5 8", low=38),
                Track("arp", replace(PULSE12, gate=0.6), "1 2 3 4 3 2 1 2 3 4 3 2 1 2 3 4", gain=0.45,
                      send=0.5, low=66)),
        drums={"kick": [FOUR], "snare": [BACK], "hat": [HATS16], "crash": CRASH8},
        drum_gain={"kick": 0.7, "snare": 0.45, "hat": 0.28, "crash": 0.3},
        fills={"snare": (4, "....x.......xxXX")},
    )
    done = Part(
        bpm=160, chords="D", seconds=2.0, bars=1,
        tracks=(Track("notes", PULSE25, "D5 F#5 A5 D6 . F#6 . A6 - - - - - - . ."),
                Track("notes", PULSE12, ". . . . . . . D6 - - - - - - . .", gain=0.8),
                Track("arp", replace(PULSE12, s=0.6), ". . . . . . . . c - - - - - - .", gain=0.6, low=62),
                Track("bass", TRI_BASS, "R . . . . . . . 8 - - - - - . .", low=38)),
        drums={"kick": ["x.......x......."], "snare": ["....ggxX........"], "crash": ["........x......."]},
        drum_gain={"kick": 0.6, "snare": 0.35, "crash": 0.35},
    )
    return Theme("purikura", "D major", "bouncy, cutest", tuple(D_MAJ), idle, transfer, done)


def showa() -> Theme:
    chords = "G | Em | C | D7 | G | Em | Am7 D7 | G"
    hook = """
        G5 .  E5 G5 A5 .  G5 E5 |
        D5 -  E5 -  B4 -  .  .  |
        E5 .  G5 .  A5 B5 A5 G5 |
        A5 -  -  -  .  .  D5 E5 |
        G5 .  E5 G5 B5 .  A5 G5 |
        E5 -  G5 -  D5 -  B4 .  |
        A4 .  B4 D5 E5 .  D5 B4 |
        G4 -  -  .  .  .  D5 E5
    """
    lead = replace(PULSE50, vib=(26, 6.0, 0.15), detune=8)
    idle = Part(
        bpm=112, chords=chords, swing=0.64,
        tracks=(Track("notes", lead, hook, send=0.45),
                Track("bass", TRI_BASS, "R . 5 . 8 . 5 .", low=40),
                Track("arp", FM_KOTO, ". c . c . c . c", gain=0.7, low=59)),
        drums={"kick": ["x.......x......."], "snare": [BACK], "hat": ["x.x.x.x.x.x.x.x."],
               "wood": ["......x.......x."]},
        drum_gain={"kick": 0.55, "snare": 0.3, "hat": 0.22, "wood": 0.18},
    )
    # Transfer: the shuffle straightens into a 60s "eleki" surf drive.
    transfer = Part(
        bpm=156, chords=join(chords, chords), bars=16, swing=0.54,
        tracks=(Track("notes", lead, join(hook, hook), send=0.4),
                Track("notes", replace(PULSE25, detune=0), join(rests(8), harmonize(hook, G_MAJ, -2)),
                      gain=0.8, send=0.3),
                Track("bass", TRI_BASS, "R R 5 R 8 R 5 R", low=40),
                Track("arp", FM_KOTO, "1 3 2 4 3 5 4 3", gain=0.6, low=67, send=0.3)),
        drums={"kick": ["x.....x.x...x..."], "snare": [BACK], "hat": [HATS16], "wood": ["..x...x...x...x."],
               "tom": ["." * 16], "crash": CRASH8},
        drum_gain={"kick": 0.7, "snare": 0.45, "hat": 0.25, "wood": 0.12, "tom": 0.5, "crash": 0.25},
        fills={"tom": (8, "........x.x.xxxx"), "snare": (8, "....x...........")},
    )
    done = Part(
        bpm=140, chords="G", seconds=2.1, bars=1,
        tracks=(Track("notes", lead, "D5 E5 G5 . B5 . D6 . G6 - - - - - . ."),
                Track("arp", FM_KOTO, ". . . . . . . . c . c . c - - -", gain=0.8, low=59),
                Track("bass", TRI_BASS, "R . . . 5 . . . R - - - - - . .", low=40)),
        drums={"wood": ["x.x.x.x........."], "kick": ["x.......x......."], "crash": ["........x......."]},
        drum_gain={"wood": 0.25, "kick": 0.55, "crash": 0.3},
    )
    return Theme("showa", "G major (yonanuki: G A B D E)", "retro shuffle, kayōkyoku",
                 tuple(G_MAJ), idle, transfer, done)


def manga() -> Theme:
    chords = "C | A7 | Dm7 | G7 | C | C7 | F Fm | C G7"
    hook = """
        G5  .  G5 E5 .   C5 . .  |
        C#5 .  E5 .  G5  .  A5 . |
        F5  .  F5 D5 .   A4 . .  |
        B4  C5 D5 .  G5  .  . .  |
        G5  .  G5 E5 .   C6 . .  |
        Bb5 .  A5 .  G5  .  E5 . |
        A5  .  F5 .  Ab5 .  F5 . |
        E5  .  C5 .  D5  .  G4 .
    """
    lead = replace(PULSE50, s=0.3, d=0.08, gate=0.55, vib=(0, 5, 0), detune=5)
    sparkle = replace(PULSE12, gate=0.35)
    idle = Part(
        bpm=128, chords=chords,
        tracks=(Track("notes", lead, hook, send=0.25),
                Track("notes", sparkle, hook, transpose=12, gain=0.45),
                Track("bass", replace(TRI_BASS, gate=0.5), "R . 5 . R . 5 .", low=36),
                Track("arp", replace(PULSE25, gate=0.3, vib=(0, 5, 0), detune=0), ". c . c . c . c",
                      gain=0.35, low=60)),
        drums={"kick": ["x.......x......."], "snare": [BACK], "hat": ["..x...x...x...x."]},
        drum_gain={"kick": 0.6, "snare": 0.35, "hat": 0.25},
        fills={"snare": (4, "....x.......x.gx")},
    )
    transfer = Part(
        bpm=180, chords=join(chords, chords), bars=16,
        tracks=(Track("notes", lead, join(hook, hook), send=0.2),
                Track("notes", sparkle, join(hook, rests(8)), transpose=12, gain=0.4),
                Track("notes", replace(lead, gain=0.2), join(rests(8), harmonize(hook, C_MAJ, -5)), send=0.2),
                Track("bass", replace(TRI_BASS, gate=0.6), "R 8 R 8 R 8 R 8", low=36),
                Track("arp", replace(PULSE25, gate=0.3, vib=(0, 5, 0), detune=0), ". c . c . c . c",
                      gain=0.3, low=60)),
        drums={"kick": [FOUR], "snare": [BACK], "clap": [BACK], "hat": [HATS16], "crash": CRASH8},
        drum_gain={"kick": 0.75, "snare": 0.4, "clap": 0.3, "hat": 0.3, "crash": 0.3},
        fills={"snare": (4, "....x.......xgxX")},
    )
    done = Part(
        bpm=150, chords="C", seconds=1.9, bars=1,
        tracks=(Track("notes", replace(lead, gate=0.6), "G4 C5 E5 G5 . . . . . . . . . . . ."),
                Track("notes", replace(PULSE50, slide=(-5, 0.06), s=0.7, detune=6), ". . . . . . C6 - - - - - . . . ."),
                Track("notes", sparkle, ". . . . . . . . . . . . . G6 . .", gain=0.9),
                Track("arp", replace(PULSE25, vib=(0, 5, 0), detune=0, s=0.5), ". . . . . . c - - - - - . . . .",
                      gain=0.4, low=60),
                Track("bass", TRI_BASS, ". . . . . . R - - - - - . . . .", low=36)),
        drums={"snare": ["gggX............"], "kick": ["......x........."], "crash": ["......x........."]},
        drum_gain={"snare": 0.4, "kick": 0.6, "crash": 0.35},
    )
    return Theme("manga", "C major (chromatic comic accents)", "punchy, staccato, comedic",
                 tuple(C_MAJ), idle, transfer, done)


def yume() -> Theme:
    chords = "Fmaj7 | G/F | Em7 | Am7 | Dm9 | G/F | Bbmaj7 | Csus4 C"
    hook = """
        A5 - -  C6 E6 - -  .  |
        D6 - B5 -  -  . G5 A5 |
        B5 - -  -  G5 - E5 -  |
        C6 - -  -  -  . E5 G5 |
        A5 - -  C6 E6 - D6 C6 |
        B5 - -  -  D6 - -  .  |
        D6 - C6 -  A5 - F5 -  |
        G5 - -  -  E5 - -  .
    """
    pad = Inst(wave="tri", a=0.35, d=1.0, s=0.8, r=0.4, gate=1.0, detune=11, gain=0.14)
    arp = replace(PULSE12, a=0.004, gate=0.5, s=0.3, gain=0.13)
    idle = Part(
        bpm=92, chords=chords, echo=(0.75, 0.42, 0.45),
        tracks=(Track("notes", FM_FLUTE, hook, send=0.8),
                Track("notes", FM_BELL, hook, transpose=12, gain=0.35, send=0.8),
                Track("arp", arp, "1 2 3 4 5 4 3 2 1 2 3 4 5 4 3 2", send=1.0, low=60),
                Track("arp", pad, "c - - - - - - -", low=53),
                Track("bass", TRI_BASS, "R - - - 5 - - -", gain=0.8, low=36)),
        drums={"kick": ["x.........x....."], "shaker": ["..x...x...x...x."], "snare": ["........g......."]},
        drum_gain={"kick": 0.45, "shaker": 0.25, "snare": 0.2},
    )
    transfer = Part(
        bpm=128, chords=join(chords, chords), bars=16, echo=(0.75, 0.38, 0.4),
        tracks=(Track("notes", FM_FLUTE, join(hook, hook), send=0.6),
                Track("notes", FM_BELL, join(rests(8), hook), transpose=12, gain=0.4, send=0.7),
                Track("notes", replace(FM_FLUTE, gain=0.2), join(rests(8), harmonize(hook, F_LYD, -2)), send=0.5),
                Track("arp", replace(arp, gain=0.12), "1 2 3 4 5 6 5 4 3 4 5 6 7 6 5 4", send=0.9, low=60),
                Track("arp", pad, "c - - - - - - -", gain=0.8, low=53),
                Track("bass", TRI_BASS, "R 5 8 5 R 5 8 5", low=36)),
        drums={"kick": ["x.....x...x....."], "snare": [BACK], "shaker": ["gxgxgxgxgxgxgxgx"],
               "hat": ["..x...x...x...x."]},
        drum_gain={"kick": 0.6, "snare": 0.3, "shaker": 0.25, "hat": 0.2},
    )
    done = Part(
        bpm=120, chords="Fmaj9", seconds=2.4, bars=1, echo=(0.5, 0.4, 0.45),
        tracks=(Track("notes", FM_BELL, "F5 A5 C6 E6 . G6 . A6 - - - - - - - .", send=0.8, gain=1.2),
                Track("notes", FM_FLUTE, ". . . . . . . A5 - - - - - - - .", gain=0.8, send=0.5),
                Track("arp", pad, ". . . . . . c - - - - - - - - .", low=53, gain=1.2),
                Track("bass", TRI_BASS, ". . . . . . R - - - - - - - . .", low=36, gain=0.8)),
        drums={"shaker": ["x.x.x.x........."], "crash": ["......x........."]},
        drum_gain={"shaker": 0.3, "crash": 0.15},
    )
    return Theme("yume", "F lydian (F G A B C D E)", "soft, airy, arpeggios",
                 tuple(F_LYD), idle, transfer, done)


def night() -> Theme:
    chords = "Dm9 | Gm9 | Bbmaj7 | A7 | Dm9 | Gm9 | Ebmaj7 | A7sus4"
    hook = """
        .  A5 C6  -   E6  - D6 .  |
        C6 -  A5  -   -   . F5 G5 |
        A5 -  -   -   F5  - D5 .  |
        E5 -  G5  -   C#6 - -  .  |
        .  A5 C6  -   E6  - F6 E6 |
        D6 -  -   Bb5 -   . A5 G5 |
        G5 -  Bb5 -   D6  - -  .  |
        E5 -  -   D5  -   - .  .
    """
    idle = Part(
        bpm=90, chords=chords, swing=0.58, swing_grid=0.5, echo=(0.75, 0.35, 0.35),
        tracks=(Track("notes", FM_LEAD, hook, send=0.6),
                Track("arp", FM_EP, "c - - - - - . c - - . . . . . .", low=55),
                Track("bass", FM_BASS, "R - - . . . R 5 . . 8 - . . 7 .", low=38)),
        drums={"kick": ["x.....x...x....."], "snare": [BACK], "hat": ["x.xgx.x.x.xgx.x."]},
        drum_gain={"kick": 0.6, "snare": 0.35, "hat": 0.22},
    )
    # Transfer: lo-fi turns into a city-pop disco groove.
    transfer = Part(
        bpm=124, chords=join(chords, chords), bars=16, echo=(0.75, 0.3, 0.3),
        tracks=(Track("notes", FM_LEAD, join(hook, hook), send=0.5),
                Track("notes", replace(PULSE25, gain=0.13), join(rests(8), harmonize(hook, D_MIN, -2)), send=0.4),
                Track("arp", FM_EP, ". . c . . . c . . . c . . . c .", low=55),
                Track("bass", FM_BASS, "R 8 R 8 5 8 7 8", low=38)),
        drums={"kick": [FOUR], "snare": [BACK], "clap": [BACK], "hat": [HATS16], "ohat": ["..x...x...x...x."],
               "crash": CRASH8},
        drum_gain={"kick": 0.7, "snare": 0.35, "clap": 0.25, "hat": 0.25, "ohat": 0.18, "crash": 0.2},
    )
    done = Part(
        bpm=110, chords="Dm9 Dmaj9", seconds=2.4, bars=1, echo=(0.5, 0.35, 0.35),
        tracks=(Track("notes", FM_LEAD, "A5 C6 E6 . D6 . . . F#6 - - - - - - .", send=0.6),
                Track("arp", FM_EP, "c - - - . . . . c - - - - - - -", low=55),
                Track("bass", FM_BASS, "R - - - . . . . R - - - - - - .", low=38)),
        drums={"kick": ["x.......x......."], "snare": ["....x..........."], "crash": ["........x......."]},
        drum_gain={"kick": 0.55, "snare": 0.3, "crash": 0.2},
    )
    return Theme("night", "D minor (dorian/aeolian, m7/m9 chords)", "chill lo-fi / city-pop",
                 tuple(D_MIN), idle, transfer, done)


def portal() -> Theme:
    """Download-portal loop: relaxing, cozy, gently bouncy "home overworld" feel. Original."""
    chords = join("Ebmaj7 | Cm7 | Abmaj7 | Bb7sus4 Bb7 | Ebmaj7 | Gm7 | Abmaj7 Fm7 | Bb7sus4 Bb",
                  "Abmaj7 | Gm7 | Fm7 | Bb7 | Ebadd9 | Cm7 | Fm7 Bb7 | Ebmaj7")
    a = """
        Bb5 - G5  Bb5 C6  - Bb5 .   |
        G5  - Eb5 -   -   . F5  G5  |
        Ab5 - C6  -   Eb6 - D6  C6  |
        Bb5 - -   -   Ab5 - .   .   |
        Bb5 - G5  Bb5 Eb6 - D6  .   |
        D6  - Bb5 -   G5  - F5  G5  |
        C6  - Ab5 -   F5  - Ab5 G5  |
        F5  - -   -   -   . .   .
    """
    b = """
        Eb6 - -   C6  -   Ab5 G5 Ab5 |
        Bb5 - -   G5  -   D5  -  .   |
        C6  - -   Ab5 -   F5  Eb5 F5 |
        D5  - F5  -   Ab5 -   G5 F5  |
        G5  - -   Bb5 -   F5  -  G5  |
        Eb6 - D6  -   C6  -   Bb5 G5 |
        Ab5 - C6  -   Bb5 -   Ab5 -  |
        G5  - -   -   -   -   .  .
    """
    lead = replace(PULSE25, a=0.015, d=0.3, s=0.55, r=0.12, vib=(13, 5.0, 0.22), detune=6, gain=0.24)
    pad = Inst(wave="tri", a=0.4, d=1.2, s=0.75, r=0.5, gate=1.0, detune=9, gain=0.10)
    idle = Part(
        bpm=96, chords=chords, bars=16, lowpass=5200, rms_db=-22.0, echo=(0.75, 0.35, 0.35),
        tracks=(Track("notes", lead, join(a, b), send=0.55),
                Track("notes", replace(FM_FLUTE, gain=0.13), join(rests(8), b), transpose=-12, send=0.4),
                Track("notes", replace(FM_BELL, gain=0.07), join(rests(8), b), transpose=12, send=0.7),
                Track("arp", replace(PULSE12, a=0.006, gate=0.45, gain=0.12), ". c . c . c . c", low=58, send=0.2),
                Track("arp", pad, "c - - - - - - -", low=51),
                Track("bass", replace(FM_BASS, index=1.2, index_end=0.3, gain=0.5), "R . . 5 8 . 5 .", low=39)),
        drums={"kick": ["x.......x......."], "brush": ["....x.......x..."],
               "shaker": ["x.g.x.g.x.g.x.gg"]},
        drum_gain={"kick": 0.45, "brush": 0.3, "shaker": 0.16},
    )
    return Theme("portal", "Eb major (maj7/add9)", "relaxing, cozy, gently bouncy", (3, 5, 7, 8, 10, 0, 2),
                 idle, None, None)


ALL = {t.name: t for t in (purikura(), showa(), manga(), yume(), night())}
assert tuple(ALL) == THEMES
PIECES = {**ALL, "portal": portal()}
SITE_MUSIC = "site/music"

# ------------------------------------------------------------------------- outputs & checks


def wav_u8(x: np.ndarray, sr: int, seed: int) -> bytes:
    rng = np.random.default_rng(seed)
    dither = (rng.random(len(x)) - rng.random(len(x)))       # TPDF, ±1 LSB
    q = np.clip(np.round(x * 127.0 + 128.0 + dither), 0, 255).astype(np.uint8)
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(1)
        w.setframerate(sr)
        w.writeframes(q.tobytes())
    return buf.getvalue()


def mp3_bytes(x: np.ndarray, sr: int) -> bytes:
    buf = io.BytesIO()
    sf.write(buf, x.astype(np.float32), sr, format="MP3", subtype="MPEG_LAYER_III",
             compression_level=MP3_QUALITY, bitrate_mode="VARIABLE")
    return buf.getvalue()


def ogg_bytes(x: np.ndarray, sr: int) -> bytes:
    buf = io.BytesIO()
    sf.write(buf, x.astype(np.float32), sr, format="OGG", subtype="VORBIS",
             compression_level=1 - VORBIS_QUALITY)
    return buf.getvalue()


def verify(name: str, x: np.ndarray, loop: bool) -> list[str]:
    """Quality gate on the float render: clipping, DC, loop seam continuity."""
    errs = []
    peak = float(np.abs(x).max())
    if peak > PEAK + 1e-6:
        errs.append(f"{name}: peak {20 * math.log10(peak):.2f} dBFS > -3")
    if abs(float(x.mean())) > 5e-4:
        errs.append(f"{name}: DC offset {x.mean():.2e}")
    if loop:
        jump = abs(float(x[0] - x[-1]))
        typical = float(np.percentile(np.abs(np.diff(x)), 99.9))
        if jump > typical:
            errs.append(f"{name}: loop seam jump {jump:.4f} > p99.9 step {typical:.4f}")
    else:
        if abs(float(x[-1])) > 1e-3 or abs(float(x[0])) > 1e-3:
            errs.append(f"{name}: jingle does not start/end at silence")
    return errs


def seam_stats(x: np.ndarray) -> tuple[float, float]:
    return abs(float(x[0] - x[-1])), float(np.percentile(np.abs(np.diff(x)), 99.9))


def plan(themes: list[str]):
    """Yield (relative path, theme, which, sr, bars, format)."""
    for name in (t for t in themes if t in ALL):
        for which in ("idle", "transfer", "done"):
            yield f"{RAW}/music_{name}_{which}.ogg", name, which, ANDROID_SR, None, "ogg"
    if "purikura" in themes:
        for which, bars in (("idle", None), ("transfer", DESKTOP_TRANSFER_BARS), ("done", None)):
            yield f"{DESK}/desktop_{which}.wav", "purikura", which, DESKTOP_SR, bars, "wav"
    if "portal" in themes:
        for fmt in ("ogg", "mp3"):
            yield f"{SITE_MUSIC}/portal.{fmt}", "portal", "idle", WEB_SR, None, fmt


def meta(rel: str, name: str, which: str, sr: int, bars: int | None) -> dict:
    t = PIECES[name]
    p: Part = getattr(t, which)
    loop = p.seconds == 0
    nbars = bars or p.bars
    beats = nbars * p.beats_per_bar
    n = round(beats * 60 / p.bpm * sr) if loop else round(p.seconds * sr)
    d = {"file": rel, "resource": Path(rel).name if name == "portal" else Path(rel).stem, "theme": name,
         "kind": "loop" if name == "portal" else which, "loop": loop,
         "sample_rate": sr, "samples": n, "ms": round(n * 1000 / sr, 3), "bpm": p.bpm, "key": t.key}
    if loop:
        d |= {"bars": nbars, "beats_per_bar": p.beats_per_bar, "bar_ms": round(p.beats_per_bar * 60000 / p.bpm, 3)}
    return d


def readme(entries: list[dict]) -> str:
    rows = "\n".join(
        f"| `{e['resource']}` | {e['theme']} | {e['kind']} | {e['key']} | {e['bpm']:g} | "
        f"{e.get('bars', '–')} | {e['ms']:.0f} | {e['sample_rate']} |" for e in entries)
    return f"""# Hoppy music (generated)

Original 16-bit-style chiptunes, one set per theme, synthesised from note data by
`tools/gen_music.py` (no samples). **Do not edit the audio files**: change the script and run

    python3 tools/dev.py music            # or: python3 tools/gen_music.py [--theme NAME]
    python3 tools/gen_music.py --check    # are the committed files up to date?

Needs `pip install numpy soundfile`. Android files live in
`apps/android/app/src/main/res/raw/` (Vorbis, 22050 Hz mono); the desktop set here is the
Purikura music as 8-bit 16 kHz WAV for `PlaySoundW`. The download portal's quieter `portal`
loop is in `site/music/` (Vorbis + MP3, 32 kHz mono, with `portal.json` for the player).
`manifest.json` lists exact lengths.
Loops are sample-exact and seamless; `done` jingles are one-shots. Playback rules are in
`docs/design/themes.md` ("Music").

| Resource | Theme | Kind | Key | BPM | Bars | ms | Hz |
| --- | --- | --- | --- | --- | --- | --- | --- |
{rows}

Licence: original compositions by the Hoppy contributors, MIT like the rest of the repo.
"""


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--theme", choices=(*THEMES, "portal"), action="append")
    ap.add_argument("--out", type=Path, default=ROOT)
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()
    themes = args.theme or [*THEMES, "portal"]
    out: Path = args.out
    errs: list[str] = []
    stale: list[str] = []
    total = 0
    for rel, name, which, sr, bars, fmt in plan(themes):
        part = getattr(PIECES[name], which)
        x = render(PIECES[name], which, sr, bars)
        loop = part.seconds == 0
        x = normalize(x, loop, sr, part.rms_db)
        errs += verify(rel, x, loop)
        want = meta(rel, name, which, sr, bars)["samples"]
        if len(x) != want:
            errs.append(f"{rel}: {len(x)} samples, expected {want}")
        path = out / rel
        if fmt == "wav":
            data = wav_u8(x, sr, seed=len(rel))
        if args.check:
            if not path.exists():
                stale.append(f"{rel}: missing")
                continue
            if fmt == "wav":
                old = path.read_bytes()
                if hashlib.sha256(old).digest() != hashlib.sha256(data).digest():
                    a = np.frombuffer(old[44:], np.uint8).astype(int)
                    b = np.frombuffer(data[44:], np.uint8).astype(int)
                    if len(a) != len(b) or np.mean(np.abs(a - b) > 1) > 1e-3:
                        stale.append(f"{rel}: content differs")
            else:
                y, ysr = sf.read(str(path), dtype="float64")
                corr = float(np.dot(x, y) / (np.linalg.norm(x) * np.linalg.norm(y) + 1e-12)) if len(y) == len(x) else 0
                if ysr != sr or len(y) != len(x) or corr < 0.95:
                    stale.append(f"{rel}: decoded {len(y)} samples @ {ysr} Hz, corr {corr:.3f}")
            continue
        path.parent.mkdir(parents=True, exist_ok=True)
        if fmt == "wav":
            path.write_bytes(data)
        else:
            blob = ogg_bytes(x, sr) if fmt == "ogg" else mp3_bytes(x, sr)
            # Ogg serial numbers are random: keep the old file if it decodes to the same audio.
            same = path.exists() and np.array_equal(sf.read(io.BytesIO(blob))[0], sf.read(str(path))[0])
            if not same:
                path.write_bytes(blob)
            y, _ = sf.read(str(path), dtype="float64")
            if len(y) != len(x):
                errs.append(f"{rel}: {fmt} decodes to {len(y)} samples, expected {len(x)}")
        size = path.stat().st_size
        total += size
        seam = f" seam {seam_stats(x)[0]:.4f}" if loop else ""
        print(f"  {rel:58s} {len(x) / sr:6.2f}s {size / 1024:6.1f} KiB "
              f"peak {20 * math.log10(np.abs(x).max()):5.1f} dB rms {20 * math.log10(loudness(x, loop, sr)):5.1f} dB{seam}")
    entries = [meta(rel, n, w, sr, b) for rel, n, w, sr, b, _ in plan([*THEMES, "portal"])]
    manifest = json.dumps({"generator": "tools/gen_music.py", "files": entries}, indent=2) + "\n"
    web = next(e for e in entries if e["theme"] == "portal")
    player = {k: web[k] for k in ("sample_rate", "samples", "ms", "bpm", "bars", "bar_ms")}
    player["files"] = {"ogg": "music/portal.ogg", "mp3": "music/portal.mp3"}
    docs = {f"{DESK}/manifest.json": manifest, f"{DESK}/README.md": readme(entries),
            f"{SITE_MUSIC}/portal.json": json.dumps(player, indent=2) + "\n"}
    for rel, text in docs.items():
        p = out / rel
        if args.check:
            if not p.exists() or p.read_text(encoding="utf-8") != text:
                stale.append(f"{rel}: differs")
        else:
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(text, encoding="utf-8")
    if errs:
        print("\n".join("✗ " + e for e in errs))
        sys.exit(f"✗ music: {len(errs)} quality problem(s)")
    if args.check:
        if stale:
            print("\n".join("✗ " + s for s in stale))
            sys.exit("✗ music outputs are stale: run python3 tools/dev.py music")
        print("✓ music outputs up to date")
    else:
        print(f"✓ music: {total / 1024:.0f} KiB written")


if __name__ == "__main__":
    main()
