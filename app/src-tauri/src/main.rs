// SPDX-License-Identifier: MIT OR Apache-2.0
//! RadioPost, the desktop application (design D27): the client's panel (`app/ui`) in a window of
//! its own.
//!
//! The panel's server runs in this process, on the loopback interface, as `radiopost panel` serves
//! it, and the window shows it. The page talks only to that server: the window grants it no access
//! to the application (no Tauri commands, no capabilities). Closing the window ends the program;
//! while a call, a session or listening is under way, it asks first, since closing ends that too.
//! A call or a session it ends is aborted first, as the session card's **Abort** does, and the
//! window closes once it has ended (`close_after_abort`): the other station hears the link end, and
//! a message received in part is kept to resume. While it runs, the panel holds a connection to
//! Aether that names the app there between calls, and closes it as the program ends.
//!
//! It updates itself: the panel says when a newer release is out (asked when it opens and every
//! six hours), and its Update and restart installs it with Tauri's updater, from the update
//! channel's `client.json`, only when the project's update key signed it (`radiopost-update`).
//!
//! The window opens where the operator left it, at the size they left it (`WINDOW_KEPT`), so
//! RadioPost arranged once beside Aether and the gateway's window opens that way again.
//!
//! When the window is narrow, the panel stacks its parts in one column, and Write opens the
//! composer in a window of its own: the page asks for it with `window.open` on `/?compose=…`, and
//! this program makes it (`composer_window`), the one window the page may open. It shows the same
//! page from the same server; its page going to `/closed` closes it, and to `/front` brings it
//! forward (`navigation`), the page never going there. Composer windows open where the operator
//! left the last one. Closing the main window closes them too.
//!
//! **Print** in the reader prints the window: its page goes to `/print`, which this program stops
//! and takes as the word to print (`Navigation::Print`, Tauri's `print`): macOS's webview does
//! nothing at the page's own `window.print()`. The page knows to by `radiopostPrints` ([`PRINTS`]).
//!
//! When an alert starts sounding (RP-8), the window comes forward, out of the taskbar if it was
//! minimized, and asks for attention: the operator may be in another program (`bring_forward`).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use radiopost_client::panel::Panel;
use radiopost_client::selection::hf::Hf;
use radiopost_client::session::Stop;
use radiopost_client::store;
use radiopost_update::{Install, Product, Watch};
use tauri::webview::{NewWindowFeatures, NewWindowResponse};
use tauri::{
    AppHandle, Manager, RunEvent, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt as _;
use tauri_plugin_window_state::{StateFlags, WindowExt as _};

/// The panel's port, the same as `radiopost panel`'s, so what the page keeps (the modem adapter's
/// address) is kept from one run to the next; another free port when that one is taken.
const PORT: u16 = 8620;

/// What the window keeps from one run to the next: where it was, its size, and whether it was
/// maximized, in the application's configuration folder (`.window-state.json`). An operator who
/// stacks the gateway, Aether and the client on one screen does it once (KK4ODA, 2026-11-14).
/// A place on a screen no longer connected is not used: the system places the window instead.
const WINDOW_KEPT: StateFlags = StateFlags::SIZE
    .union(StateFlags::POSITION)
    .union(StateFlags::MAXIMIZED);

/// How often the window looks for an alert that has started sounding.
const ALARM_LOOK: std::time::Duration = std::time::Duration::from_millis(500);

/// The composer windows' name: each is `compose-<n>`, and the window-state plugin keeps them all
/// under this one, so a composer window opens where the last one was left, at its size.
const COMPOSER: &str = "compose";

/// The composer windows opened so far, to name the next.
static COMPOSERS: AtomicU32 = AtomicU32::new(0);

/// Where a composer window's page goes when it is done: the window closes instead, the page never
/// going there. A page's own `window.close()` closes the window on Windows only (on Linux the
/// webview drops the page and leaves the window, on macOS it does nothing).
const CLOSED: &str = "/closed";

/// Where the main window sends a composer window's page to bring it forward, when Write is
/// pressed again while it is open: the window comes forward instead, its page as it was. A page's
/// `focus()` of another window does nothing in the app's webviews.
const FRONT: &str = "/front";

/// Where a window's page goes to be printed (the reader's **Print**): the window prints instead,
/// with the system's print dialog, where Save as PDF makes a PDF; the page never goes there.
/// Tauri's `print` is the one way on every system: macOS's webview does nothing at the page's own
/// `window.print()`.
const PRINT: &str = "/print";

/// What the main window's page is told before it runs: that it prints by [`PRINT`] (`app/ui/app.js`'s
/// `printMessage`).
const PRINTS: &str = "window.radiopostPrints = true;";

/// What a composer window's page is told before it runs: that its window is the app's, so it is
/// closed and brought forward by [`CLOSED`] and [`FRONT`] (`app/ui/app.js`'s `closeWindow` and
/// `bringForward`).
const IN_THE_APP: &str = "window.radiopostApp = true;";

/// Whether a window the panel's page asks for (`window.open`) is the composer in a window of its
/// own: the panel's own page, with `compose` in its query.
fn opens_composer(panel: &Url, url: &Url) -> bool {
    url.origin() == panel.origin()
        && url.path() == "/"
        && url.query_pairs().any(|(name, _)| name == COMPOSER)
}

/// What a composer window does with a page it is sent to.
#[derive(Debug, PartialEq, Eq)]
enum Navigation {
    /// Shows it: the panel's own pages, and the blank page a window starts on.
    Go,
    /// Closes the window: the page is done ([`CLOSED`]).
    Close,
    /// Brings the window forward, the page staying ([`FRONT`]).
    Front,
    /// Prints the window, the page staying ([`PRINT`]).
    Print,
    /// Stays where it is: anywhere else.
    Refuse,
}

fn navigation(panel: &Url, to: &Url) -> Navigation {
    if to.as_str() == "about:blank" {
        Navigation::Go
    } else if to.origin() != panel.origin() {
        Navigation::Refuse
    } else if to.path() == CLOSED {
        Navigation::Close
    } else if to.path() == FRONT {
        Navigation::Front
    } else if to.path() == PRINT {
        Navigation::Print
    } else {
        Navigation::Go
    }
}

/// Does `what` to the window named `label` from another thread: done inside the page's own
/// event, it would re-enter the webview.
fn later(app: &AppHandle, label: &str, what: fn(&tauri::WebviewWindow)) {
    let (app, label) = (app.clone(), label.to_owned());
    std::thread::spawn(move || {
        if let Some(window) = app.get_webview_window(&label) {
            what(&window);
        }
    });
}

/// Where the main window's page may go: anywhere, but [`PRINT`], at which the window prints, the
/// page staying (the reader's **Print**).
fn printing(app: AppHandle, panel: Url) -> impl Fn(&Url) -> bool + Send + 'static {
    move |to| {
        if navigation(&panel, to) == Navigation::Print {
            later(&app, "main", |window| {
                let _ = window.print();
            });
            false
        } else {
            true
        }
    }
}

