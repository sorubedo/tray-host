//! Unix socket IPC for tray-host.
//!
//! ## Protocol
//!
//! A simple line-based text protocol over a Unix domain socket.
//! One command per line, responses are lines of tab-separated values
//! terminated by `OK` or `ERROR: ...`.
//!
//! | Client sends          | Server responds                              |
//! |-----------------------|----------------------------------------------|
//! | `LIST`                | `title\taddress\ticon_source\n...` then `OK\n`     |
//! | `MENU <address>`      | `id\tlabel\ttype\tenabled\n...` then `OK\n`  |
//! | `ACTIVATE <addr> <id>`| `OK\n` or `ERROR: <msg>\n`                   |
//! | `ACTIVATE_DEFAULT <addr>` | `OK\n` or `ERROR: <msg>\n`               |
//! | `PING`                | `PONG\n`                                     |

use std::path::PathBuf;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::Host;

/// Returns the default socket path.
///
/// Uses `$XDG_RUNTIME_DIR/tray-host.sock` if available,
/// otherwise falls back to `/tmp/tray-host.sock`.
#[must_use]
pub fn default_socket_path() -> PathBuf {
    if let Some(dir) = dirs::runtime_dir() {
        dir.join("tray-host.sock")
    } else {
        PathBuf::from("/tmp/tray-host.sock")
    }
}

/// Runs the Unix socket server, blocking until an error occurs.
///
/// Handles one connection at a time (single-threaded accept loop).
/// Each connection is processed to completion before accepting the next.
///
/// # Errors
/// Returns an error if the socket cannot be bound or if I/O fails.
pub async fn run_socket_server(
    host: Arc<Host>,
    socket_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Clean up any stale socket file
    let _ = std::fs::remove_file(socket_path);

    // Ensure parent directory exists
    if let Some(parent) = std::path::Path::new(socket_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(socket_path)?;
    log::info!("Socket server listening on {socket_path}");

    loop {
        let (stream, _addr) = listener.accept().await?;
        log::debug!("Accepted client connection");
        if let Err(e) = handle_connection(&host, stream).await {
            log::error!("Connection error: {e}");
        }
    }
}

/// Handles a single client connection.
async fn handle_connection(
    host: &Host,
    stream: UnixStream,
) -> Result<(), Box<dyn std::error::Error>> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            // EOF
            break;
        }

        let cmd = line.trim();
        if cmd.is_empty() {
            continue;
        }

        let response = process_command(host, cmd).await;
        writer.write_all(response.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    Ok(())
}

/// Processes a single command and returns the response.
async fn process_command(host: &Host, cmd: &str) -> String {
    let parts: Vec<&str> = cmd.splitn(3, ' ').collect();

    match parts[0] {
        "LIST" => handle_list(host),
        "MENU" => {
            if parts.len() < 2 {
                return "ERROR: missing address argument".to_string();
            }
            handle_menu(host, parts[1])
        }
        "ACTIVATE" => {
            if parts.len() < 3 {
                return "ERROR: missing address or menu_id argument".to_string();
            }
            let menu_id: i32 = match parts[2].parse() {
                Ok(id) => id,
                Err(_) => return "ERROR: menu_id must be an integer".to_string(),
            };
            handle_activate(host, parts[1], menu_id).await
        }
        "ACTIVATE_DEFAULT" => {
            if parts.len() < 2 {
                return "ERROR: missing address argument".to_string();
            }
            handle_activate_default(host, parts[1]).await
        }
        "PING" => "PONG".to_string(),
        _ => format!("ERROR: unknown command: {}", parts[0]),
    }
}

/// Handles the LIST command.
///
/// Output format: `title\taddress\ticon_source` per line.
/// `icon_source` is an absolute path to a cached PNG, a freedesktop icon name,
/// or empty if no icon is available.
fn handle_list(host: &Host) -> String {
    let items = host.list_items_with_icons();
    if items.is_empty() {
        return "OK".to_string();
    }
    let mut out = String::new();
    for (title, addr, icon) in &items {
        let icon_source = icon.as_deref().unwrap_or("");
        out.push_str(&format!("{title}\t{addr}\t{icon_source}\n"));
    }
    out.push_str("OK");
    out
}

/// Handles the MENU command.
fn handle_menu(host: &Host, address: &str) -> String {
    match host.get_menu(address) {
        Some(menu) => {
            if menu.is_empty() {
                return "OK".to_string();
            }
            let mut out = String::new();
            for item in &menu {
                out.push_str(&format!(
                    "{}\t{}\t{}\t{}\n",
                    item.id, item.label, item.menu_type, item.enabled
                ));
            }
            out.push_str("OK");
            out
        }
        None => format!("ERROR: item not found or has no menu: {address}"),
    }
}

/// Handles the ACTIVATE command.
async fn handle_activate(host: &Host, address: &str, menu_id: i32) -> String {
    // Find the menu path for this item
    let menu_path = match host.get_menu_path(address) {
        Some(path) => path,
        None => return format!("ERROR: item not found or has no menu: {address}"),
    };

    // Notify the app that the menu is about to be shown
    let _ = host
        .about_to_show_menuitem(address, &menu_path, 0)
        .await;

    match host
        .activate_menu_item(address, &menu_path, menu_id)
        .await
    {
        Ok(()) => "OK".to_string(),
        Err(e) => format!("ERROR: {e}"),
    }
}

/// Handles the ACTIVATE_DEFAULT command.
async fn handle_activate_default(host: &Host, address: &str) -> String {
    match host.activate_default(address, 0, 0).await {
        Ok(()) => "OK".to_string(),
        Err(e) => format!("ERROR: {e}"),
    }
}

/// Connects to a tray-host daemon, sends a command, and returns the response.
///
/// Reads all response lines up to (and excluding) the terminating `OK` line.
/// The `OK` / `ERROR: ...` line is *not* included in the returned string.
///
/// # Errors
/// Returns an error if the socket cannot be connected or I/O fails.
pub async fn send_command(
    socket_path: &str,
    cmd: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path).await?;
    let (reader, mut writer) = stream.into_split();

    writer
        .write_all(format!("{cmd}\n").as_bytes())
        .await?;
    writer.flush().await?;

    let mut reader = BufReader::new(reader);
    let mut result = String::new();
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed == "OK" {
            break;
        }
        if let Some(err) = trimmed.strip_prefix("ERROR: ") {
            return Err(err.into());
        }
        result.push_str(trimmed);
        result.push('\n');
    }

    // Remove trailing newline if present
    if result.ends_with('\n') {
        result.pop();
    }

    Ok(result)
}
