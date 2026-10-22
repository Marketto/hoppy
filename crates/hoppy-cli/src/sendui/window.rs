//! The send window itself: a `tao` window holding a `wry` webview (WebView2 on Windows,
//! WKWebView on macOS, WebKitGTK on Linux) that shows [`super::web`]'s page.
//!
//! Main thread only (macOS and GTK require it). [`run`] owns the event loop until the
//! window closes; `gui.rs` passes a handler that gets [`Input`]s (the page is ready, a
//! validated [`Action`], a worker's wake-up) and answers through [`Control`] (render a
//! [`View`], keep on top, close). Workers wake the loop with a [`Waker`].
//!
//! [`run`] returns [`Unavailable`] *before anything was shown* when the webview cannot
//! start (no WebView2 runtime, no display, WebKitGTK failing, the page not loading within
//! [`READY_TIMEOUT`]); `gui.rs` then uses the native device picker, so sending never
//! breaks. The window stays hidden until the page says it is ready (no blank flash).
//!
//! Security (ADR 0011): the page and its fonts come from the binary (`hoppy:` protocol);
//! navigation elsewhere, new windows, devtools, zoom hotkeys, clipboard access and
//! autoplay are off; IPC messages are only taken from our own page and are parsed by
//! [`super::parse_action`] (anything unexpected is logged by kind and dropped).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::{Icon, Window, WindowBuilder};
use wry::http::{Request, Response, StatusCode, header};
use wry::{NewWindowResponse, WebContext, WebView, WebViewBuilder};

use super::web;
use super::{Action, Init, View, parse_action, script_call};
use crate::diag;
use crate::theme::Theme;

/// The page must say "ready" within this, else [`run`] gives up (native picker instead).
pub const READY_TIMEOUT: Duration = Duration::from_secs(10);
/// Logical size of the window (points / DIPs).
pub const WIDTH: f64 = 420.0;
pub const HEIGHT: f64 = 580.0;

#[cfg(not(target_os = "macos"))]
static ICON_PNG: &[u8] = include_bytes!("../../../../assets/brand/linux/hoppy-128.png");

/// Events for the loop from the webview and from worker threads.
#[derive(Debug)]
pub enum UserEvent {
    Wake,
    Ipc(String),
}

/// Lets worker threads wake the window's loop (`Input::Wake`). Cheap to clone.
#[derive(Clone)]
pub struct Waker(EventLoopProxy<UserEvent>);

impl Waker {
    pub fn wake(&self) {
        let _ = self.0.send_event(UserEvent::Wake);
    }
}

/// What the handler is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// The loop is about to start (the window is still hidden): start the workers.
    Started,
    /// The page loaded; it now shows the last rendered view, the window is visible.
    Ready,
    /// A validated message from the page (the close button arrives as `Cancel`).
    Action(Action),
    /// A worker called [`Waker::wake`].
    Wake,
}

/// The webview could not start; nothing was shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable(pub String);

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unavailable {}

/// The window as the handler sees it.
pub struct Control<'a> {
    window: &'a Window,
    webview: &'a WebView,
    flags: &'a mut Flags,
    waker: &'a Waker,
}

#[derive(Default)]
struct Flags {
    ready: bool,
    exit: bool,
    close_at: Option<Instant>,
    /// The last `hoppy.render(…)` script (re-sent once the page is ready).
    view_script: Option<String>,
    init_script: String,
}

impl Control<'_> {
    /// Shows `view` (kept until the page is ready if it is not yet).
    pub fn render(&mut self, view: &View) {
        let script = script_call("render", view);
        if self.flags.ready
            && let Err(e) = self.webview.evaluate_script(&script)
        {
            diag::log(format!("send window: render failed: {e}"));
        }
        self.flags.view_script = Some(script);
    }

    /// Switches theme/language at once (the smoke test; the tray's choice applies to the
    /// next window).
    pub fn reinit(&mut self, init: &Init) {
        self.flags.init_script = script_call("init", init);
        if self.flags.ready {
            let _ = self.webview.evaluate_script(&self.flags.init_script);
        }
    }

    pub fn set_on_top(&self, on: bool) {
        self.window.set_always_on_top(on);
    }

    /// Closes the window after `delay` (unless the user closes it first).
    pub fn close_after(&mut self, delay: Duration) {
        self.flags.close_at = Some(Instant::now() + delay);
    }

    /// Closes the window: [`run`] returns.
    pub fn exit(&mut self) {
        self.flags.exit = true;
    }

    pub fn waker(&self) -> Waker {
        self.waker.clone()
    }
}

