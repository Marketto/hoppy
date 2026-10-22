//! `hoppyw --smoke-test-window [--hold MS] [--theme ID] [--lang it|en]`: opens the real
//! send window, waits for the page, shows every sample screen ([`super::sample_views`]) in
//! every theme (or one), printing `shown <theme> <screen>` for each, then closes. Exit code
//! 0 = the webview started, loaded the page and took every render; 2 = it could not start
//! (the case where `hoppyw send` falls back to the native picker). CI runs it on macOS and
//! (under Xvfb) Linux; screenshots are taken by grabbing the screen after each line.
//!
//! `hoppyw --dump-views [--lang it|en]` prints the sample views as JSON (no window), for
//! `tools/sendui_audit.py` (axe-core + contrast in Chromium).

use std::io::Write;
use std::thread;
use std::time::Duration;

use super::window::{self, Input};
use super::{Init, sample_views};
use crate::gui::{EN, IT, Strings};
use crate::theme::Theme;

fn arg<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn strings(args: &[String]) -> &'static Strings {
    match arg(args, "--lang") {
        Some("en") => &EN,
        Some(_) => &IT,
        None => crate::gui::strings(),
    }
}

/// `--dump-views`: `[{"name":…, "view":…}, …]` on stdout.
pub fn dump(args: &[String]) -> i32 {
    let s = strings(args);
    let views: Vec<serde_json::Value> = sample_views(s)
        .into_iter()
        .map(|(name, view)| serde_json::json!({ "name": name, "view": view }))
        .collect();
    let init = Init::new(s, Theme::Purikura);
    let out = serde_json::json!({ "lang": init.lang, "title": init.title, "views": views });
    println!("{out}");
    0
}

/// `--smoke-test-window`.
pub fn run(args: &[String]) -> i32 {
    let s = strings(args);
    let hold = arg(args, "--hold")
        .and_then(|v| v.parse().ok())
        .unwrap_or(150u64);
    let themes: Vec<Theme> = match arg(args, "--theme").and_then(Theme::from_id) {
        Some(t) => vec![t],
        None => Theme::ALL.to_vec(),
    };
    let views = sample_views(s);
    let steps: Vec<(Theme, usize)> = themes
        .iter()
        .flat_map(|&t| (0..views.len()).map(move |i| (t, i)))
        .collect();
    let mut next = 0usize;
    let mut ready = false;
    let result = window::run(
        s.ui_window_title,
        themes[0],
        &Init::new(s, themes[0]),
        |ctl, input| {
            match input {
                Input::Started => ctl.render(&views[0].1),
                Input::Ready => {
                    ready = true;
                    let waker = ctl.waker();
                    thread::spawn(move || {
                        loop {
                            thread::sleep(Duration::from_millis(hold));
                            waker.wake();
                        }
                    });
                }
                Input::Wake if ready => {
                    let Some(&(theme, i)) = steps.get(next) else {
                        ctl.exit();
                        return;
                    };
                    let mut init = Init::new(s, theme);
                    init.title = format!("{} · {}", s.ui_window_title, theme.label());
                    ctl.reinit(&init);
                    ctl.render(&views[i].1);
                    println!("shown {} {}", theme.id(), views[i].0);
                    let _ = std::io::stdout().flush();
                    next += 1;
                }
                // A person may click: log it, never act on it.
                Input::Action(a) => println!("action {a:?}"),
                Input::Wake => {}
            }
        },
    );
    match result {
        Ok(()) if next == steps.len() => {
            println!("smoke window OK: {} screen(s)", steps.len());
            0
        }
        Ok(()) => {
            println!("smoke window: closed early after {next} screen(s)");
            1
        }
        Err(e) => {
            println!("smoke window: unavailable: {e}");
            2
        }
    }
}