/// A window the panel's page asks for: the composer in a window of its own, made here, with the
/// size the page asked for and the panel's page in it; any other is refused, as before.
fn composer_window(
    app: &AppHandle,
    panel: &Url,
    url: &Url,
    features: NewWindowFeatures,
) -> NewWindowResponse<tauri::Wry> {
    if !opens_composer(panel, url) {
        return NewWindowResponse::Deny;
    }
    let label = format!(
        "{COMPOSER}-{}",
        COMPOSERS.fetch_add(1, Ordering::Relaxed) + 1
    );
    let (app_, named, home) = (app.clone(), label.clone(), panel.clone());
    let built = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(blank()))
        .window_features(features)
        .title("RadioPost")
        .min_inner_size(380.0, 480.0)
        // shown once it is where the last composer window was (`placed`)
        .visible(false)
        .initialization_script(IN_THE_APP)
        .on_navigation(move |to| match navigation(&home, to) {
            Navigation::Go => true,
            Navigation::Refuse => false,
            Navigation::Close => {
                later(&app_, &named, |window| {
                    let _ = window.close();
                });
                false
            }
            Navigation::Front => {
                later(&app_, &named, |window| {
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_focus();
                });
                false
            }
            Navigation::Print => {
                later(&app_, &named, |window| {
                    let _ = window.print();
                });
                false
            }
        })
        .on_new_window(|_, _| NewWindowResponse::Deny)
        .on_document_title_changed(|window, title| {
            let _ = window.set_title(&title);
        })
        .build();
    match built {
        Ok(window) => {
            placed(window.clone());
            NewWindowResponse::Create { window }
        }
        Err(e) => {
            radiopost_client::logfile::line(&format!("the composer's window cannot open: {e}"));
            NewWindowResponse::Deny
        }
    }
}

