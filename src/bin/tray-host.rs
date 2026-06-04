//! tray-host — Headless StatusNotifierItem host.
//!
//! Usage:
//!   tray-host daemon &                    # Start the background daemon
//!   tray-host pick                        # Interactive: pick tray icon → pick menu → activate
//!   tray-host pick --picker "rofi -dmenu" # Use a different picker
//!   tray-host list                        # List all tray items
//!   tray-host menu <addr> | fuzzel        # List menu items, pick with fuzzel
//!   tray-host activate <addr> <id>        # Activate a menu item

use std::io::{self, Write};
use std::sync::Arc;

use clap::{CommandFactory, Parser};
use clap_complete::generate;
use simplelog::{CombinedLogger, Config as LogConfig, LevelFilter, WriteLogger};
use tokio::process::Command as TokioCommand;

use tray_host::cli::{Cli, Command};
use tray_host::config::Config;
use tray_host::socket::{self, default_socket_path};
use tray_host::Host;

static APP_NAME: &str = "tray-host";

/// Magic bytes for Rofi's extended dmenu protocol.
const ICON_SEP: &[u8] = b"\0icon\x1f";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Handle shell completions (no daemon needed)
    if let Some(shell) = cli.completions {
        let mut cmd = Cli::command();
        let mut out = io::stdout();
        generate(shell, &mut cmd, APP_NAME, &mut out);
        return Ok(());
    }

    // Initialize debug logging
    if cli.debug {
        CombinedLogger::init(vec![WriteLogger::new(
            LevelFilter::Debug,
            LogConfig::default(),
            std::fs::File::create("app.log")?,
        )])?;
    }

    let command = cli.command.unwrap_or(Command::Daemon);

    match command {
        Command::Daemon => run_daemon().await,
        Command::Pick { picker } => run_pick(&picker).await,
        Command::List => run_list().await,
        Command::Menu { address } => run_menu(&address).await,
        Command::Activate { address, menu_id } => run_activate(&address, menu_id).await,
        Command::ActivateDefault { address } => run_activate_default(&address).await,
    }
}

/// Start the daemon: connect to D-Bus, cache icons, start socket server, block forever.
async fn run_daemon() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::new(&None)?;
    let host = Host::with_sorting(config.sorting).await?;
    log::info!("Host initialized, D-Bus watcher running");

    // Cache initial icons
    let _ = host.list_items_with_icons();
    log::info!("Initial icons cached");

    let socket_path = default_socket_path();
    let socket_path_str = socket_path.to_string_lossy().to_string();

    println!("Listening on {socket_path_str}");

    let host = Arc::new(host);

    // Spawn background task to keep icon cache in sync with D-Bus events
    let host_bg = host.clone();
    tokio::spawn(async move {
        let mut rx = host_bg.subscribe();
        while let Ok(event) = rx.recv().await {
            match event {
                system_tray::client::Event::Add(addr, _) => {
                    log::debug!("Icon cache: add {addr}");
                    host_bg.refresh_icon(&addr);
                }
                system_tray::client::Event::Update(addr, update) => {
                    // Only re-cache when the icon actually changes
                    if matches!(update, system_tray::client::UpdateEvent::Icon { .. }) {
                        log::debug!("Icon cache: icon changed for {addr}");
                        host_bg.refresh_icon(&addr);
                    }
                }
                system_tray::client::Event::Remove(_addr) => {
                    log::debug!("Icon cache: remove {_addr}");
                }
            }
        }
    });

    socket::run_socket_server(host, &socket_path_str).await
}

