//! # tray-host
//!
//! A headless StatusNotifierItem host (system tray) for embedded and headless systems.
//! Designed to work with external launchers like fuzzel, rofi, or dmenu.
//!
//! ## Architecture
//!
//! ```text
//! tray-host daemon  (background process)
//!   ├── system_tray::Client → D-Bus StatusNotifierWatcher
//!   ├── Host → in-memory tray items (Arc<Mutex<HashMap>>)
//!   └── Unix socket → $XDG_RUNTIME_DIR/tray-host.sock
//!
//! tray-host list | fuzzel   → user picks a tray icon
//! tray-host menu <addr> | fuzzel → user picks a menu item
//! tray-host activate <addr> <id> → sends click to the app
//! ```
//!
//! ## Library usage
//!
//! ```rust,no_run
//! use tray_host::Host;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let host = Host::new().await?;
//!     for (title, addr, icon) in host.list_items() {
//!         println!("{title}\t{addr}\t{}", icon.as_deref().unwrap_or(""));
//!     }
//!     Ok(())
//! }
//! ```

pub mod cli;
pub mod config;
pub mod socket;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use system_tray::client::Client;
use system_tray::item::IconPixmap;
use tokio::sync::broadcast;

// Re-export key types so consumers don't need to depend on system-tray directly.
pub use system_tray::client::{ActivateRequest, Event, UpdateEvent};
pub use system_tray::item::StatusNotifierItem;
pub use system_tray::menu::{MenuItem, TrayMenu};

/// A flattened menu item for display and selection.
#[derive(Debug, Clone)]
pub struct FlatMenuItem {
    /// The menu item's D-Bus id.
    pub id: i32,
    /// Human-readable label.
    pub label: String,
    /// Whether the item is enabled (clickable).
    pub enabled: bool,
    /// MenuType as a string: "standard" or "separator".
    pub menu_type: String,
}

/// A headless host for StatusNotifierItem tray icons.
///
/// Wraps [`system_tray::client::Client`] with a simplified API for
/// embedded, headless, and programmatic use.
///
/// The host maintains the D-Bus `StatusNotifierWatcher`, so as long as
/// this struct lives, applications can register their tray icons.
#[derive(Debug)]
pub struct Host {
    client: Client,
    items: Arc<Mutex<HashMap<String, (StatusNotifierItem, Option<TrayMenu>)>>>,
    sorting: bool,
}