/// A new composer window put where the last one was, at its size, then shown and in front.
///
/// From another thread, once the page's `window.open` has its window: the window-state plugin's
/// own restore runs while the window is being made, and on Windows moving it there sends the
/// window's events while Tauri holds its plugins' lock, which their handler takes again, and the
/// program hangs (found running it, 2026-11-18). So the plugin skips composer windows' first
/// restore (`skip_initial_state`) and this does it.
fn placed(window: tauri::WebviewWindow) {
    std::thread::spawn(move || {
        let _ = window.restore_state(WINDOW_KEPT);
        let _ = window.show();
        let _ = window.set_focus();
    });
}

/// The page a new window starts on, before the page that asked for it is put in it.
fn blank() -> Url {
    "about:blank".parse().expect("about:blank is a URL")
}

/// Brings the window forward each time an alert starts sounding (RP-8 §7): out of the taskbar,
/// in front, and asking for attention, which flashes its taskbar button on Windows.
fn bring_forward(panel: Panel, window: tauri::WebviewWindow) {
    std::thread::spawn(move || {
        let mut sounding = false;
        loop {
            let now = panel.alarming();
            if now && !sounding {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
                let _ = window.request_user_attention(Some(tauri::UserAttentionType::Critical));
            }
            sounding = now;
            std::thread::sleep(ALARM_LOOK);
        }
    });
}

/// Starts the panel's server for the home at `home`: where it is, and a handle on its state.
/// `engine`, the VOACAP engine the installer put among the program's resources, for HF
/// predictions (choosing a gateway, §11.2).
fn start_panel(home: &Path, engine: Option<&Path>) -> Result<(SocketAddr, Panel), String> {
    let updates = Watch::new(
        Product::Client,
        env!("CARGO_PKG_VERSION"),
        &radiopost_update::channel(),
    );
    let panel = Panel::new(home, None)
        .with_updates(updates)
        .with_hf(Hf::bundled(engine));
    let at = panel
        .clone()
        .start(&format!("127.0.0.1:{PORT}"))
        .or_else(|_| panel.clone().start("127.0.0.1:0"))?;
    Ok((at, panel))
}

/// What closing the window would end, in the operator's words; `None` when nothing is under way.
fn interrupts(activity: &str) -> Option<&'static str> {
    match activity {
        "calling" => Some("A call is going out: closing stops it."),
        "listening" => Some("The station is listening for a call."),
        "session" => Some(
            "A session is under way: closing aborts it. What did not cross is offered again on \
             your next call.",
        ),
        "alerting" => Some("An alert is going out."),
        _ => None,
    }
}

/// How long closing waits for the call or session it aborted to end. The adapter answers an
/// abort at once, and a session's thread gives up on one it does not answer in ten seconds.
const CLOSE_PATIENCE: Duration = Duration::from_secs(12);

/// Closes the window, the operator having said so: a call or a session under way is aborted
/// first, as the session card's **Abort** does, and the window closes once it has ended (or after
/// [`CLOSE_PATIENCE`]), so the other station hears the link end and what was received in part is
/// on disk to resume (RP-3 §7). The program ending would otherwise stop the session's thread
/// where it stood.
fn close_after_abort(panel: &Panel, window: WebviewWindow) {
    if panel.hang_up(Stop::Abort).is_err() {
        // nothing on the air to end first
        let _ = window.destroy();
        return;
    }
    let panel = panel.clone();
    std::thread::spawn(move || {
        let until = Instant::now() + CLOSE_PATIENCE;
        while matches!(panel.activity(), "calling" | "session") && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = window.destroy();
    });
}

