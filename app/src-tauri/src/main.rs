// SPDX-License-Identifier: MIT OR Apache-2.0
//! RadioPost, the desktop application (design D27): the client's panel (`app/ui`) in a window of
//! its own.
//!
//! The panel's server runs in this process, on the loopback interface, as `radiopost panel` serves
//! it, and the window shows it. The page talks only to that server: the window grants it no access
//! to the application (no Tauri commands, no capabilities). Closing the window ends the program;
//! while a call, a session or listening is under way, it asks first, since closing ends that too and
//! leaves the other station waiting on the air. While it runs, the panel holds a connection to
//! Aether that names the app there between calls, and closes it as the program ends.
//!
//! It updates itself: the panel says when a newer release is out (asked when it opens and every
//! six hours), and its Update and restart installs it with Tauri's updater, from the update
//! channel's `client.json`, only when the project's update key signed it (`radiopost-update`).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use radiopost_client::panel::Panel;
use radiopost_client::store;
use radiopost_update::{Install, Product, Watch};
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt as _;

/// The panel's port, the same as `radiopost panel`'s, so what the page keeps (the modem adapter's
/// address) is kept from one run to the next; another free port when that one is taken.
const PORT: u16 = 8620;

/// Starts the panel's server for the home at `home`: where it is, and a handle on its state.
fn start_panel(home: &Path) -> Result<(SocketAddr, Panel), String> {
    let updates = Watch::new(
        Product::Client,
        env!("CARGO_PKG_VERSION"),
        &radiopost_update::channel(),
    );
    let panel = Panel::new(home, None).with_updates(updates);
    let at = panel
        .clone()
        .start(&format!("127.0.0.1:{PORT}"))
        .or_else(|_| panel.clone().start("127.0.0.1:0"))?;
    Ok((at, panel))
}

/// What closing the window would end, in the operator's words; `None` when nothing is under way.
fn interrupts(activity: &str) -> Option<&'static str> {
    match activity {
        "calling" => Some("A call is going out."),
        "listening" => Some("The station is listening for a call."),
        "session" => Some("A session is under way: closing ends it, and the other station waits."),
        _ => None,
    }
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
    let (at, panel) = match start_panel(&home) {
        Ok(started) => started,
        Err(e) => {
            eprintln!("RadioPost: the panel cannot start: {e}");
            std::process::exit(1);
        }
    };
    let url = format!("http://{at}/");
    let updates = Arc::clone(panel.updates());
    let leaving = panel.clone();
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_updater::Builder::new()
                .pubkey(radiopost_update::public_key())
                .build(),
        )
        .setup(move |app| {
            // a build made before the project's update key existed installs nothing
            if !radiopost_update::public_key().is_empty() {
                updates.set_install(installer(app.handle().clone()));
            }
            updates.watch();
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse()?))
                .title("RadioPost")
                .inner_size(1100.0, 760.0)
                .min_inner_size(380.0, 480.0)
                .build()?;
            let asking = window.clone();
            window.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { api, .. } = event
                    && let Some(what) = interrupts(panel.activity())
                {
                    api.prevent_close();
                    let window = asking.clone();
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
                                let _ = window.destroy();
                            }
                        });
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!());
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

    use super::{interrupts, start_panel};

    /// The window's page is the panel's, served for the home given.
    #[test]
    fn the_panel_the_window_shows() {
        let home = std::env::temp_dir().join(format!("radiopost-app-{}", std::process::id()));
        let (at, panel) = start_panel(&home).expect("the panel starts");
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
        let (other, _) = start_panel(&home).expect("another port");
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
