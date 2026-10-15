//! Modal dialogs through `zenity` (GNOME, preinstalled on Ubuntu) or `kdialog` (KDE): the
//! device list of `hoppyw send`, the file picker, error boxes and the accept question when
//! notifications cannot carry buttons.
//!
//! Arguments are built by pure functions (unit-tested) and passed as an argv, never through
//! a shell. Options take their value in the same argument (`--text=…`), and list entries
//! (device names come from the network) never start with `-`, so nothing can be read as
//! an option. zenity's question/error text is shown with `--no-markup`; kdialog guesses
//! rich text from `<…>`, so those characters are swapped for look-alikes there.
//!
//! The verification dialog (`dialog_args`) is the one place with Pango markup on: large
//! emoji and the QR code drawn with Unicode half blocks in a black-on-white monospace span
//! (it scans like a picture). Every text in it goes through `markup_escape` first.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use super::which;
use crate::verify::DialogView;

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

/// Text for Pango markup (`&`, `<`, `>`, quotes).
pub fn markup_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// Pango sizes in 1/1024 pt (the integer form works with every Pango version): emoji,
/// digits, and the QR code's half blocks (7 pt: about 4 px per module at 96 dpi).
const EMOJI_SIZE: u32 = 30 * 1024;
const DIGITS_SIZE: u32 = 26 * 1024;
const QR_SIZE: u32 = 7 * 1024;

/// What the verification dialog shows this time (it is re-opened to switch).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DialogState {
    /// The code as digits instead of emoji.
    pub digits: bool,
    /// The QR code was confirmed: ✓ text instead of the code.
    pub verified: bool,
}

/// The Pango markup of the verification dialog's text.
pub fn dialog_markup(view: &DialogView, state: DialogState) -> String {
    let mut parts = vec![markup_escape(&view.text)];
    if !view.note.is_empty() {
        parts.push(markup_escape(&view.note));
    }
    if let Some(code) = &view.code {
        parts.push(if state.digits {
            format!(
                "<span size=\"{DIGITS_SIZE}\" font_family=\"monospace\"><b>{}</b></span>",
                markup_escape(&code.digits)
            )
        } else {
            format!(
                "<span size=\"{EMOJI_SIZE}\">{}</span>\n<small>{}</small>",
                markup_escape(&code.emoji_row()),
                markup_escape(&code.names_row())
            )
        });
    }
    if let Some(qr) = &view.qr {
        parts.push(if state.verified {
            format!("<b>{}</b>", markup_escape(&qr.verified))
        } else {
            // Black on white whatever the theme, so the quiet zone stays light.
            format!(
                "<span font_family=\"monospace\" size=\"{QR_SIZE}\" background=\"white\" \
                 foreground=\"black\">{}</span>\n{}",
                qr.matrix.half_blocks(),
                markup_escape(&qr.caption)
            )
        });
    }
    parts.join("\n\n")
}

/// The code toggle's label for `state` ("123" while the emoji show).
fn toggle_label(view: &DialogView, state: DialogState) -> Option<&str> {
    let code = view.code.as_ref()?;
    Some(if state.digits {
        &code.to_emoji
    } else {
        &code.to_digits
    })
}

/// Arguments of the verification dialog. zenity: a question whose Cancel button is the
/// view's first (default) button, OK the second, then the others and the code toggle as
/// extra buttons (printed on stdout when clicked). kdialog (no markup, no extra buttons):
/// a warning question with the first two buttons and everything as plain text.
pub fn dialog_args(
    tool: Tool,
    view: &DialogView,
    state: DialogState,
    timeout: Option<Duration>,
) -> Vec<String> {
    let label = |i: usize| view.buttons.get(i).map_or("", |b| b.1.as_str());
    match tool {
        Tool::Zenity => {
            let mut args = if view.buttons.len() >= 2 {
                vec![
                    "--question".to_owned(),
                    "--default-cancel".into(),
                    format!("--cancel-label={}", label(0)),
                    format!("--ok-label={}", label(1)),
                ]
            } else {
                vec!["--info".to_owned(), format!("--ok-label={}", label(0))]
            };
            args.extend([
                format!("--title={}", view.title),
                format!("--text={}", dialog_markup(view, state)),
                "--width=460".into(),
            ]);
            for (_, extra) in view.buttons.iter().skip(2) {
                args.push(format!("--extra-button={extra}"));
            }
            if let Some(toggle) = toggle_label(view, state) {
                args.push(format!("--extra-button={toggle}"));
            }
            if let Some(t) = timeout {
                args.push(format!("--timeout={}", t.as_secs().max(1)));
            }
            args
        }
        Tool::Kdialog => {
            let text = kde_text(&view.plain_text());
            let mut args = vec![format!("--title={}", kde_text(&view.title))];
            if view.buttons.len() >= 2 {
                args.extend([
                    format!("--yes-label={}", label(1)),
                    format!("--no-label={}", label(0)),
                    format!("--warningyesno={text}"),
                ]);
            } else {
                args.push(format!("--msgbox={text}"));
            }
            args
        }
    }
}

