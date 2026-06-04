# tray-host

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)

**⚠️ Vibe-coded with [Claude Code](https://claude.ai/code) (DeepSeek V4 Pro).**

A **headless system tray host** (StatusNotifierItem daemon) designed to work with external launchers like **fuzzel**, **rofi**, or **dmenu** — inspired by the [cliphist](https://github.com/sentriz/cliphist) + fuzzel architecture.

## Credits

| Project | Role |
|---------|------|
| [tray-tui](https://github.com/Levizor/tray-tui) | Original TUI system tray — stripped down to become this project |
| [system-tray](https://github.com/jakestanger/system-tray) | D-Bus StatusNotifierItem backend that does all the heavy lifting |
| [cliphist](https://github.com/sentriz/cliphist) | Architectural inspiration: daemon + socket + picker |

This project is a complete rewrite of `tray-tui` — all TUI/frontend code removed, replaced with a Unix socket daemon + CLI designed for composability with fuzzy pickers.

## Architecture

```
D-Bus Session Bus
    │  Apps (Discord, Dropbox, copyq...) register tray icons
    ▼
tray-host daemon (background process)
    ├── system_tray::Client → D-Bus StatusNotifierWatcher
    ├── Host → in-memory tray item cache
    └── Unix socket → $XDG_RUNTIME_DIR/tray-host.sock
         ▲
         │
    tray-host pick    ← the only command you need
```

## Installation

### Cargo

```
cargo install --git https://github.com/sorubedo/tray-tui
```

### From source

```
git clone https://github.com/sorubedo/tray-tui
cd tray-tui
cargo build --release
```

## Usage

### 1. Start the daemon

Add to your compositor autostart, or run manually:

```
tray-host daemon &
```

### 2. Interact with tray icons

```
tray-host pick
```

That's it. `pick` handles the full flow:

1. Lists tray icons → spawns **fuzzel** for selection
2. If the icon has a menu → spawns **fuzzel** again for menu selection
3. Sends the click via D-Bus to the application
4. If no menu → sends a default (left-click) activation

Use a different picker:

```
tray-host pick --picker "rofi -dmenu -show-icons"
```

## Configuration

Optional config at `$XDG_CONFIG_HOME/tray-host/config.toml`:

```toml
sorting = false   # sort tray items alphabetically
```

## Library usage

```rust
use tray_host::Host;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = Host::new().await?;
    for (addr, title) in host.list_items() {
        println!("{addr}\t{title}");
    }
    Ok(())
}
```

## License

MIT — see [LICENSE](./LICENSE).
