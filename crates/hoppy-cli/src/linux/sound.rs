//! The delivery jingle (`assets/music/desktop_done.wav`, embedded) through the first player
//! found: `pw-play` (PipeWire, Ubuntu 22.10+), `paplay` (PulseAudio), `aplay` (ALSA).
//! The WAV is written to a private file in `$XDG_RUNTIME_DIR` while it plays. One sound per
//! process, like `PlaySoundW`: a new one replaces the current one.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use super::which;
use crate::diag;

static DONE_JINGLE: &[u8] = include_bytes!("../../../../assets/music/desktop_done.wav");

/// (program, arguments before the file), in order of preference.
pub const PLAYERS: [(&str, &[&str]); 3] =
    [("pw-play", &[]), ("paplay", &[]), ("aplay", &["--quiet"])];

struct Playing {
    child: Child,
    file: PathBuf,
}

static PLAYING: Mutex<Option<Playing>> = Mutex::new(None);

/// The first available player.
pub fn player(has: impl Fn(&str) -> bool) -> Option<(&'static str, &'static [&'static str])> {
    PLAYERS.into_iter().find(|(p, _)| has(p))
}

/// Plays the jingle without waiting; silently does nothing without a player.
pub fn play_done_jingle() {
    stop_sound();
    let Some((program, args)) = player(|p| which(p).is_some()) else {
        diag::log("sound: no player found (pw-play, paplay, aplay)");
        return;
    };
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let file = dir.join(format!("hoppy-done-{}.wav", std::process::id()));
    // A leftover (or planted) file or symlink is replaced, never written through.
    let _ = std::fs::remove_file(&file);
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
        .and_then(|mut f| f.write_all(DONE_JINGLE));
    if let Err(e) = written {
        diag::log(format!("sound: cannot write the jingle: {e}"));
        return;
    }
    let child = Command::new(program)
        .args(args)
        .arg(&file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match child {
        Ok(child) => {
            *PLAYING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Playing { child, file });
        }
        Err(e) => {
            diag::log(format!("sound: {program} failed: {e}"));
            let _ = std::fs::remove_file(&file);
        }
    }
}

/// Stops the jingle (if still playing) and removes its file.
pub fn stop_sound() {
    let playing = PLAYING.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(mut p) = playing {
        if matches!(p.child.try_wait(), Ok(None)) {
            let _ = p.child.kill();
        }
        let _ = p.child.wait();
        let _ = std::fs::remove_file(&p.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_pipewire_then_pulse_then_alsa() {
        assert_eq!(player(|_| true).map(|p| p.0), Some("pw-play"));
        assert_eq!(player(|p| p != "pw-play").map(|p| p.0), Some("paplay"));
        assert_eq!(player(|p| p == "aplay"), Some(("aplay", &["--quiet"][..])));
        assert_eq!(player(|_| false), None);
    }

    #[test]
    fn embedded_jingle_is_a_wav() {
        assert_eq!(&DONE_JINGLE[..4], b"RIFF");
        assert_eq!(&DONE_JINGLE[8..12], b"WAVE");
    }
}