impl Host {
    /// Creates a new Host, connecting to the session D-Bus and
    /// registering as a StatusNotifierHost.
    ///
    /// # Errors
    /// Returns an error if the D-Bus session connection fails.
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_sorting(false).await
    }

    /// Creates a new Host with optional alphabetical sorting of items.
    ///
    /// # Errors
    /// Returns an error if the D-Bus session connection fails.
    pub async fn with_sorting(sorting: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let client = Client::new().await?;
        let items = client.items();
        Ok(Self {
            client,
            items,
            sorting,
        })
    }

    /// Returns a shared reference to the items map.
    ///
    /// The map is kept in sync with D-Bus events by the underlying
    /// `system_tray::Client`. Key = D-Bus address (e.g. `":1.58"`),
    /// value = `(StatusNotifierItem, Option<TrayMenu>)`.
    #[must_use]
    pub fn items(
        &self,
    ) -> &Arc<Mutex<HashMap<String, (StatusNotifierItem, Option<TrayMenu>)>>> {
        &self.items
    }

    /// Returns a sorted list of `(title, address, icon_name)` for all registered items.
    ///
    /// The title is the best available display name, falling back through:
    /// `title → tooltip.title → id → address → "Unknown"`.
    /// `icon_name` is the raw freedesktop icon name from D-Bus (`None` if absent).
    /// If `sorting` is enabled, results are sorted alphabetically by title.
    #[must_use]
    pub fn list_items(&self) -> Vec<(String, String, Option<String>)> {
        let map = self.items.lock().expect("items lock poisoned");
        let mut items: Vec<(String, String, Option<String>)> = map
            .iter()
            .map(|(addr, (sni, _))| {
                (
                    get_title(sni, addr),
                    addr.clone(),
                    sni.icon_name.clone(),
                )
            })
            .collect();
        if self.sorting {
            items.sort_by(|a, b| a.0.cmp(&b.0));
        }
        items
    }

    /// Like [`list_items`](Self::list_items) but resolves and caches icons.
    ///
    /// Each returned tuple is `(title, address, icon_source)` where `icon_source`
    /// is an absolute file path to a cached PNG, a freedesktop icon name,
    /// or `None` if no icon is available.
    #[must_use]
    pub fn list_items_with_icons(&self) -> Vec<(String, String, Option<String>)> {
        let map = self.items.lock().expect("items lock poisoned");
        let mut items: Vec<(String, String, Option<String>)> = map
            .iter()
            .map(|(addr, (sni, _))| {
                let title = get_title(sni, addr);
                let icon = resolve_icon(sni, addr);
                (title, addr.clone(), icon)
            })
            .collect();
        if self.sorting {
            items.sort_by(|a, b| a.0.cmp(&b.0));
        }
        items
    }

    /// Returns the flattened menu for a tray item, identified by its D-Bus address.
    ///
    /// Recursively flattens the nested menu tree. Returns `None` if the item
    /// isn't found or has no menu.
    #[must_use]
    pub fn get_menu(&self, address: &str) -> Option<Vec<FlatMenuItem>> {
        let map = self.items.lock().expect("items lock poisoned");
        let (_sni, menu) = map.get(address)?;
        let menu = menu.as_ref()?;
        let mut result = Vec::new();
        flatten_menu(&menu.submenus, &mut result);
        Some(result)
    }

    /// Subscribes to tray events (Add, Update, Remove).
    ///
    /// Returns a [`broadcast::Receiver`] that receives all subsequent events.
    /// Call this **before** entering your event loop, then read initial state
    /// with [`list_items`](Self::list_items) or [`items`](Self::items).
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.client.subscribe()
    }

    /// Sends an activation request to an application via D-Bus.
    ///
    /// Delegates to [`Client::activate`].
    ///
    /// # Errors
    /// Returns an error if the D-Bus call fails.
    pub async fn activate(
        &self,
        request: ActivateRequest,
    ) -> Result<(), system_tray::error::Error> {
        self.client.activate(request).await
    }

    /// Convenience method: activate a specific menu item.
    ///
    /// # Errors
    /// Returns an error if the D-Bus call fails.
    pub async fn activate_menu_item(
        &self,
        address: &str,
        menu_path: &str,
        submenu_id: i32,
    ) -> Result<(), system_tray::error::Error> {
        self.client
            .activate(ActivateRequest::MenuItem {
                address: address.to_string(),
                menu_path: menu_path.to_string(),
                submenu_id,
            })
            .await
    }

    /// Convenience method: send a default activation (simulates left-click on the tray icon).
    ///
    /// `x` and `y` are screen coordinates (hints for where to show windows).
    ///
    /// # Errors
    /// Returns an error if the D-Bus call fails.
    pub async fn activate_default(
        &self,
        address: &str,
        x: i32,
        y: i32,
    ) -> Result<(), system_tray::error::Error> {
        self.client
            .activate(ActivateRequest::Default {
                address: address.to_string(),
                x,
                y,
            })
            .await
    }

    /// Notifies an application that its menu is about to be shown.
    ///
    /// Returns `true` if the menu needs an update. Call with `id: 0` for the root menu.
    ///
    /// # Errors
    /// Returns an error if the D-Bus call fails.
    pub async fn about_to_show_menuitem(
        &self,
        address: &str,
        menu_path: &str,
        id: i32,
    ) -> Result<bool, system_tray::error::Error> {
        self.client
            .about_to_show_menuitem(address.to_string(), menu_path.to_string(), id)
            .await
    }

    /// Returns whether alphabetical sorting is enabled.
    #[must_use]
    pub fn sorting(&self) -> bool {
        self.sorting
    }

    /// Looks up the D-Bus menu path for a tray item.
    ///
    /// Returns `None` if the item isn't found or has no menu.
    #[must_use]
    pub fn get_menu_path(&self, address: &str) -> Option<String> {
        let map = self.items.lock().expect("items lock poisoned");
        let (sni, _menu) = map.get(address)?;
        sni.menu.clone()
    }
}

/// Returns the icon cache directory (`$XDG_CACHE_HOME/tray-host/icons/`).
fn icon_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("tray-host")
        .join("icons")
}

