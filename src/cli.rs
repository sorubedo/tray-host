use clap::{Parser, Subcommand};
use clap_complete::Shell;

#[derive(Parser, Debug)]
#[command(
    name = "tray-host",
    version,
    about = "Headless StatusNotifierItem host for use with external launchers like fuzzel/rofi/dmenu",
    long_about = None
)]
pub struct Cli {
    /// Prints debug information to app.log file
    #[arg(short, long, global = true, action = clap::ArgAction::SetTrue, default_value_t = false)]
    pub debug: bool,

    /// Generates completion scripts for the specified shell
    #[arg(long, value_name = "SHELL", value_enum)]
    pub completions: Option<Shell>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Start the background daemon (D-Bus watcher + Unix socket server)
    Daemon,

    /// Interactive mode: list tray items → pick with fuzzel → show menu → pick → activate.
    ///
    /// This is the main command for end users. It handles the full two-step
    /// selection flow automatically: first pick a tray icon, then pick a menu
    /// item (or left-click if the icon has no menu).
    #[command(name = "pick")]
    Pick {
        /// Picker command to use instead of fuzzel (e.g. "rofi -dmenu", "dmenu")
        #[arg(long, default_value = "fuzzel --dmenu -d '\t' --with-nth=2")]
        picker: String,
    },

    /// List all registered tray items (for piping to fuzzel/rofi/dmenu)
    ///
    /// Output format: title\taddress\ticon_source
    List,

    /// List menu items for a tray item (for piping to fuzzel/rofi/dmenu)
    ///
    /// Output format: id\tlabel\ttype\tenabled
    Menu {
        /// D-Bus address of the tray item (from `list` output)
        address: String,
    },

    /// Activate a menu item (sends a click event to the application via D-Bus)
    Activate {
        /// D-Bus address of the tray item
        address: String,
        /// Menu item ID (from `menu` output)
        menu_id: i32,
    },

    /// Send a default activation (simulates left-click on the tray icon)
    ActivateDefault {
        /// D-Bus address of the tray item
        address: String,
    },
}
