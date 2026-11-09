// SPDX-License-Identifier: MIT OR Apache-2.0
//! RadioPost, the desktop application (design D27): the client's panel (`app/ui`) in a window of
//! its own.
//!
//! The panel's server runs in this process, on the loopback interface, as `radiopost panel` serves
//! it, and the window shows it. The page talks only to that server: the window grants it no access
//! to the application (no Tauri commands, no capabilities). Closing the window ends the program,
//! and a session under way with it.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::SocketAddr;
use std::path::Path;

use radiopost_client::panel::Panel;
use radiopost_client::store;
use tauri::{WebviewUrl, WebviewWindowBuilder};

/// The panel's port, the same as `radiopost panel`'s, so what the page keeps (the modem adapter's
/// address) is kept from one run to the next; another free port when that one is taken.
const PORT: u16 = 8620;

/// Starts the panel's server for the home at `home`, and says where it is.
fn start_panel(home: &Path) -> Result<SocketAddr, String> {
    Panel::new(home, None)
        .start(&format!("127.0.0.1:{PORT}"))
        .or_else(|_| Panel::new(home, None).start("127.0.0.1:0"))
}

fn main() {
    let home = store::home(None);
    let at = match start_panel(&home) {
        Ok(at) => at,
        Err(e) => {
            eprintln!("RadioPost: the panel cannot start: {e}");
            std::process::exit(1);
        }
    };
    let url = format!("http://{at}/");
    let result = tauri::Builder::default()
        .setup(move |app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse()?))
                .title("RadioPost")
                .inner_size(1100.0, 760.0)
                .min_inner_size(380.0, 480.0)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("RadioPost: the window cannot open: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    use super::start_panel;

    /// The window's page is the panel's, served for the home given.
    #[test]
    fn the_panel_the_window_shows() {
        let home = std::env::temp_dir().join(format!("radiopost-app-{}", std::process::id()));
        let at = start_panel(&home).expect("the panel starts");
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
        let other = start_panel(&home).expect("another port");
        assert_ne!(other, at);
        let _ = std::fs::remove_dir_all(&home);
    }
}