/// How the verification dialog was answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Button(u32),
    /// The code toggle: show it again the other way.
    Toggle,
    /// Timed out or failed: the caller uses the default.
    Closed,
}

/// Maps a dialog's exit code and stdout back to `view`'s buttons.
pub fn parse_answer(
    tool: Tool,
    view: &DialogView,
    state: DialogState,
    code: i32,
    stdout: &str,
) -> Answer {
    let id = |i: usize| view.buttons.get(i).map(|b| b.0);
    let default = id(0).map_or(Answer::Closed, Answer::Button);
    let affirmative = || id(1).or(id(0)).map_or(Answer::Closed, Answer::Button);
    match (tool, code) {
        (_, 0) => affirmative(),
        // zenity: Cancel, the window's close button, or an extra button (its label printed).
        (Tool::Zenity, 1) => {
            let printed = stdout.trim_end_matches(['\n', '\r']);
            if printed.is_empty() {
                return default;
            }
            if toggle_label(view, state) == Some(printed) {
                return Answer::Toggle;
            }
            view.buttons
                .iter()
                .skip(2)
                .find(|(_, l)| l == printed)
                .map_or(default, |(id, _)| Answer::Button(*id))
        }
        (Tool::Kdialog, 1) => default,
        _ => Answer::Closed, // zenity 5 = timeout, or the dialog failed
    }
}

/// How `run_watch` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ran {
    Exited(i32, String),
    /// `stop` returned true: the dialog was closed by us.
    Stopped,
    /// Could not start, killed after the timeout, or ended by a signal.
    Failed,
}

/// Runs the dialog; `(exit code, stdout)`, or `None` if it could not start or was killed
/// after `timeout`.
pub fn run(tool: Tool, args: &[String], timeout: Option<Duration>) -> Option<(i32, String)> {
    match run_watch(tool, args, timeout, &|| false) {
        Ran::Exited(code, out) => Some((code, out)),
        Ran::Stopped | Ran::Failed => None,
    }
}

/// [`run`], also closing the dialog as soon as `stop()` is true (checked every 50 ms).
pub fn run_watch(
    tool: Tool,
    args: &[String],
    timeout: Option<Duration>,
    stop: &dyn Fn() -> bool,
) -> Ran {
    run_program_watch(tool.program(), args, timeout, stop)
}

