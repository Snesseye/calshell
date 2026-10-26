# Calshell

**Google Calendar as a real Windows app. Tray, global hotkey, native notifications, runs on startup. Zero lost features.**

Calshell wraps the full Google Calendar web app in a native shell and adds the things a browser tab can't do: a quick-add popup on a global hotkey, a tray menu with your next events, Windows toast notifications even when the window is closed, and launch-on-login. Everything Google Calendar does, Calshell does, because it *is* Google Calendar underneath.

[![CI](https://github.com/Snesseye/calshell/actions/workflows/ci.yml/badge.svg)](https://github.com/Snesseye/calshell/actions/workflows/ci.yml)

## Download

Grab `Calshell_x.y.z_x64-setup.exe` from the [latest release](https://github.com/Snesseye/calshell/releases/latest) and run it. It installs per user (no admin prompt) and pulls in the WebView2 runtime if your machine lacks it.

The installer is not code-signed yet, so Windows SmartScreen may show "Windows protected your PC". Click **More info → Run anyway**, or build it yourself (below).

The calendar window works right away. For the tray, quick add and notifications you connect a Google OAuth client once; see [First launch](#4-first-launch).

---

## Why

Opening a browser, finding the right tab, and clicking through to add an event is friction you pay dozens of times a week. Browser notifications only fire when the tab is open. There is no system tray presence, no hotkey, no "what's next" at a glance.

The obvious fix, rebuilding a calendar client on top of the Calendar API, doesn't work. The API doesn't cover appointment booking pages, the Tasks sidebar, Gemini, Workspace add-ons, or hundreds of small UI behaviours. You'd spend months reaching 90% parity and still lose features.

So Calshell doesn't rebuild anything. It wraps `calendar.google.com` in a native window for 100% parity, then uses the Calendar API only for the native-only features around it.

## What it is / isn't

| It is | It isn't |
|---|---|
| A native Windows shell around the real Google Calendar web UI | A reimplementation of Google Calendar |
| A quick-capture tool (hotkey → type → Enter) | An offline-first calendar |
| A native notification and tray layer | A multi-account manager (yet) |
| Local-only: your data goes to Google and nowhere else | A hosted service |

## Features

### Shell (the web app, made native)
- Loads `calendar.google.com` in WebView2 with a persistent profile, so you stay signed in
- Single instance: launching again focuses the existing window
- Close-to-tray instead of quit
- Launch on login, minimised to tray
- Remembers window size and position
- External links (Meet, Zoom, Drive, any non-Google URL) open in your default browser, including links wrapped in Google's `google.com/url?q=` redirector
- Handles `window.open`, so Google's "open in new window", print, and add-on sign-in popups work in-app
- `calshell.exe https://calendar.google.com/...` opens that page in the running app

### Native layer (Calendar API)
- **Quick-add popup**: global hotkey (default `Ctrl+Shift+Space`) opens a tiny always-on-top input. Type `call Sam tomorrow 3pm 30min`, press Enter, done. Uses Google's own natural-language parser (`events.quickAdd`), so it behaves exactly like the web "Quick add". When your text has no length or end time, Calshell applies your default length (30 min unless you change it) instead of Google's fixed 1 hour
- **Tray menu**: next 3 events (configurable) with Join meeting / Open in calendar, plus New event, Open calendar, Sync now, Settings, Quit. Tooltip counts down to the next event
- **Native notifications**: a background task fires Windows toasts at each event's popup reminder time, with a **Join** button when the event has a Meet, Zoom, Teams or Webex link. Works with the window closed. Declined events stay quiet
- **Incremental sync**: `events.list` with `syncToken` every 60s, only deltas, negligible quota use
- **All calendars**: reads from `calendarList`, so shared and secondary calendars work. Turn any of them off for the tray and notifications in Settings

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  Tauri 2 app (Rust core + React/TS frontend)                │
│                                                             │
│  ┌──────────────────────┐    ┌───────────────────────────┐  │
│  │ Main window          │    │ Quick-add window           │  │
│  │ WebView2 →           │    │ (borderless, on-top,       │  │
│  │ calendar.google.com  │    │  toggled by global hotkey) │  │
│  └──────────────────────┘    └─────────────┬─────────────┘  │
│                                            │ invoke         │
│  ┌─────────────────────────────────────────▼─────────────┐  │
│  │ Rust core                                             │  │
│  │  • OAuth 2.0 PKCE (loopback redirect)                 │  │
│  │  • Token storage: Windows Credential Manager (keyring)│  │
│  │  • Sync task: events.list + syncToken, 60s interval   │  │
│  │  • Reminder scheduler → Windows toasts                │  │
│  │  • Tray menu builder                                  │  │
│  └─────────────────────────────┬─────────────────────────┘  │
│                                │ HTTPS                      │
└────────────────────────────────┼────────────────────────────┘
                                 ▼
                    Google Calendar API v3
```

Two independent Google sessions exist on purpose:
1. The **webview session** (cookies in the WebView2 profile) powers the full UI.
2. The **API session** (OAuth refresh token in Credential Manager) powers tray, hotkey, and notifications.

They never touch each other. If one breaks, the other keeps working.

The main window and its popups load google.com, so they get no Tauri capabilities, and every Rust command also checks that the caller is one of Calshell's own windows (`quick-add` or `settings`).

## Stack

| Layer | Choice | Why |
|---|---|---|
| App framework | [Tauri 2](https://tauri.app) | Small binary, Rust backend, WebView2 on Windows |
| Frontend | React + TypeScript + Vite | Only used for the quick-add popup and settings; tiny surface |
| Webview | WebView2 (Edge/Chromium) | Google permits sign-in here. Electron is blocked without UA spoofing |
| HTTP | `reqwest` with native TLS (SChannel) | Calendar API calls from Rust, no extra C toolchain on Windows |
| OAuth | Hand-rolled PKCE + loopback (`sha2`, `base64`) | Standard installed-app flow, about 200 lines, easy to audit |
| Secrets | `keyring` crate | Refresh token in Windows Credential Manager, never on disk in plain text |
| Toasts | `tauri-winrt-notification` | Native toasts with Join / Open buttons |

Tauri plugins: `single-instance`, `autostart`, `window-state`, `global-shortcut`, `notification` (non-Windows fallback), `opener` (external URLs), `log`.

## Project structure

```
calshell/
├── src/                      # React frontend (one bundle, view picked by window label)
│   ├── windows/
│   │   ├── QuickAdd.tsx      # hotkey popup
│   │   └── Settings.tsx      # account, hotkey, default calendar, calendars, autostart
│   ├── lib/ipc.ts            # typed invoke() wrappers
│   ├── styles.css
│   └── main.tsx
├── src-tauri/
│   ├── src/
│   │   ├── main.rs           # app setup, plugins, startup
│   │   ├── auth.rs           # OAuth PKCE flow, token refresh, keyring
│   │   ├── gcal.rs           # Calendar API client (calendarList, events.list, quickAdd, patch)
│   │   ├── sync.rs           # background sync loop with syncToken
│   │   ├── reminders.rs      # schedules toasts from synced events
│   │   ├── tray.rs           # tray menu + tooltip
│   │   ├── navigation.rs     # internal vs external URL routing
│   │   ├── windows.rs        # main window, popups, quick-add, settings, hotkey
│   │   ├── commands.rs       # #[tauri::command] handlers exposed to the frontend
│   │   ├── config.rs         # config.json
│   │   ├── quick_add.rs      # "did the user type a length?" heuristic
│   │   ├── format.rs         # time labels
│   │   └── state.rs          # shared app state
│   ├── capabilities/         # Tauri 2 permission manifests per window
│   ├── tauri.conf.json
│   └── Cargo.toml
├── .github/workflows/
│   ├── ci.yml                # lint, clippy, tests on Windows
│   └── release.yml           # builds the Windows installer on tag
├── LICENSE
└── README.md
```

## Getting started

### Prerequisites (to build from source)
- Windows 10/11 with [WebView2 runtime](https://developer.microsoft.com/microsoft-edge/webview2/) (preinstalled on Windows 11)
- [Rust](https://rustup.rs) stable (1.82+)
- Node 20.19+ and pnpm
- [Tauri 2 prerequisites](https://tauri.app/start/prerequisites/) for Windows (MSVC build tools)

### 1. Create a Google Cloud OAuth client
Needed only for the native layer (tray, hotkey, notifications). The shell works without it.

1. Go to [Google Cloud Console](https://console.cloud.google.com) and create a project
2. Enable the **Google Calendar API**
3. Configure the OAuth consent screen:
   - User type: **External**
   - Scopes: `https://www.googleapis.com/auth/calendar.events` and `https://www.googleapis.com/auth/calendar.readonly`
   - **Set publishing status to "In production"**, not "Testing". In Testing, refresh tokens expire after 7 days and you'll re-login weekly. You don't need to complete verification; you'll see an "unverified app" warning once at sign-in, then it works indefinitely for your own account
4. Create credentials → OAuth client ID → **Desktop app**
5. Copy the client ID and client secret

Then either:
- **Paste them into Calshell** (Settings → Google Calendar). Works with the downloaded installer, no build needed. They're saved in `config.json`; or
- **Compile them in**: create `src-tauri/.env` (see `src-tauri/.env.example`):

```
GOOGLE_CLIENT_ID=xxxxxxxx.apps.googleusercontent.com
GOOGLE_CLIENT_SECRET=GOCSPX-xxxxxxxx
```

About the secret: the flow uses PKCE, but Google's token endpoint still requires the client secret for "Desktop app" clients. Google's docs state that for installed apps it is not treated as confidential. Don't commit `.env` anyway.

### 2. Run in development
```bash
pnpm install
pnpm tauri dev
```

### 3. Build the installer
```bash
pnpm tauri build
```
Produces an NSIS installer in `src-tauri/target/release/bundle/nsis/`.

### 4. First launch
1. Sign in to Google in the main window as you normally would
2. Open Settings from the tray → **Connect** → complete the OAuth flow in your browser
3. Set your default calendar for quick add and your hotkey
4. **Launch on startup** is on by default; turn it off in Settings if you prefer

## Configuration

Stored in `%APPDATA%\calshell\config.json` and editable from Settings:

```json
{
  "hotkey": "Ctrl+Shift+Space",
  "default_calendar_id": "primary",
  "sync_interval_seconds": 60,
  "launch_on_startup": true,
  "start_minimized": true,
  "close_to_tray": true,
  "tray_event_count": 3,
  "default_event_duration_minutes": 30,
  "notifications_enabled": true,
  "use_24h_clock": true,
  "disabled_calendar_ids": []
}
```

Optional keys: `user_agent` (override the WebView2 user agent, see gotchas), `google_client_id` and `google_client_secret` (bring your own OAuth client; these win over a compiled-in one).

`start_minimized` applies to launches at login: Calshell starts in the tray. Opening it yourself always shows the window.

Tokens are never in this file. The refresh token lives in Windows Credential Manager (service `calshell`, account `google-refresh-token`).

## Roadmap

Ticked items are implemented. Version 0.1 builds, lints and passes its unit tests on Windows in CI; real-world reports are very welcome.

### Phase 1: Shell
- [x] Main window loading `calendar.google.com` with persistent WebView2 profile
- [ ] Verify Google sign-in works in WebView2 across personal and Workspace (SSO) accounts
- [x] Single instance
- [x] Close-to-tray, tray icon with Open / Quit
- [x] Autostart plugin, start minimised option
- [x] Window state persistence
- [x] External URL routing to default browser
- [x] `window.open` handling
- [x] NSIS installer via `tauri build`

### Phase 2: Quick-add
- [x] OAuth PKCE flow with loopback redirect
- [x] Refresh token in Credential Manager, silent token refresh
- [x] Global hotkey registration (configurable)
- [x] Quick-add window: borderless, always on top, centred, closes on blur/Escape
- [x] `events.quickAdd` against the configured default calendar
- [x] Success/failure feedback (brief inline state, then auto-close)
- [x] Settings window: hotkey, default calendar, autostart toggle

### Phase 3: Notifications and tray
- [x] `calendarList` fetch, per-calendar enable/disable
- [x] Incremental sync with `syncToken`, full resync on `410 Gone`
- [x] Reminder scheduler respecting per-event and per-calendar default reminders
- [x] Windows toasts with Join action for conference links
- [x] Tray menu: next N events, click to open in main window or join
- [x] Tray tooltip: next event and countdown

### Phase 4: Polish
- [ ] SQLite cache so tray/notifications survive offline (today the cache is in memory and refills on launch)
- [ ] Deep-link handler for `calendar.google.com` URLs (a command-line URL already works)
- [ ] Multi-account support
- [ ] Own natural-language parser with configurable defaults (calendar by keyword)
- [ ] Always-visible "next up" mini widget
- [ ] Auto-update via `tauri-plugin-updater`
- [ ] Code-signed installer
- [ ] macOS/Linux support (needs WKWebView/WebKitGTK sign-in validation, see below)

## Known gotchas

- **Google sign-in in embedded webviews**: Google blocks OAuth sign-in in Electron by default. WebView2 on Windows is allowed because it identifies as Edge. If sign-in ever fails, set `user_agent` in `config.json` to a current Chrome or Edge user-agent string and restart. Linux WebKitGTK is where this usually breaks, which is why Linux support is phase 4.
- **Workspace SSO**: while you're on a sign-in page, Calshell lets the window navigate anywhere so third-party identity providers (Okta, Entra ID) work. Once you're on `calendar.google.com`, non-Google links go to your browser.
- **OAuth consent in "Testing" mode** expires refresh tokens after 7 days. Set the app to "In production" as described above. If the token is ever rejected, Calshell shows "Not connected" and you reconnect from Settings.
- **Web notifications inside WebView2** do not become Windows toasts. That's why the native sync + reminder layer exists. Calshell doesn't intercept the webview's Notification API.
- **Toast buttons need Calshell running**: Join/Open act through the running app (it lives in the tray). Toasts in Action Center after a quit just open Calshell.
- **Google Meet inside the webview**: camera and mic permission handling in WebView2 is unreliable. Meet links are routed to the default browser on purpose.
- **`events.quickAdd`** always uses the calendar's time zone and creates 1-hour events when no length is given. Calshell patches the end time to your default length when your text has no duration, range or end ("30min", "3-4pm", "until 5", "all day"). Ambiguous text is left at Google's default.
- **Sync tokens expire**. The API returns `410 Gone`; Calshell drops the token and does a full `events.list` with `timeMin` = now minus 1 day.
- **Rate limits** are not a concern at 1 request/minute, but Calshell respects `Retry-After` on 403/429 anyway.
- **Hotkey taken**: if another app already owns your shortcut, Settings says so and keeps the previous one.

## Privacy

Calshell is local-only. The only network traffic is between your machine and Google. No telemetry, no analytics, no third-party servers.

| What | Where |
|---|---|
| OAuth refresh token | Windows Credential Manager (`calshell` / `google-refresh-token`) |
| Settings | `%APPDATA%\calshell\` |
| WebView2 profile (your Google web session), logs, reminder history | `%LOCALAPPDATA%\calshell\` |

**Disconnect** in Settings revokes the token at Google and deletes it locally. Delete both folders and the credential to fully sign out. The uninstaller offers to remove app data.

## Contributing

Issues and PRs welcome. A few conventions:

- Rust: `cargo fmt` and `cargo clippy --all-targets -- -D warnings` must pass, and `cargo test`
- Frontend: `pnpm lint` must pass
- One feature per PR, with the roadmap checkbox ticked in the same PR
- Anything touching auth or token storage needs a short note in the PR on the threat model

Good first issues are tagged `good-first-issue`.

## License

MIT. See [LICENSE](LICENSE).

Google Calendar is a trademark of Google LLC. Calshell is not affiliated with or endorsed by Google.
