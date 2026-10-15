//! Modal dialogs through `zenity` (GNOME, preinstalled on Ubuntu) or `kdialog` (KDE): the
//! device list of `hoppyw send`, the file picker, error boxes and the accept question when
//! notifications cannot carry buttons.
//!
//! Arguments are built by pure functions (unit-tested) and passed as an argv, never through
//! a shell. Options take their value in the same argument (`--text=…`), and list entries
//! (device names come from the network) never start with `-`, so nothing can be read as
//! an option. zenity's question/error text is shown with `--no-markup`; kdialog guesses
//! rich text from `<…>`, so those characters are swapped for look-alikes there.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use super::which;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Zenity,
    Kdialog,
}

impl Tool {
    fn program(self) -> &'static str {
        match self {
            Tool::Zenity => "zenity",
            Tool::Kdialog => "kdialog",
        }
    }
}

/// zenity everywhere except on KDE, where kdialog fits in (either one if only one exists).
pub fn choose(desktop: &str, has: impl Fn(&str) -> bool) -> Option<Tool> {
    let kde = desktop.split(':').any(|d| d.eq_ignore_ascii_case("KDE"));
    let order = if kde {
        [Tool::Kdialog, Tool::Zenity]
    } else {
        [Tool::Zenity, Tool::Kdialog]
    };
    order.into_iter().find(|t| has(t.program()))
}

/// The dialog program of this desktop, looked up once.
pub fn tool() -> Option<Tool> {
    static TOOL: OnceLock<Option<Tool>> = OnceLock::new();
    *TOOL.get_or_init(|| {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        choose(&desktop, |p| which(p).is_some())
    })
}

/// The accept question needs No as the default button: zenity (`--default-cancel`)
/// whenever it is installed, also on KDE; kdialog only as a last resort.
pub fn question_tool() -> Option<Tool> {
    static TOOL: OnceLock<Option<Tool>> = OnceLock::new();
    *TOOL.get_or_init(|| choose("", |p| which(p).is_some()))
}

/// A positional value that can never be taken for an option.
fn positional(s: &str) -> String {
    if s.starts_with('-') {
        format!(" {s}")
    } else {
        s.to_owned()
    }
}

/// kdialog renders text that looks like HTML as rich text: make `<`/`>` harmless.
fn kde_text(s: &str) -> String {
    s.replace('<', "‹").replace('>', "›")
}

pub fn question_args(
    tool: Tool,
    title: &str,
    text: &str,
    yes: &str,
    no: &str,
    timeout: Duration,
) -> Vec<String> {
    match tool {
        Tool::Zenity => vec![
            "--question".into(),
            "--no-markup".into(),
            "--default-cancel".into(),
            format!("--title={title}"),
            format!("--text={text}"),
            format!("--ok-label={yes}"),
            format!("--cancel-label={no}"),
            format!("--timeout={}", timeout.as_secs().max(1)),
            "--width=420".into(),
        ],
        Tool::Kdialog => vec![
            format!("--title={}", kde_text(title)),
            format!("--yes-label={yes}"),
            format!("--no-label={no}"),
            // KMessageBox's warning variant; documented to focus "No" (unlike --yesno,
            // whose default is Yes). Verify on KDE (docs/platforms/linux.md test plan).
            format!("--warningyesno={}", kde_text(text)),
        ],
    }
}

pub fn error_args(tool: Tool, title: &str, text: &str) -> Vec<String> {
    match tool {
        Tool::Zenity => vec![
            "--error".into(),
            "--no-markup".into(),
            format!("--title={title}"),
            format!("--text={text}"),
            "--width=420".into(),
        ],
        Tool::Kdialog => vec![
            format!("--title={}", kde_text(title)),
            format!("--error={}", kde_text(text)),
        ],
    }
}

/// A list to pick one entry from; prints the chosen id (`parse_choice`).
pub fn list_args(tool: Tool, title: &str, text: &str, rows: &[(u32, &str)]) -> Vec<String> {
    let mut args = match tool {
        Tool::Zenity => vec![
            "--list".into(),
            format!("--title={title}"),
            format!("--text={text}"),
            "--column=id".into(),
            "--column=".into(),
            "--hide-column=1".into(),
            "--print-column=1".into(),
            "--hide-header".into(),
            "--width=440".into(),
            "--height=380".into(),
        ],
        Tool::Kdialog => vec![
            format!("--title={}", kde_text(title)),
            format!("--menu={}", kde_text(text)),
        ],
    };
    for (id, label) in rows {
        args.push(id.to_string());
        args.push(positional(&match tool {
            Tool::Zenity => (*label).to_owned(),
            Tool::Kdialog => kde_text(label),
        }));
    }
    args
}

/// The id printed by a list dialog, if it is one of `rows`.
pub fn parse_choice(stdout: &str, rows: &[(u32, &str)]) -> Option<u32> {
    // zenity ≤ 3.x printed the column twice when double-clicked ("100|100").
    let first = stdout.trim().split('|').next()?;
    let id = first.parse::<u32>().ok()?;
    rows.iter().any(|(r, _)| *r == id).then_some(id)
}

pub fn files_args(tool: Tool, title: &str, start: &str) -> Vec<String> {
    match tool {
        Tool::Zenity => vec![
            "--file-selection".into(),
            "--multiple".into(),
            "--separator=\n".into(),
            format!("--title={title}"),
        ],
        Tool::Kdialog => vec![
            format!("--title={}", kde_text(title)),
            "--getopenfilename".into(),
            positional(start),
            "--multiple".into(),
            "--separate-output".into(),
        ],
    }
}