fn run_program_watch(
    program: &str,
    args: &[String],
    timeout: Option<Duration>,
    stop: &dyn Fn() -> bool,
) -> Ran {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Ran::Failed;
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Ran::Failed;
    };
    // Read while it runs: a long file selection could fill the pipe.
    let reader = thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    });
    let started = Instant::now();
    let ended = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if stop() => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Ran::Stopped;
            }
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
    match ended {
        Some(code) => Ran::Exited(code, out),
        None => Ran::Failed,
    }
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

    fn verify_view() -> DialogView {
        use crate::verify::{CodeView, QrMatrix, QrView};
        let sas = hoppy_core::Sas::from_value(0x05_1234);
        DialogView {
            title: "Hoppy — incoming files".into(),
            text: "<b>Mario</b> & co wants to send you 1 file".into(),
            note: "🔒 Encrypted · new device".into(),
            code: Some(CodeView::new(&sas, false, "123", "Emoji")),
            qr: Some(QrView {
                matrix: QrMatrix::encode("hoppy:verify:1:id:FP:00").unwrap(),
                caption: "Scan with Hoppy on the phone".into(),
                verified: "✓ Verified".into(),
            }),
            buttons: vec![
                (1, "Decline".into()),
                (2, "Accept".into()),
                (3, "They match, accept".into()),
            ],
            details: None,
            timeout: Some(Duration::from_secs(110)),
        }
    }

    #[test]
    fn verification_dialog_escapes_text_and_defaults_to_the_first_button() {
        let view = verify_view();
        let args = dialog_args(Tool::Zenity, &view, DialogState::default(), view.timeout);
        assert_eq!(
            args[..4],
            [
                "--question",
                "--default-cancel",
                "--cancel-label=Decline",
                "--ok-label=Accept"
            ]
        );
        assert!(!args.contains(&"--no-markup".to_owned()));
        let text = args.iter().find(|a| a.starts_with("--text=")).unwrap();
        assert!(
            text.starts_with("--text=&lt;b&gt;Mario&lt;/b&gt; &amp; co wants"),
            "{text}"
        );
        let code = view.code.as_ref().unwrap();
        let row = format!("<span size=\"30720\">{}</span>", code.emoji_row());
        assert!(text.contains(&row), "{text}");
        assert!(text.contains("background=\"white\""));
        assert!(text.contains(&view.qr.as_ref().unwrap().matrix.half_blocks()));
        assert!(text.ends_with("\nScan with Hoppy on the phone"));
        assert_eq!(
            args[args.len() - 3..],
            [
                "--extra-button=They match, accept",
                "--extra-button=123",
                "--timeout=110"
            ]
        );
        // Digits, verified: the toggle flips, ✓ replaces the code.
        let state = DialogState {
            digits: true,
            verified: true,
        };
        let args = dialog_args(Tool::Zenity, &view, state, None);
        let text = args.iter().find(|a| a.starts_with("--text=")).unwrap();
        assert!(text.contains(&format!("<b>{}</b>", code.digits)), "{text}");
        assert!(text.ends_with("<b>✓ Verified</b>") && !text.contains('█'));
        assert!(args.contains(&"--extra-button=Emoji".to_owned()));
        assert!(!args.iter().any(|a| a.starts_with("--timeout")));
        // kdialog: plain text, No (= Decline) focused.
        let kde = dialog_args(Tool::Kdialog, &view, DialogState::default(), None);
        assert_eq!(kde[1..3], ["--yes-label=Accept", "--no-label=Decline"]);
        assert!(kde[3].starts_with("--warningyesno=‹b›Mario‹/b› & co"));
        assert!(kde[3].contains(&view.code.as_ref().unwrap().digits));
    }

    #[test]
    fn verification_answers_map_back_to_buttons() {
        let view = verify_view();
        let s = DialogState::default();
        let z = |code, out| parse_answer(Tool::Zenity, &view, s, code, out);
        assert_eq!(z(0, ""), Answer::Button(2));
        assert_eq!(z(1, ""), Answer::Button(1));
        assert_eq!(z(1, "They match, accept\n"), Answer::Button(3));
        assert_eq!(z(1, "123\n"), Answer::Toggle);
        assert_eq!(
            z(1, "Emoji\n"),
            Answer::Button(1),
            "not the toggle in this state"
        );
        assert_eq!(z(1, "something else\n"), Answer::Button(1));
        assert_eq!(z(5, ""), Answer::Closed);
        assert_eq!(z(-1, ""), Answer::Closed);
        let k = |code| parse_answer(Tool::Kdialog, &view, s, code, "");
        assert_eq!(
            (k(0), k(1), k(2)),
            (Answer::Button(2), Answer::Button(1), Answer::Closed)
        );
        assert_eq!(markup_escape("a<b>&\"'"), "a&lt;b&gt;&amp;&quot;&apos;");
    }

    #[test]
    fn watched_dialogs_can_be_stopped() {
        // Any program will do (a real zenity needs a display).
        let started = Instant::now();
        let ran = run_program_watch("sleep", &["5".into()], None, &|| {
            started.elapsed() > Duration::from_millis(100)
        });
        assert_eq!(ran, Ran::Stopped);
        assert!(started.elapsed() < Duration::from_secs(3));
        let ran = run_program_watch(
            "sh",
            &["-c".into(), "echo hi; exit 3".into()],
            None,
            &|| false,
        );
        assert_eq!(ran, Ran::Exited(3, "hi\n".into()));
        assert_eq!(
            run_program_watch("surely-not-a-program", &[], None, &|| false),
            Ran::Failed
        );
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