/// Interactive pick using fuzzel's native icon support via Rofi extended dmenu protocol.
///
/// Flow:
/// 1. LIST → parse title/address/icon_source
/// 2. Format: `address\ttitle\0icon\x1ficon_source\n` (addr hidden by --with-nth=2)
/// 3. Pipe to fuzzel → user sees real icons
/// 4. Extract address from first tab-separated field in fuzzel output
/// 5. MENU → fuzzel → ACTIVATE (or ACTIVATE_DEFAULT if no menu)
async fn run_pick(picker: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Parse picker command into program + args
    let picker_parts: Vec<&str> = picker.split_whitespace().collect();
    let picker_prog = picker_parts.first().map_or("fuzzel", |s| *s);
    let picker_args: Vec<&str> = picker_parts.iter().skip(1).copied().collect();

    // Step 1: Get tray items from daemon
    let raw = send_or_fail("LIST").await?;
    let lines: Vec<&str> = raw.lines().filter(|l| !l.is_empty() && *l != "OK").collect();

    if lines.is_empty() {
        eprintln!("No tray items currently registered.");
        return Ok(());
    }

    // Step 2: Build fuzzel input. Display text = "address\ttitle",
    // extended protocol adds the icon after \0. fuzzel --with-nth=2 hides
    // the address column but outputs the full line, so we always get addr.
    let mut input = Vec::new();
    for line in &lines {
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        let title = parts.first().unwrap_or(&"Unknown");
        let addr = parts.get(1).unwrap_or(&"");
        let icon = parts.get(2).unwrap_or(&"");

        // Display text: address\ttitle (addr hidden by --with-nth=2)
        input.extend_from_slice(addr.as_bytes());
        input.push(b'\t');
        input.extend_from_slice(title.as_bytes());

        // Icon metadata via extended dmenu protocol
        if !icon.is_empty() {
            input.extend_from_slice(ICON_SEP);
            input.extend_from_slice(icon.as_bytes());
        }

        input.push(b'\n');
    }

    // Step 3: Pipe to fuzzel
    let selected = pipe_bytes_to_picker(picker_prog, &picker_args, &input).await?;
    if selected.is_empty() {
        return Ok(()); // user cancelled
    }

    // Step 4: Extract address — always the first tab-separated field
    let address = selected.split('\t').next().unwrap_or(&selected);
    log::debug!("User picked tray item: {address}");

    // Step 5: Get menu and let user pick a menu item (plain text, no icons needed)
    let menu = send_or_fail(&format!("MENU {address}")).await?;

    if menu.trim().is_empty() || menu.trim() == "OK" {
        // No menu — send default activation (left-click)
        log::debug!("No menu, sending default activation");
        send_or_fail(&format!("ACTIVATE_DEFAULT {address}")).await?;
    } else {
        // Format: id\tlabel (drop type and enabled — not useful for selection)
        let menu_input = menu
            .lines()
            .filter(|l| !l.is_empty() && *l != "OK")
            .map(|l| {
                let parts: Vec<&str> = l.splitn(3, '\t').collect();
                let id = parts.first().unwrap_or(&"0");
                let label = parts.get(1).unwrap_or(&"");
                format!("{id}\t{label}")
            })
            .collect::<Vec<String>>()
            .join("\n");

        let menu_selected = pipe_text_to_picker(picker_prog, &picker_args, &menu_input).await?;
        if menu_selected.is_empty() {
            return Ok(()); // user cancelled
        }

        let menu_id_str = menu_selected.split('\t').next().unwrap_or(&menu_selected);
        let menu_id: i32 = menu_id_str
            .parse()
            .map_err(|_| format!("invalid menu id from picker: {menu_id_str}"))?;
        log::debug!("User picked menu item: {menu_id}");

        send_or_fail(&format!("ACTIVATE {address} {menu_id}")).await?;
    }

    Ok(())
}

/// Pipe raw bytes to a picker program, return the selected UTF-8 text.
async fn pipe_bytes_to_picker(
    prog: &str,
    args: &[&str],
    input: &[u8],
) -> Result<String, Box<dyn std::error::Error>> {
    let mut child = TokioCommand::new(prog)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(input).await?;
    }

    let output = child.wait_with_output().await?;
    let selected = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(selected)
}

/// Pipe text to a picker program, return the selected line.
async fn pipe_text_to_picker(
    prog: &str,
    args: &[&str],
    input: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut child = TokioCommand::new(prog)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(input.as_bytes()).await?;
    }

    let output = child.wait_with_output().await?;
    let selected = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(selected)
}

/// Connect to the daemon and list all tray items.
async fn run_list() -> Result<(), Box<dyn std::error::Error>> {
    let response = send_or_fail("LIST").await?;
    print_response(&response);
    Ok(())
}

/// Connect to the daemon and list menu items for a tray item.
async fn run_menu(address: &str) -> Result<(), Box<dyn std::error::Error>> {
    let response = send_or_fail(&format!("MENU {address}")).await?;
    print_response(&response);
    Ok(())
}

/// Connect to the daemon and activate a menu item.
async fn run_activate(address: &str, menu_id: i32) -> Result<(), Box<dyn std::error::Error>> {
    send_or_fail(&format!("ACTIVATE {address} {menu_id}")).await?;
    Ok(())
}

/// Connect to the daemon and send a default activation.
async fn run_activate_default(address: &str) -> Result<(), Box<dyn std::error::Error>> {
    send_or_fail(&format!("ACTIVATE_DEFAULT {address}")).await?;
    Ok(())
}

/// Send a command to the daemon socket, with friendly error handling.
async fn send_or_fail(cmd: &str) -> Result<String, Box<dyn std::error::Error>> {
    let socket_path = default_socket_path();
    let socket_path_str = socket_path.to_string_lossy().to_string();

    match socket::send_command(&socket_path_str, cmd).await {
        Ok(response) => Ok(response),
        Err(e) => {
            let path_str = socket_path_str;
            if e.to_string().contains("connection refused")
                || e.to_string().contains("No such file")
            {
                eprintln!(
                    "Error: daemon is not running.\n\
                     Start it with: tray-host daemon &\n\
                     (socket path: {path_str})"
                );
            } else {
                eprintln!("Error: {e}");
            }
            std::process::exit(1);
        }
    }
}

/// Print a response string to stdout, flushing afterwards.
fn print_response(response: &str) {
    if !response.is_empty() {
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        let _ = handle.write_all(response.as_bytes());
        let _ = handle.write_all(b"\n");
        let _ = handle.flush();
    }
}
