# Hoppy music (generated)

Original 16-bit-style chiptunes, one set per theme, synthesised from note data by
`tools/gen_music.py` (no samples). **Do not edit the audio files**: change the script and run

    python3 tools/dev.py music            # or: python3 tools/gen_music.py [--theme NAME]
    python3 tools/gen_music.py --check    # are the committed files up to date?

Needs `pip install numpy soundfile`. Android files live in
`apps/android/app/src/main/res/raw/` (Vorbis, 22050 Hz mono); the desktop set here is the
Purikura music as 8-bit 16 kHz WAV for `PlaySoundW`. `manifest.json` lists exact lengths.
Loops are sample-exact and seamless; `done` jingles are one-shots. Playback rules are in
`docs/design/themes.md` ("Music").

| Resource | Theme | Kind | Key | BPM | Bars | ms | Hz |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `music_purikura_idle` | purikura | idle | D major | 124 | 8 | 15484 | 22050 |
| `music_purikura_transfer` | purikura | transfer | D major | 172 | 16 | 22326 | 22050 |
| `music_purikura_done` | purikura | done | D major | 160 | – | 2000 | 22050 |
| `music_showa_idle` | showa | idle | G major (yonanuki: G A B D E) | 112 | 8 | 17143 | 22050 |
| `music_showa_transfer` | showa | transfer | G major (yonanuki: G A B D E) | 156 | 16 | 24615 | 22050 |
| `music_showa_done` | showa | done | G major (yonanuki: G A B D E) | 140 | – | 2100 | 22050 |
| `music_manga_idle` | manga | idle | C major (chromatic comic accents) | 128 | 8 | 15000 | 22050 |
| `music_manga_transfer` | manga | transfer | C major (chromatic comic accents) | 180 | 16 | 21333 | 22050 |
| `music_manga_done` | manga | done | C major (chromatic comic accents) | 150 | – | 1900 | 22050 |
| `music_yume_idle` | yume | idle | F lydian (F G A B C D E) | 92 | 8 | 20870 | 22050 |
| `music_yume_transfer` | yume | transfer | F lydian (F G A B C D E) | 128 | 16 | 30000 | 22050 |
| `music_yume_done` | yume | done | F lydian (F G A B C D E) | 120 | – | 2400 | 22050 |
| `music_night_idle` | night | idle | D minor (dorian/aeolian, m7/m9 chords) | 90 | 8 | 21333 | 22050 |
| `music_night_transfer` | night | transfer | D minor (dorian/aeolian, m7/m9 chords) | 124 | 16 | 30968 | 22050 |
| `music_night_done` | night | done | D minor (dorian/aeolian, m7/m9 chords) | 110 | – | 2400 | 22050 |
| `desktop_idle` | purikura | idle | D major | 124 | 8 | 15484 | 16000 |
| `desktop_transfer` | purikura | transfer | D major | 172 | 8 | 11163 | 16000 |
| `desktop_done` | purikura | done | D major | 160 | – | 2000 | 16000 |

Licence: original compositions by the Hoppy contributors, MIT like the rest of the repo.