/// Opens the send window and runs it until it closes. `handler` gets every [`Input`].
pub fn run(
    title: &str,
    theme: Theme,
    init: &Init,
    mut handler: impl FnMut(&mut Control<'_>, Input),
) -> Result<(), Unavailable> {
    precheck()?;
    // tao panics when the toolkit cannot start (e.g. GTK without a display).
    let mut event_loop = catch_unwind(AssertUnwindSafe(|| {
        EventLoopBuilder::<UserEvent>::with_user_event().build()
    }))
    .map_err(|_| Unavailable("the window toolkit did not start".into()))?;
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        // Menu-bar app: no Dock icon for the send window either.
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let waker = Waker(event_loop.create_proxy());
    let window = build_window(&event_loop, title, theme)?;
    let mut context = web_context();
    let webview = build_webview(&window, &mut context, theme, waker.clone())?;
    diag::log(format!(
        "send window: webview created, theme {}",
        theme.id()
    ));

    let mut flags = Flags {
        init_script: script_call("init", init),
        ..Flags::default()
    };
    let started = Instant::now();
    handler(
        &mut Control {
            window: &window,
            webview: &webview,
            flags: &mut flags,
            waker: &waker,
        },
        Input::Started,
    );
    let mut gave_up = false;
    event_loop.run_return(|event, _target, control_flow| {
        let mut ctl = Control {
            window: &window,
            webview: &webview,
            flags: &mut flags,
            waker: &waker,
        };
        match event {
            Event::UserEvent(UserEvent::Wake) => handler(&mut ctl, Input::Wake),
            Event::UserEvent(UserEvent::Ipc(raw)) => match parse_action(&raw) {
                Some(Action::Ready) if !ctl.flags.ready => {
                    ctl.flags.ready = true;
                    let mut script = ctl.flags.init_script.clone();
                    script.push_str(ctl.flags.view_script.as_deref().unwrap_or_default());
                    if let Err(e) = ctl.webview.evaluate_script(&script) {
                        diag::log(format!("send window: init failed: {e}"));
                    }
                    handler(&mut ctl, Input::Ready);
                    show(&window);
                    diag::log(format!(
                        "send window: shown after {} ms",
                        started.elapsed().as_millis()
                    ));
                }
                Some(Action::Ready) => {}
                Some(action) if ctl.flags.ready => handler(&mut ctl, Input::Action(action)),
                Some(_) => {}
                None => diag::log(format!(
                    "send window: ignored an invalid message ({} bytes)",
                    raw.len()
                )),
            },
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => handler(&mut ctl, Input::Action(Action::Cancel)),
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                if ctl.flags.close_at.is_some_and(|t| Instant::now() >= t) {
                    ctl.flags.exit = true;
                }
                if !ctl.flags.ready && started.elapsed() >= READY_TIMEOUT {
                    gave_up = true;
                    ctl.flags.exit = true;
                }
            }
            _ => {}
        }
        *control_flow = if flags.exit {
            ControlFlow::Exit
        } else {
            let mut next = flags.close_at;
            if !flags.ready {
                let deadline = started + READY_TIMEOUT;
                next = Some(next.map_or(deadline, |t| t.min(deadline)));
            }
            match next {
                Some(t) => ControlFlow::WaitUntil(t),
                None => ControlFlow::Wait,
            }
        };
    });
    window.set_visible(false);
    drop(webview);
    drop(window);
    if gave_up {
        return Err(Unavailable("the page did not load".into()));
    }
    Ok(())
}

/// Cheap checks before touching the toolkit.
fn precheck() -> Result<(), Unavailable> {
    if cfg!(target_os = "linux")
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
    {
        return Err(Unavailable("no display".into()));
    }
    if cfg!(windows) {
        // Preinstalled on Windows 11 and most Windows 10; missing on some LTSC/Server.
        match wry::webview_version() {
            Ok(v) => diag::log(format!("send window: WebView2 {v}")),
            Err(e) => return Err(Unavailable(format!("no WebView2 runtime ({e})"))),
        }
    }
    Ok(())
}

fn build_window(
    event_loop: &EventLoop<UserEvent>,
    title: &str,
    theme: Theme,
) -> Result<Window, Unavailable> {
    let (r, g, b) = theme.background_rgb();
    let builder = WindowBuilder::new()
        .with_title(title)
        .with_inner_size(LogicalSize::new(WIDTH, HEIGHT))
        .with_min_inner_size(LogicalSize::new(WIDTH, HEIGHT * 0.8))
        .with_resizable(false)
        .with_visible(false)
        .with_always_on_top(true)
        .with_background_color((r, g, b, 255))
        .with_theme(Some(if theme.is_dark() {
            tao::window::Theme::Dark
        } else {
            tao::window::Theme::Light
        }))
        .with_window_icon(window_icon());
    let window = builder
        .build(event_loop)
        .map_err(|e| Unavailable(format!("window: {e}")))?;
    place_near_pointer(event_loop, &window);
    Ok(window)
}

