# KV Bridge

Use the **Windows laptop's keyboard and Logitech Lift mouse to control a Mac**.
Press the Lift's **Forward** thumb button to send all typing and mouse movement to the
Mac; press it again to come back to Windows. The monitor stays connected to Windows;
you watch the Mac on its own screen.

```
 ┌──────── Windows: KV Bridge ────────┐   same Wi-Fi,   ┌──────── Mac: KV Bridge ────────┐
 │ keyboard + Lift ─▶ capture         │   encrypted     │ replay keys, mouse, scrolling  │
 │ Lift button switches control ──────┼───────────────▶ │ Windows shortcuts, UK layout   │
 │ tray icon, settings window         │                 │ menu-bar icon, settings window │
 └────────────────────────────────────┘                 └────────────────────────────────┘
```

## Install the KV Bridge app

Download from the [**Latest build** release](https://github.com/MingshanHe/KV_Bridge/releases/tag/latest):

- **Windows:** [`KV-Bridge-Setup.exe`](https://github.com/MingshanHe/KV_Bridge/releases/download/latest/KV-Bridge-Setup.exe).
  If Windows shows "Windows protected your PC", click *More info* → *Run anyway*.
  The first time it runs, allow it on **private networks** when Windows Firewall asks.
- **Mac:** [`KV-Bridge.dmg`](https://github.com/MingshanHe/KV_Bridge/releases/download/latest/KV-Bridge.dmg).
  Open it and drag **KV Bridge** into **Applications**. The first time, right-click
  the app → **Open** (it isn't from the App Store). Turn on KV Bridge under
  *System Settings → Privacy & Security → Accessibility* when asked.

Both start automatically at login and live in the tray (Windows) / menu bar (Mac).
Click the icon → *打开 KV Bridge* for the settings window.

### Pair once

1. On the Mac, click **配对新电脑**. A 6-digit code appears.
2. On Windows, the Mac shows up under **附近的 Mac**. Click **配对** and type the code.
   (If it doesn't show up, use *用 IP 地址配对* with the Mac's IP.)

From then on the two computers find each other and connect by themselves whenever
they're on the same Wi-Fi. Press the Lift's **Forward** button (or Ctrl+Alt+M) to
switch between them.

The connection is encrypted with a key created during pairing. The code itself never
crosses the network, so it can't be guessed from what's sent.

### Settings

- **Windows:** which Lift button switches, the Ctrl+Alt+M fallback, the switch sound.
- **Mac:** Windows-style shortcuts, Windows keyboard layout (auto / UK / US / off),
  scroll speed and direction, Japanese Windows keyboard.
- **Both:** the computer's name and start at login.

## How it's built

| Folder | What |
|---|---|
| `core/` | Wire protocol, key tables, switching logic, keyboard layouts |
| `net/` | LAN discovery (mDNS), pairing (SPAKE2), encrypted sessions (Noise) |
| `win-server/` | Windows keyboard/mouse capture (library) + the old `kvm-win` CLI |
| `mac-client/` | Mac input replay (library) + the old `kvm-mac` CLI |
| `app/` | The KV Bridge app (Tauri): `src-tauri/` Rust, `ui/` settings window |

GitHub Actions runs the tests on Linux and builds the Windows installer and the Mac
dmg on every push; pushes to `main` publish them to the *Latest build* release.

The command-line tools (`kvm-win.exe`, `kvm-mac.dmg`) are still published for testing,
but they don't use pairing or encryption and can't talk to the app.

## Windows shortcuts on the Mac

While you control the Mac, the usual Windows shortcuts do the matching Mac thing:

| On the Windows keyboard | On the Mac |
|---|---|
| Ctrl+C / V / X / Z / A / S / F / W ... | Cmd+C / V / X / Z / A / S / F / W ... |
| **Alt+Tab** (hold Alt, tap Tab) | App switcher (Cmd+Tab); Alt+Shift+Tab goes back |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous tab (browsers, editors) |
| Ctrl+Left / Right | Jump by word |
| Ctrl+Backspace / Delete | Delete a word |
| Home / End | Start / end of line |
| Ctrl+Home / Ctrl+End | Start / end of document |
| Ctrl+click | Cmd+click (add to selection) |
| Alt | Option |
| Windows key | Ctrl |
| 変換 / 無変換 | かな / 英数 |
| Back thumb button | Back |

**In terminal apps** (Terminal, iTerm2, Warp, Alacritty, kitty, WezTerm, Ghostty, Hyper,
Tabby) it works like Windows Terminal:

| On the Windows keyboard | In the Mac terminal |
|---|---|
| Ctrl+C with text selected | Copy |
| Ctrl+C with nothing selected | Stop the running program (real Ctrl+C) |
| Ctrl+V | Paste |
| Ctrl+Shift+C / Ctrl+Shift+V | Copy / paste (always) |
| Ctrl+Shift+T / W / N | New tab / close tab / new window |
| Any other Ctrl key (Ctrl+D, Ctrl+R, Ctrl+L, Ctrl+Z ...) | Real Ctrl, as in Linux shells |

If Ctrl+C ever stops the program when you meant to copy, use **Ctrl+Shift+C**, which
always copies.

### Different keyboard layouts (e.g. UK Windows keyboard, Japanese Mac)

Symbols are typed **as printed on the Windows keyboard**, whatever layout the Mac
uses. kvm-win tells the Mac its keyboard layout (UK and US are supported). For each
symbol key, the Mac then finds the key on its own layout that types the same
character. On a Japanese Mac, UK Shift+- gives `_`, Shift+; gives `:`, and Shift+'
gives `@`. A character the Mac layout has no key for (such as `£` on JIS) is typed
directly. Letters and shortcuts (Ctrl/Alt/Win held) are not changed, so Japanese
romaji input works as usual.

To override the detected layout, change *Windows 键盘布局* in the Mac settings
(UK, US, or no conversion).

Turn off *使用 Windows 快捷键习惯* in the Mac settings to send keys exactly as they are. With a Japanese Windows keyboard, tick the Japanese keyboard option: the 英数 (Caps) key selects 英数 and
半角/全角 toggles 英数/かな.

## Safety behaviour

- If the connection drops or the Mac sleeps (no heartbeat for 3 s), control returns
  to Windows automatically, and the Mac releases every key and button it was holding.
- Keys held while switching are released on the machine they were pressed on, so
  nothing gets stuck down.
- The toggle button itself never reaches either machine.
- The Mac display is kept awake while it's being controlled.
- Only a paired computer can connect, and everything sent is encrypted.

## Known limits

- Windows doesn't let any app intercept **Ctrl+Alt+Del** or **Win+L**, and input to
  **administrator windows** (e.g. Task Manager, UAC prompts) can't be captured.
- If Logi Options+ has the Forward button remapped to something else, KV Bridge sees
  the remapped action instead. Set the button back to *Forward* in Options+, or choose
  the Back button in KV Bridge's settings.
- Volume and media keys are not forwarded yet.
- Without the signing secrets below, the Mac app gets a new signature on every build,
  and macOS asks for the Accessibility permission again after each update.

## Mac signing (keeps the Accessibility permission across updates)

The workflow signs the Mac app with a fixed self-signed certificate when these two
repository secrets exist (*Settings → Secrets and variables → Actions*):

- `MAC_CERT_P12`: the certificate + key as a base64 `.p12`
- `MAC_CERT_PASSWORD`: its password

Without them it falls back to an ad-hoc signature. Keep the `.p12` private: anyone with
it could sign an app that macOS would treat as KV Bridge.

## Developing

`cargo test --workspace --exclude kv-bridge` runs the unit tests on any OS. To
type-check the Windows and Mac code from any OS:
`RUSTFLAGS="--cfg kvm_check" cargo clippy --workspace --exclude kv-bridge`.
The app is built with the Tauri CLI: `cd app && npx @tauri-apps/cli@2 build`.

### Command-line tools (older)

```
kvm-win.exe [--port N] [--button forward|back|none] [--no-hotkey]
            [--allow MAC-IP] [--log] [--quiet]

kvm-mac <WINDOWS-IP> [--port N] [--no-swap] [--invert-scroll]
                     [--scroll-speed PX] [--layout auto|uk|us|off] [--jis] [--verbose]
kvm-mac --test-type
```

## Roadmap

1. ~~Keyboard + mouse forwarding, Lift toggle~~
2. ~~Tray / menu-bar app, start at login, pairing, encryption~~ (this version)
3. Mac switches to the British keyboard layout automatically while controlled
4. Shortcut editor with a full "Windows habits" preset (Win+E, Alt+F4, Win+Shift+S …)
5. Sign in with Google / GitHub to sync settings
6. Clipboard sync, media keys
