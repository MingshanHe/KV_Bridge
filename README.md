# kvm-bridge

Use the **Windows laptop's keyboard and Logitech Lift mouse to control a Mac**.
Press the Lift's **Forward** thumb button to send all typing and mouse movement to the
Mac; press it again to come back to Windows. The monitor stays connected to Windows;
you watch the Mac on its own screen.

```
 ┌─────────── Windows laptop ───────────┐            ┌──────── Mac ────────┐
 │ keyboard + Lift ─▶ kvm-win           │   TCP      │ kvm-mac ─▶ CGEvent  │
 │   low-level hooks, Lift toggle  ─────┼──────────▶ │  types, clicks,     │
 │   (blocks input while on the Mac)    │  port 24900│  scrolls            │
 └──────────────────────────────────────┘            └─────────────────────┘
```

No external dependencies: both apps call the OS APIs directly (Win32 hooks on Windows,
CoreGraphics on macOS).

## Get the apps

Every push to `main` publishes both apps to the
[**Latest build** release](https://github.com/MingshanHe/KV_Bridge/releases/tag/latest):

- **Windows:** download
  [`kvm-win.exe`](https://github.com/MingshanHe/KV_Bridge/releases/download/latest/kvm-win.exe)
- **Mac:** install with one Terminal command (works on Apple Silicon and Intel):

  ```sh
  curl -fsSL https://raw.githubusercontent.com/MingshanHe/KV_Bridge/main/install-mac.sh | bash
  ```

  This puts the app in `~/kvm-mac` and clears macOS's download block, so there's
  nothing to unzip by hand. Run the same command again to update.

To build yourself, install Rust from <https://rustup.rs> and run
`cargo build --release -p kvm-win` on Windows or `cargo build --release -p kvm-mac` on the Mac.

## First run

Both laptops must be on the same network.

### 1. Mac: allow it to type (one time)

```sh
~/kvm-mac/kvm-mac --test-type
```

macOS will ask for **Accessibility** access for Terminal. Turn it on in
*System Settings → Privacy & Security → Accessibility*, then run the command again
and click into TextEdit. It should type `hello from kvm bridge`. If it does, the Mac
side works.

### 2. Windows: start the sender

Open PowerShell or Command Prompt in the folder with `kvm-win.exe`:

```
kvm-win.exe --log
```

It prints the address to use on the Mac. Allow it through Windows Firewall on
**Private networks** when asked. With `--log`, press the Lift's thumb buttons and some
keys: you should see `Forward thumb button` and key lines. (The log is for checking;
leave `--log` off in normal use.)

### 3. Mac: connect

```sh
~/kvm-mac/kvm-mac 192.168.x.x        # the address kvm-win printed
```

Or double-click **Start kvm-mac** in the `kvm-mac` folder in your home folder. It asks
for the address once and remembers it (`rm ~/.kvm-mac-host` to change it).

### 4. Switch

Press the Lift's **Forward** button. Windows beeps, the console title says
*controlling MAC*, and your keyboard and mouse now drive the Mac. Press it again to
return. **Ctrl+Alt+M** does the same as a keyboard fallback.

## Key mapping on the Mac

| Windows key | Mac key |
|---|---|
| Ctrl | Cmd (so Ctrl+C / Ctrl+V / Ctrl+Tab work as expected) |
| Windows key | Ctrl |
| Alt | Option |
| 変換 / 無変換 | かな / 英数 |
| Back thumb button | Back (button 4) |

Use `--no-swap` on the Mac to keep Ctrl as Ctrl. With a Japanese keyboard add `--jis`:
the 英数 (Caps) key selects 英数 and 半角/全角 toggles 英数/かな.

## Options

```
kvm-win.exe [--port N] [--button forward|back|none] [--no-hotkey]
            [--allow MAC-IP] [--log] [--quiet]

kvm-mac <WINDOWS-IP> [--port N] [--no-swap] [--invert-scroll]
                     [--scroll-speed PX] [--jis] [--verbose]
kvm-mac --test-type
```

## Safety behaviour

- If the connection drops or the Mac sleeps (no heartbeat for 3 s), control returns
  to Windows automatically, and the Mac releases every key and button it was holding.
- Keys held while switching are released on the machine they were pressed on, so
  nothing gets stuck down.
- The toggle button itself never reaches either machine.
- The Mac display is kept awake while it's being controlled.

## Known limits (for now)

- **Not encrypted.** Keystrokes cross the network in plain text. Use it on your own
  home network, and consider `--allow <Mac IP>` on Windows. TLS is on the roadmap.
- Windows doesn't let any app intercept **Ctrl+Alt+Del** or **Win+L**, and input to
  **administrator windows** (e.g. Task Manager, UAC prompts) can't be captured unless
  kvm-win is also run as administrator.
- If Logi Options+ has the Forward button remapped to something else, kvm-win sees
  the remapped action instead. Set the button back to *Forward* in Options+, or run with
  `--button back`.
- Volume and media keys are not forwarded yet.
- Both apps are console programs; a tray icon (Windows) and menu-bar icon (Mac) are on
  the roadmap.

## Project layout

```
core/         shared: wire protocol, key tables, switching logic (unit tested)
win-server/   kvm-win: Win32 hooks, network server, status output
mac-client/   kvm-mac: injection logic (unit tested) + CoreGraphics posting
.github/      CI: tests on Linux, builds on Windows and macOS runners
```

`cargo test --workspace` runs the unit tests anywhere. To type-check the Windows and
Mac code from any OS: `RUSTFLAGS="--cfg kvm_check" cargo check --workspace`.

## Roadmap

1. ~~Keyboard + mouse forwarding, Lift toggle~~ (this version)
2. Tray / menu-bar apps with a clear "who has control" indicator, start at login
3. TLS + pairing code
4. Clipboard sync
5. Media keys, hold-to-peek mode (hold the button to use the Mac, release to return)