/// Centres the window on the screen that has the mouse pointer (where the user just
/// right-clicked the files). Wayland does not let apps place windows: ignored there.
fn place_near_pointer(event_loop: &EventLoop<UserEvent>, window: &Window) {
    let Ok(pointer) = event_loop.cursor_position() else {
        return;
    };
    let monitor = event_loop
        .available_monitors()
        .find(|m| {
            let (p, s) = (m.position(), m.size());
            pointer.x >= f64::from(p.x)
                && pointer.y >= f64::from(p.y)
                && pointer.x < f64::from(p.x) + f64::from(s.width)
                && pointer.y < f64::from(p.y) + f64::from(s.height)
        })
        .or_else(|| event_loop.primary_monitor());
    let Some(monitor) = monitor else {
        return;
    };
    // A hidden GTK window reports a 0×0 size: use ours (plus the title bar, roughly).
    let reported = window.outer_size();
    let ours: tao::dpi::PhysicalSize<u32> =
        LogicalSize::new(WIDTH, HEIGHT + 32.0).to_physical(monitor.scale_factor());
    let w = if reported.width < ours.width / 2 {
        ours
    } else {
        reported
    };
    let (p, s) = (monitor.position(), monitor.size());
    let x = i64::from(p.x) + (i64::from(s.width) - i64::from(w.width)) / 2;
    let y = i64::from(p.y) + (i64::from(s.height) - i64::from(w.height)) / 2;
    if let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) {
        window.set_outer_position(PhysicalPosition::new(x, y));
    }
}

#[cfg(not(target_os = "macos"))]
fn window_icon() -> Option<Icon> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(ICON_PNG));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba {
        return None;
    }
    buf.truncate(info.buffer_size());
    Icon::from_rgba(buf, info.width, info.height).ok()
}

/// macOS windows show the app's icon (Hoppy.icns), not their own.
#[cfg(target_os = "macos")]
fn window_icon() -> Option<Icon> {
    None
}

/// WebView2 keeps its profile next to the exe by default (in the install folder): use
/// Hoppy's local AppData folder instead. Elsewhere nothing is stored (incognito).
fn web_context() -> WebContext {
    let dir = cfg!(windows)
        .then(|| crate::config::state_dir().ok().map(|d| d.join("WebView2")))
        .flatten();
    WebContext::new(dir)
}

fn build_webview(
    window: &Window,
    context: &mut WebContext,
    theme: Theme,
    waker: Waker,
) -> Result<WebView, Unavailable> {
    let (r, g, b) = theme.background_rgb();
    let builder = WebViewBuilder::new_with_web_context(context)
        .with_custom_protocol(web::SCHEME.into(), |_id, request| serve(&request))
        .with_url(web::start_url())
        .with_background_color((r, g, b, 255))
        .with_incognito(true)
        .with_devtools(false)
        .with_hotkeys_zoom(false)
        .with_clipboard(false)
        .with_autoplay(false)
        .with_back_forward_navigation_gestures(false)
        .with_navigation_handler(|url| {
            let own = web::is_own_url(&url);
            if !own {
                diag::log("send window: blocked a navigation away from the page");
            }
            own
        })
        .with_new_window_req_handler(|_url, _features| NewWindowResponse::Deny)
        .with_ipc_handler(move |request: Request<String>| {
            // Only our page talks to us (no other origin can be loaded anyway).
            if web::is_own_url(&request.uri().to_string()) {
                let _ = waker.0.send_event(UserEvent::Ipc(request.into_body()));
            }
        });
    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window
            .default_vbox()
            .ok_or_else(|| Unavailable("no GTK container".into()))?;
        builder.build_gtk(vbox)
    };
    #[cfg(not(target_os = "linux"))]
    let webview = builder.build(window);
    webview.map_err(|e| Unavailable(format!("webview: {e}")))
}

/// The custom protocol: GET of an embedded file, with the CSP; anything else 404/405.
fn serve(request: &Request<Vec<u8>>) -> Response<std::borrow::Cow<'static, [u8]>> {
    let empty = |status| {
        Response::builder()
            .status(status)
            .body(std::borrow::Cow::Borrowed(&[][..]))
            .unwrap_or_default()
    };
    if request.method() != wry::http::Method::GET {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    }
    let Some(asset) = web::asset(request.uri().path()) else {
        return empty(StatusCode::NOT_FOUND);
    };
    Response::builder()
        .header(header::CONTENT_TYPE, asset.mime)
        .header(header::CONTENT_SECURITY_POLICY, web::CSP)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "no-store")
        .body(asset.body)
        .unwrap_or_else(|_| empty(StatusCode::INTERNAL_SERVER_ERROR))
}

/// Shows the window and brings it to the front (Explorer/Finder started us in the
/// background; macOS also needs the app activated, `ui::activate_app`).
fn show(window: &Window) {
    window.set_visible(true);
    window.set_focus();
    crate::platform::ui::activate_app();
}