/// Clean the entire icon cache directory (called on daemon startup).
pub fn clean_icon_cache() {
    let dir = icon_cache_dir();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Convert a single ARGB32 (big-endian) IconPixmap to PNG bytes.
///
/// The input pixels are in network byte order: A, R, G, B per pixel.
/// Output is RGBA PNG.
fn argb32_to_png(pixmap: &IconPixmap) -> Result<Vec<u8>, String> {
    let width = pixmap.width as u32;
    let height = pixmap.height as u32;
    let pixels = &pixmap.pixels;

    let expected = (width * height * 4) as usize;
    if pixels.len() != expected {
        return Err(format!(
            "pixmap size mismatch: got {} bytes, expected {expected}",
            pixels.len()
        ));
    }

    // ARGB big-endian → RGBA
    let mut rgba = Vec::with_capacity(pixels.len());
    for chunk in pixels.chunks_exact(4) {
        // chunk[0]=A, chunk[1]=R, chunk[2]=G, chunk[3]=B (big-endian ARGB)
        rgba.push(chunk[1]); // R
        rgba.push(chunk[2]); // G
        rgba.push(chunk[3]); // B
        rgba.push(chunk[0]); // A
    }

    // Encode PNG to memory
    let mut buf = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut buf, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("png header: {e}"))?;
        writer
            .write_image_data(&rgba)
            .map_err(|e| format!("png write: {e}"))?;
        writer
            .finish()
            .map_err(|e| format!("png finish: {e}"))?;
    }
    Ok(buf)
}

/// Resolve the best icon source for a StatusNotifierItem.
///
/// 1. If `icon_pixmap` exists → convert largest pixmap to PNG, cache to disk, return file path
/// 2. Else if `icon_name` exists → return icon name (fuzzel resolves from icon theme)
/// 3. Else → return `None`
fn resolve_icon(sni: &StatusNotifierItem, address: &str) -> Option<String> {
    // Try pixmap first (guaranteed to display if available)
    if let Some(pixmaps) = &sni.icon_pixmap {
        if !pixmaps.is_empty() {
            let best = pixmaps.iter().max_by_key(|p| p.width * p.height)?;
            let png_bytes = match argb32_to_png(best) {
                Ok(b) => b,
                Err(e) => {
                    log::error!("PNG conversion failed for {address}: {e}");
                    return sni.icon_name.clone();
                }
            };
            let dir = icon_cache_dir();
            if let Err(e) = std::fs::create_dir_all(&dir) {
                log::error!("Failed to create icon cache dir: {e}");
                return sni.icon_name.clone();
            }
            // Content-addressed filename: same icon → same file, no duplicates
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            png_bytes.hash(&mut hasher);
            let hash = hasher.finish();
            let path = dir.join(format!("{hash:016x}.png"));
            if let Err(e) = std::fs::write(&path, &png_bytes) {
                log::error!("Failed to write cached icon for {address}: {e}");
                return sni.icon_name.clone();
            }
            return Some(path.to_string_lossy().to_string());
        }
    }
    // Fallback: freedesktop icon name (fuzzel resolves from system theme)
    sni.icon_name.clone()
}

/// Get the best display title for a StatusNotifierItem.
///
/// Never returns an empty string. Fallback chain:
/// `title → tooltip.title → id → address → "Unknown"`
fn get_title(item: &StatusNotifierItem, address: &str) -> String {
    let title = item
        .title
        .clone()
        .or_else(|| item.tool_tip.as_ref().map(|t| t.title.clone()))
        .unwrap_or_default();

    // Skip values that match icon_name (Electron apps use icon identifier as title/id)
    let matches_icon = |v: &str| {
        item.icon_name.as_deref() == Some(v)
            // Also skip if value looks like a technical icon identifier:
            // lowercase, no spaces, contains underscores (e.g. "chrome_status_icon_1")
            || (!v.contains(' ') && v.chars().any(|c| c == '_')
                && !v.chars().any(|c| c.is_uppercase()))
    };

    if !title.is_empty() && !matches_icon(&title) {
        return title;
    }

    if !item.id.is_empty() && !matches_icon(&item.id) {
        return item.id.clone();
    }

    if !address.is_empty() {
        return address.to_string();
    }

    "Unknown".to_string()
}

/// Recursively flatten a menu tree into a Vec of FlatMenuItem.
fn flatten_menu(items: &[MenuItem], out: &mut Vec<FlatMenuItem>) {
    for item in items {
        out.push(FlatMenuItem {
            id: item.id,
            label: item.label.clone().unwrap_or_default(),
            enabled: item.enabled,
            menu_type: format!("{:?}", item.menu_type).to_lowercase(),
        });
        if !item.submenu.is_empty() {
            flatten_menu(&item.submenu, out);
        }
    }
}
