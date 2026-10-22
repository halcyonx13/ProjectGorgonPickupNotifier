# PG Pickup Notifier

## 100% coded by Claude Opus

VIP required for chat log access.

## Info

A small Windows tray app that watches your Project Gorgon chat log and shows a popup in the
corner of your screen with the **name and icon of every item you pick up**.

It reads the `[Status] … added to inventory.` lines the game writes to
`%USERPROFILE%\AppData\LocalLow\Elder Game\Project Gorgon\ChatLogs\Chat-YY-MM-DD.log`,
looks the item up in the game's `items.json` and shows its icon from the Project Gorgon CDN.

## Getting started

1. Unzip anywhere and run `pg-pickup-notifier.exe`. There is no installer.
   Windows SmartScreen may warn about an unrecognised app the first time: click
   **More info → Run anyway**, or build it yourself (see below).
2. A coin icon appears in the notification area (tray). Pick something up in game and a popup
   appears in the bottom-right corner.
3. **Left-click** the tray icon (or run the exe again) to open **Settings**. **Right-click** it for
   *Show test notification*, *Pause notifications*, *Open chat log folder* and *Exit*.

Requirements and tips:

- Windows 10 or 11, 64-bit.
- Chat logging must be switched on in the game (you already have `Chat-*.log` files if it is).
- Play in **windowed** or **borderless/fullscreen-windowed** mode. Like every overlay, the popups
  can't be drawn on top of *exclusive* fullscreen.
- To start it automatically, tick **Start PG Pickup Notifier with Windows** in Settings.

## Settings

Every change in the Settings window is saved and applied immediately, with a live preview.
**Show test notification on screen** plays sample pickups on your real screen.
**Undo changes** goes back to how things were when you opened the window.

| Setting | Default | Notes |
|---|---|---|
| Grouping | Combine | **Combine** adds items as rows to one box. **Stack** gives each item its own box, newest on top; boxes all share the width of the widest one and slide down as older ones leave. |
| Combined box leaves | All at once | *All at once*: each new item resets the timer for the whole box. *One row at a time*: each row has its own timer and the box shrinks. |
| New items appear | On top | Or at the bottom. |
| Max notifications | 5 | Boxes (stack) or rows (combine). The oldest leaves first. |
| Show duration | 1200 ms | Time fully shown, after the entry animation. |
| Show quantities | Off | Adds `x2`, `x5` … to the name. |
| Corner | Bottom right | Or bottom left. Placed above the taskbar. |
| Monitor | Primary | |
| Distance from side / bottom | 16 px / 16 px | |
| Animate / Style / Speed | On / Slide / 300 ms | Slide in from the screen edge, or fade. |
| Scale | 100 % | 100 % = 32×32 icons, 200 % = 64×64. Text size, padding and gaps scale too. |
| Opacity | 100 % | Whole popup. |
| Background colour / opacity | #1C212E / 100 % | |
| Corner radius / Padding | 4 px / 4 px | |
| Font, Bold, Italic | Segoe UI | Any installed font. |
| Font size | 15 px | At 100 % scale. |
| Text colour | White | Optional soft text shadow. |
| Justify items | Right | Left, Center or Right (icon and text move together). |
| Long names | Full width | **Full width**: the box grows to fit. **Wrap** or **Truncate** (with …) at *Max text width* (220 px). |
| Show icons / Icon position | On / Left of text | |
| Gap between items | 2 px with icons, 1 px without | The gap in use switches automatically when icons are turned on or off. |
| Highlight | `trophy` → yellow | Items whose name contains the text (ignoring case) get their own background and text colours. Add as many rules as you like; the first match wins. In a combined box only that item's row is coloured. The count next to each rule shows how many items in the current item list match (hover for examples). |
| Hide | `bottle of` | Items whose name contains the text (ignoring case) aren't shown, e.g. "Bottle of Water" and "Bottle Of Soy Milk". |
| Show hidden items | Off | Shows hidden items anyway, faded, so you can check what your rules catch. |
| Chat log folder | Game default | Change it if your logs live elsewhere. |

## Where things are kept

- Settings: `%APPDATA%\PG Pickup Notifier\config.toml` (plain text; editing it by hand also works, it's picked up within a second)
- Cache: `%LOCALAPPDATA%\PG Pickup Notifier\cache\`
  - `items_v<version>.json` is the item list for the current game data version, re-downloaded only when
    `https://client.projectgorgon.com/fileversion.txt` changes. That's checked at startup, every 30 minutes,
    and whenever an unknown item name turns up.
  - `icons\icon_<IconId>.png`: each icon is downloaded once, the first time you pick that item up.
- Log for troubleshooting: `%LOCALAPPDATA%\PG Pickup Notifier\log.txt`

To uninstall: untick *Start with Windows*, exit from the tray menu, delete the exe and the two folders above.

## How it works

- Only new lines are read. Existing content of today's log is skipped when the app starts.
- At midnight the game starts a new `Chat-YY-MM-DD.log`. The app notices the newer file within a couple of
  seconds and reads it from the first line.
- Item names are matched exactly (case-insensitive as a fallback). Items that aren't in `items.json`
  still pop up, with a `?` placeholder icon.
- Hide and highlight rules are checked against the item name as logged (without the quantity), so
  `trophy` also matches recipe-style names like "Work Order for Trophy Bear Skins (Skinning)".
  Hidden pickups are noted in `log.txt` ("hidden by rule: …").
- The popup is a click-through, always-on-top, per-pixel-alpha window: it never steals focus or blocks clicks.

## Building from source

Needs Rust 1.88 or newer.

On Windows:

```
cargo build --release
```

The exe is `target\release\pg-pickup-notifier.exe`. The icon, version info and DPI-aware manifest are
embedded by `build.rs`.

Cross-compiling from Linux:

```
rustup target add x86_64-pc-windows-gnu
sudo apt install gcc-mingw-w64-x86-64 binutils-mingw-w64-x86-64
cargo build --release --target x86_64-pc-windows-gnu
```

`cargo test` runs the platform-independent tests (log parsing and tailing, config, item lookup, notification
timing). `cargo run --example preview -- out-dir [icon-dir]` renders sample popups to PNG files on any OS.

Project layout:

| File | Purpose |
|---|---|
| `src/chatlog.rs` | Parse pickup lines; follow the newest daily log |
| `src/itemdb.rs` | Version check, `items.json` and icon download/cache |
| `src/resolver.rs` | Background threads: keep the item list current; turn pickups into name + icon |
| `src/notify.rs` | Stack/combine logic, timers, slide/fade animation, composing the popup image |
| `src/render.rs`, `src/font.rs` | Drawing boxes, icons and text (tiny-skia + fontdue, system fonts via fontdb) |
| `src/config.rs` | All settings and their defaults, hide/highlight matching |
| `src/win/app.rs` | Tray app, message loop, frame pacing |
| `src/win/overlay.rs` | The layered overlay window |
| `src/win/tray.rs` | Tray icon and menu |
| `src/win/settings.rs` | Settings window (egui), run as `pg-pickup-notifier.exe --settings` |