/// One path per line (a file name containing a newline would be split and then rejected
/// as "not a file", which is acceptable).
pub fn parse_files(stdout: &str) -> Vec<PathBuf> {
    stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Runs the dialog; `(exit code, stdout)`, or `None` if it could not start or was killed
/// after `timeout`.
pub fn run(tool: Tool, args: &[String], timeout: Option<Duration>) -> Option<(i32, String)> {
    let mut child = Command::new(tool.program())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Read while it runs: a long file selection could fill the pipe.
    let reader = thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if timeout.is_some_and(|t| started.elapsed() >= t) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(_) => break None,
        }
    };
    let out = reader.join().unwrap_or_default();
    Some((status?.code().unwrap_or(-1), out))
}

/// Yes/No question (No is the default). `None` if no dialog program is installed.
pub fn ask(title: &str, text: &str, yes: &str, no: &str, timeout: Duration) -> Option<bool> {
    let tool = question_tool()?;
    let args = question_args(tool, title, text, yes, no, timeout);
    // kdialog has no timeout of its own; zenity gets a little longer than its own.
    let result = run(tool, &args, Some(timeout + Duration::from_secs(2)));
    Some(matches!(result, Some((0, _))))
}

/// Error box; false if no dialog program is installed.
pub fn error(title: &str, text: &str) -> bool {
    let Some(tool) = tool() else { return false };
    run(tool, &error_args(tool, title, text), None).is_some()
}

/// Pick one of `rows`: `Some(choice)` (`None` choice = cancelled), or `None` if no dialog
/// program is installed.
pub fn choose_from(title: &str, text: &str, rows: &[(u32, &str)]) -> Option<Option<u32>> {
    let tool = tool()?;
    let result = run(tool, &list_args(tool, title, text, rows), None);
    Some(result.and_then(|(code, out)| (code == 0).then(|| parse_choice(&out, rows)).flatten()))
}

/// Multi-select file picker: `Some(files)` (empty = cancelled), `None` without a dialog
/// program.
pub fn pick_files(title: &str) -> Option<Vec<PathBuf>> {
    let tool = tool()?;
    let start = dirs::home_dir()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".into());
    let result = run(tool, &files_args(tool, title, &start), None);
    Some(match result {
        Some((0, out)) => parse_files(&out),
        _ => Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_desktop_tool() {
        let both = |_: &str| true;
        assert_eq!(choose("ubuntu:GNOME", both), Some(Tool::Zenity));
        assert_eq!(choose("KDE", both), Some(Tool::Kdialog));
        assert_eq!(choose("KDE", |p| p == "zenity"), Some(Tool::Zenity));
        assert_eq!(
            choose("X-Cinnamon", |p| p == "kdialog"),
            Some(Tool::Kdialog)
        );
        assert_eq!(choose("GNOME", |_| false), None);
    }

    #[test]
    fn list_entries_never_look_like_options() {
        let rows = [(100, "--help"), (101, "-x evil"), (1, "Search again")];
        let args = list_args(Tool::Zenity, "Hoppy", "Send to:", &rows);
        let tail: Vec<&str> = args.iter().map(String::as_str).skip(10).collect();
        assert_eq!(
            tail,
            ["100", " --help", "101", " -x evil", "1", "Search again"]
        );
        // Every option carries its value; no bare "--" (zenity would show it as a row).
        assert!(
            args.iter()
                .take(10)
                .all(|a| a.starts_with("--") && a != "--")
        );
        let kde = list_args(Tool::Kdialog, "Hoppy", "<b>to</b>:", &[(7, "<i>Pixel</i>")]);
        assert_eq!(
            kde,
            ["--title=Hoppy", "--menu=‹b›to‹/b›:", "7", "‹i›Pixel‹/i›"]
        );
    }

    #[test]
    fn parses_choices() {
        let rows = [(100, "a"), (1, "b")];
        assert_eq!(parse_choice("100\n", &rows), Some(100));
        assert_eq!(parse_choice("1|1\n", &rows), Some(1));
        assert_eq!(parse_choice("7\n", &rows), None);
        assert_eq!(parse_choice("", &rows), None);
    }

    #[test]
    fn questions_default_to_no_and_show_plain_text() {
        let args = question_args(
            Tool::Zenity,
            "Hoppy",
            "<b>Mario</b> & co",
            "Accept",
            "Decline",
            Duration::from_secs(110),
        );
        assert!(args.contains(&"--no-markup".to_string()));
        assert!(args.contains(&"--default-cancel".to_string()));
        assert!(args.contains(&"--text=<b>Mario</b> & co".to_string()));
        assert!(args.contains(&"--timeout=110".to_string()));
        let kde = question_args(Tool::Kdialog, "T", "<b>x</b>", "Y", "N", Duration::ZERO);
        assert_eq!(kde[3], "--warningyesno=‹b›x‹/b›");
    }

    #[test]
    fn parses_file_lists() {
        assert_eq!(
            parse_files("/home/a/x y.txt\n/home/a/é.png\n"),
            [
                PathBuf::from("/home/a/x y.txt"),
                PathBuf::from("/home/a/é.png")
            ]
        );
        assert!(parse_files("").is_empty());
        let kde = files_args(Tool::Kdialog, "T", "-weird");
        assert_eq!(kde[2], " -weird");
    }
}
