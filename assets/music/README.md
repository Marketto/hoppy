# Hoppy music (generated)

Original 16-bit-style chiptunes, one set per theme, synthesised from note data by
`tools/gen_music.py` (no samples). **Do not edit the audio files**: change the script and run

    python3 tools/dev.py music            # or: python3 tools/gen_music.py [--theme NAME]
    python3 tools/gen_music.py --check    # are the committed files up to date?

Needs `pip install numpy soundfile`. Android files live in
`apps/android/app/src/main/res/raw/` (Vorbis, 22050 Hz mono); the desktop set here is the
Purikura music as 8-bit WAV for `PlaySoundW` (installer loop 12 kHz, delivery jingle 16 kHz). The download portal's quieter `portal`
loop is in `site/music/` (Vorbis + MP3, 32 kHz mono, with `portal.json` for the player).
`manifest.json` lists exact lengths.
Loops are sample-exact and seamless; `done` jingles are one-shots. Levels (RMS): loops
−24 dB, jingles −21 dB, installer `desktop_idle.wav` and the portal −26 dB; peaks ≤ −3 dBFS. Playback rules are in
`docs/design/themes.md` ("Music").

| Resource | Theme | Kind | Key | BPM | Bars | ms | Hz |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `music_purikura_idle` | purikura | idle | D major | 108 | 8 | 17778 | 22050 |
| `music_purikura_transfer` | purikura | transfer | D major | 135 | 16 | 28444 | 22050 |
| `music_purikura_done` | purikura | done | D major | 160 | – | 2000 | 22050 |
| `music_showa_idle` | showa | idle | G major (yonanuki: G A B D E) | 98 | 8 | 19592 | 22050 |
| `music_showa_transfer` | showa | transfer | G major (yonanuki: G A B D E) | 122 | 16 | 31475 | 22050 |
| `music_showa_done` | showa | done | G major (yonanuki: G A B D E) | 140 | – | 2100 | 22050 |
| `music_manga_idle` | manga | idle | C major (chromatic comic accents) | 112 | 8 | 17143 | 22050 |
| `music_manga_transfer` | manga | transfer | C major (chromatic comic accents) | 140 | 16 | 27429 | 22050 |
| `music_manga_done` | manga | done | C major (chromatic comic accents) | 150 | – | 1900 | 22050 |
| `music_yume_idle` | yume | idle | F lydian (F G A B C D E) | 80 | 8 | 24000 | 22050 |
| `music_yume_transfer` | yume | transfer | F lydian (F G A B C D E) | 100 | 16 | 38400 | 22050 |
| `music_yume_done` | yume | done | F lydian (F G A B C D E) | 120 | – | 2400 | 22050 |
| `music_night_idle` | night | idle | D minor (dorian/aeolian, m7/m9 chords) | 78 | 8 | 24615 | 22050 |
| `music_night_transfer` | night | transfer | D minor (dorian/aeolian, m7/m9 chords) | 98 | 16 | 39184 | 22050 |
| `music_night_done` | night | done | D minor (dorian/aeolian, m7/m9 chords) | 110 | – | 2400 | 22050 |
| `desktop_idle` | purikura | idle | D major | 108 | 8 | 17778 | 12000 |
| `desktop_done` | purikura | done | D major | 160 | – | 2000 | 16000 |
| `portal.ogg` | portal | loop | Eb major (maj7/add9) | 84 | 16 | 45714 | 32000 |
| `portal.mp3` | portal | loop | Eb major (maj7/add9) | 84 | 16 | 45714 | 32000 |

Licence: original compositions by Marco Ricupero, MIT like the rest of the repo.
