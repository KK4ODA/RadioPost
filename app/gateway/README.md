# RadioPost Gateway

The gateway as a desktop application, for the computer at a gateway's radio: it sets the gateway
up, registers it with its operator's post office, answers calls through Aether HF, and shows what
it does. The operator's guide is [`docs/user/gateway.md`](../../docs/user/gateway.md).

| Where | What |
|---|---|
| `core/gateway` (`radiopost-gateway`) | The gateway itself. `station` answers calls and carries each session to the caller's post office, or answers it from what it holds when that cannot be reached (`offline`); `upkeep` polls for copies placed ahead, refreshes the post offices' keys (`keys`) and reports what was carried offline, beside it; `store` is what it holds (`store.json`); `monitor` keeps what it is doing (its state, the session under way, the sessions carried in `sessions.json`, its log, the post offices' answers); `runner` runs it on a thread of its own, started and stopped, with the post offices checked every ten minutes beside it; `panel` serves the page and its JSON API. |
| `app/gateway/ui` | The page: no build step, built into the program, light or dark as the system is. |
| `app/gateway/src-tauri` (`radiopost-gateway-app`) | The window: the panel's server and the gateway in this process, the window on the panel. Its own package and lockfile, outside the core workspace, as the client's is. |

The gateway's directory is `$RADIOPOST_GATEWAY`, else `.radiopost-gateway` in the user's home
directory; the app and `radiopost-gateway panel` share it, and `radiopost-gateway` takes `--dir`.

## The panel

`radiopost-gateway panel [--port N] [--ui DIR]` serves the same page (port 8621) for a browser on the
same computer, and answers calls when the gateway is set to answer at start; `--ui DIR` serves the
page from a directory, for working on it. The app's window shows it on 8621, or another free port.

Its API, under `/api/`: `GET state` (the settings, the gateway's key and credential, whether it
runs, its status, the client home it registers through, and in `held` the copies and the queue in
numbers), `GET log?since=N`, `GET sessions`, `GET held` (each copy held, never its text; each
session waiting to be reported; the last reports; the post offices polled and the keys kept), and,
with `X-RadioPost: 1`, `POST setup`, `post-office`, `register`, `credential`, `start`, `stop` and
`check`. The settings change only while the gateway is stopped. As the client's panel does, it
answers only on the loopback interface and only to its own host name.

While it answers, the gateway waits for its modem when it cannot reach it (trying every five
seconds) and opens it again when it goes away; **Stop** ends a session under way with a disconnect
and lets the modem go, claim and all.

## Build and test

```
cd app/gateway/src-tauri
cargo test --locked
cargo run                                     # the window
npx @tauri-apps/cli@2 build --bundles deb     # an installer
```

On Linux it needs `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libxdo-dev
libayatana-appindicator3-dev`. The gateway's own tests are in the core workspace:
`cargo test -p radiopost-gateway` there runs `tests/panel.rs` (the operator's path through the API:
set up, registered with a real post office, started, a user's call carried and listed, checked,
stopped; a modem away waited for), beside the online and conformance tests. The icons come from
`tools/figures/icons.py`.