/// The update on offer, downloaded, its signature checked with the project's key, installed, and
/// the application started again; it returns only when that failed.
fn installer(app: AppHandle) -> Install {
    Arc::new(move || {
        let manifest = format!(
            "{}/{}",
            radiopost_update::channel(),
            Product::Client.manifest()
        );
        tauri::async_runtime::block_on(async {
            let url = manifest.parse().map_err(|e| format!("{manifest}: {e}"))?;
            let update = app
                .updater_builder()
                .endpoints(vec![url])
                .and_then(tauri_plugin_updater::UpdaterBuilder::build)
                .map_err(|e| e.to_string())?
                .check()
                .await
                .map_err(|e| format!("the update channel: {e}"))?
                .ok_or("no update is on offer")?;
            update
                .download_and_install(|_, _| {}, || {})
                .await
                .map_err(|e| format!("the update: {e}"))?;
            app.restart()
        })
    })
}

fn main() {
    let home = store::home(None);
    // what the panel says, and any panic, in the home's log file: the window's log is gone
    // once it closes (2026-11-12)
    radiopost_client::logfile::keep_in(&home.join(radiopost_client::LOG_FILE));
    radiopost_client::logfile::record_panics();
    radiopost_client::logfile::line(&format!("RadioPost {} starts", env!("CARGO_PKG_VERSION")));
    // the context first: it says where the installer put the program's resources, the engine
    // among them
    let context = tauri::generate_context!();
    let engine =
        tauri::utils::platform::resource_dir(context.package_info(), &tauri::Env::default())
            .ok()
            .map(|dir| dir.join("engine"));
    let (at, panel) = match start_panel(&home, engine.as_deref()) {
        Ok(started) => started,
        Err(e) => {
            eprintln!("RadioPost: the panel cannot start: {e}");
            std::process::exit(1);
        }
    };
    let url = format!("http://{at}/");
    let updates = Arc::clone(panel.updates());
    let leaving = panel.clone();
    let watching = panel.clone();
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_updater::Builder::new()
                .pubkey(radiopost_update::public_key())
                .build(),
        )
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(WINDOW_KEPT)
                .map_label(|label| {
                    if label.starts_with(COMPOSER) {
                        COMPOSER
                    } else {
                        label
                    }
                })
                // `placed` puts each composer window where the last was
                .skip_initial_state(COMPOSER)
                .build(),
        )
        .setup(move |app| {
            // a build made before the project's update key existed installs nothing
            if !radiopost_update::public_key().is_empty() {
                updates.set_install(installer(app.handle().clone()));
            }
            updates.watch();
            let page: Url = url.parse()?;
            let opening = app.handle().clone();
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(page.clone()))
                .title("RadioPost")
                .inner_size(1100.0, 760.0)
                .min_inner_size(380.0, 480.0)
                .initialization_script(PRINTS)
                .on_navigation(printing(app.handle().clone(), page.clone()))
                .on_new_window(move |to, features| composer_window(&opening, &page, &to, features))
                .build()?;
            bring_forward(watching, window.clone());
            let asking = window.clone();
            window.on_window_event(move |event| {
                // the program ends with its main window, composer windows and all
                if let WindowEvent::Destroyed = event {
                    asking.app_handle().exit(0);
                }
                if let WindowEvent::CloseRequested { api, .. } = event
                    && let Some(what) = interrupts(panel.activity())
                {
                    api.prevent_close();
                    let (window, closing) = (asking.clone(), panel.clone());
                    asking
                        .app_handle()
                        .dialog()
                        .message(format!("{what} Close RadioPost anyway?"))
                        .title("RadioPost")
                        .kind(MessageDialogKind::Warning)
                        .buttons(MessageDialogButtons::OkCancelCustom(
                            "Close".into(),
                            "Keep it open".into(),
                        ))
                        .show(move |close| {
                            if close {
                                close_after_abort(&closing, window);
                            }
                        });
                }
            });
            Ok(())
        })
        .build(context);
    match result {
        // the program ends: its connection to Aether between calls closes first, so Aether says
        // the app has gone
        Ok(app) => app.run(move |_, event| {
            if let RunEvent::Exit = event {
                leaving.leave();
            }
        }),
        Err(e) => {
            eprintln!("RadioPost: the window cannot open: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    use tauri::Url;

    use super::{Navigation, interrupts, navigation, opens_composer, start_panel};

    fn url(text: &str) -> Url {
        text.parse().expect("a URL")
    }

    /// The page may open the composer in a window of its own, and nothing else: no other page,
    /// no other origin (the postcard's blank window stays refused, as before).
    #[test]
    fn the_one_window_the_page_may_open() {
        let panel = url("http://127.0.0.1:8620/");
        for yes in [
            "http://127.0.0.1:8620/?compose=new",
            "http://127.0.0.1:8620/?compose=reply&id=b3f1c9a2",
            "http://127.0.0.1:8620/?compose=new&to=ana%40radiopost.org%2C+",
        ] {
            assert!(opens_composer(&panel, &url(yes)), "{yes}");
        }
        for no in [
            "about:blank",
            "http://127.0.0.1:8620/",
            "http://127.0.0.1:8620/?composed=1",
            "http://127.0.0.1:8620/style.css?compose=new",
            "http://127.0.0.1:8621/?compose=new",
            "http://localhost:8620/?compose=new",
            "https://example.com/?compose=new",
        ] {
            assert!(!opens_composer(&panel, &url(no)), "{no}");
        }
    }

    /// A window shows the panel's pages, closes at `/closed`, comes forward at `/front`, prints at
    /// `/print`, and goes nowhere else.
    #[test]
    fn where_a_window_goes() {
        let panel = url("http://127.0.0.1:8620/");
        assert_eq!(navigation(&panel, &url("about:blank")), Navigation::Go);
        assert_eq!(
            navigation(
                &panel,
                &url("http://127.0.0.1:8620/?compose=forward&id=4c2d")
            ),
            Navigation::Go
        );
        assert_eq!(
            navigation(&panel, &url("http://127.0.0.1:8620/closed")),
            Navigation::Close
        );
        assert_eq!(
            navigation(&panel, &url("http://127.0.0.1:8620/front")),
            Navigation::Front
        );
        // the reader's Print, in either window: the window prints
        assert_eq!(
            navigation(&panel, &url("http://127.0.0.1:8620/print")),
            Navigation::Print
        );
        for elsewhere in [
            "https://example.com/",
            "http://127.0.0.1:8621/closed",
            "https://example.com/front",
            "https://example.com/print",
            "file:///C:/Windows/",
        ] {
            assert_eq!(
                navigation(&panel, &url(elsewhere)),
                Navigation::Refuse,
                "{elsewhere}"
            );
        }
    }

    /// The window's page is the panel's, served for the home given.
    #[test]
    fn the_panel_the_window_shows() {
        let home = std::env::temp_dir().join(format!("radiopost-app-{}", std::process::id()));
        let (at, panel) = start_panel(&home, None).expect("the panel starts");
        assert_eq!(panel.activity(), "idle");
        assert!(at.ip().is_loopback());
        let mut s = TcpStream::connect(at).expect("the panel answers");
        write!(
            s,
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            at.port()
        )
        .expect("sent");
        let mut page = String::new();
        s.read_to_string(&mut page).expect("read");
        assert!(page.contains("<title>RadioPost</title>"), "{page}");
        // a second panel, as a second copy of the program would start, takes another port
        let (other, _) = start_panel(&home, None).expect("another port");
        assert_ne!(other, at);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Closing asks first while anything is under way on the air, and only then.
    #[test]
    fn closing_asks_while_the_station_is_busy() {
        assert_eq!(interrupts("idle"), None);
        for busy in ["calling", "listening", "session"] {
            assert!(interrupts(busy).is_some(), "{busy}");
        }
    }
}
