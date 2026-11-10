// SPDX-License-Identifier: MIT OR Apache-2.0
//! RadioPost Gateway, the desktop application: the gateway and its panel (`app/gateway/ui`) in a
//! window of its own, for a gateway's operator at the radio's computer.
//!
//! The gateway runs in this process, and its panel's server on the loopback interface, as
//! `radiopost-gateway panel` serves it; the window shows the panel. The page talks only to that
//! server: the window grants it no access to the application (no Tauri commands, no
//! capabilities). The gateway answers as soon as the app opens when it is set to, as an unattended
//! gateway should. Closing the window stops it; while it answers, the window asks first, since
//! callers then get no answer, and a session under way ends.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use radiopost_gateway::monitor::Status;
use radiopost_gateway::panel::Panel;
use radiopost_gateway::runner::Runner;
use radiopost_gateway::station::{self, Settings};
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

/// The panel's port, the same as `radiopost-gateway panel`'s; another free port when that one is
/// taken.
const PORT: u16 = 8621;

/// Starts the gateway's panel for the gateway in `dir`, and the gateway itself when it is set to
/// answer at start: where the panel is, and the gateway's runner.
fn start(dir: &Path) -> Result<(SocketAddr, Arc<Runner>), String> {
    let runner = Arc::new(Runner::new(dir));
    let panel = Panel::new(runner.clone(), None);
    let at = panel
        .clone()
        .start(&format!("127.0.0.1:{PORT}"))
        .or_else(|_| panel.start("127.0.0.1:0"))?;
    if Settings::load(dir).is_ok_and(|s| s.answer_at_start) {
        // not registered yet, or no credential: the panel says so, and Start is the operator's
        let _ = runner.start();
    }
    Ok((at, runner))
}

/// What closing the window would end, in the operator's words; `None` when the gateway is not
/// answering.
fn interrupts(running: bool, status: &Status) -> Option<String> {
    if let Some(s) = &status.session {
        return Some(format!(
            "A session with {} is under way: closing ends it, and the gateway stops answering.",
            s.remote
        ));
    }
    running.then(|| {
        "The gateway is answering calls: closing stops it, and callers get no answer.".to_owned()
    })
}

fn main() {
    let dir = station::default_dir();
    let (at, runner) = match start(&dir) {
        Ok(started) => started,
        Err(e) => {
            eprintln!("RadioPost Gateway: the panel cannot start: {e}");
            std::process::exit(1);
        }
    };
    let url = format!("http://{at}/");
    let asking_runner = runner.clone();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse()?))
                .title("RadioPost Gateway")
                .inner_size(1100.0, 760.0)
                .min_inner_size(420.0, 480.0)
                .build()?;
            let asking = window.clone();
            window.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { api, .. } = event
                    && let Some(what) =
                        interrupts(asking_runner.running(), &asking_runner.monitor().status())
                {
                    api.prevent_close();
                    let window = asking.clone();
                    asking
                        .app_handle()
                        .dialog()
                        .message(format!("{what} Close RadioPost Gateway anyway?"))
                        .title("RadioPost Gateway")
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
    let app = match app {
        Ok(app) => app,
        Err(e) => {
            eprintln!("RadioPost Gateway: the window cannot open: {e}");
            std::process::exit(1);
        }
    };
    app.run(move |_, event| {
        if let RunEvent::Exit = event {
            // the modem is let go, and a session under way ends with a disconnect
            runner.stop();
        }
    });
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    use radiopost_gateway::monitor::{SessionRecord, Status};

    use super::{interrupts, start};

    /// The window's page is the gateway's panel, served for the directory given; a gateway not
    /// set up does not answer.
    #[test]
    fn the_panel_the_window_shows() {
        let dir =
            std::env::temp_dir().join(format!("radiopost-gateway-app-{}", std::process::id()));
        let (at, runner) = start(&dir).expect("the panel starts");
        assert!(at.ip().is_loopback());
        assert!(!runner.running(), "nothing to answer as");
        let mut s = TcpStream::connect(at).expect("the panel answers");
        write!(
            s,
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            at.port()
        )
        .expect("sent");
        let mut page = String::new();
        s.read_to_string(&mut page).expect("read");
        assert!(page.contains("<title>RadioPost Gateway</title>"), "{page}");
        // a second copy of the program takes another port
        let (other, _) = start(&dir).expect("another port");
        assert_ne!(other, at);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Closing asks first while the gateway answers, and says so when a session is under way.
    #[test]
    fn closing_asks_while_the_gateway_answers() {
        let idle = Status {
            state: "stopped",
            since: 0,
            problem: None,
            session: None,
            logged: 0,
            checks: Vec::new(),
        };
        assert_eq!(interrupts(false, &idle), None);
        assert!(interrupts(true, &idle).is_some_and(|w| w.contains("answering")));
        let busy = Status {
            state: "session",
            session: Some(SessionRecord {
                remote: "WD5EMA".into(),
                ..SessionRecord::default()
            }),
            ..idle
        };
        assert!(interrupts(true, &busy).is_some_and(|w| w.contains("WD5EMA")));
    }
}
