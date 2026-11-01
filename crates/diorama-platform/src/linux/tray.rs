//! Tray icon through StatusNotifierItem (KDE, waybar, most panels; GNOME
//! with the AppIndicator extension) with a `com.canonical.dbusmenu` menu:
//! open the UI, move playlists on, pause or resume, quit. zbus serves both
//! on its own thread; choices reach `Platform::poll_events` as
//! [`DesktopEvent::Tray`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use zbus::blocking::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::wayland::Waker;
use crate::{DesktopEvent, Error, Result, TrayCommand, TrayState};

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const MENU_INTERFACE: &str = "com.canonical.dbusmenu";

fn bus_err(call: &'static str) -> impl FnOnce(zbus::Error) -> Error {
    move |e| Error::Os {
        call,
        message: e.to_string(),
    }
}

/// What the menu shows and where choices go; shared with zbus's thread.
#[derive(Debug)]
struct Shared {
    state: Mutex<TrayState>,
    /// Bumped on every menu change (dbusmenu layout revision).
    revision: AtomicU32,
    events: Arc<Mutex<Vec<DesktopEvent>>>,
    waker: Waker,
}

impl Shared {
    fn send(&self, command: TrayCommand) {
        if let Ok(mut events) = self.events.lock() {
            events.push(DesktopEvent::Tray(command));
        }
        self.waker.wake();
    }

    fn state(&self) -> TrayState {
        self.state.lock().map(|s| *s).unwrap_or_default()
    }
}

/// The icon while it is shown; removed from the panel when dropped.
#[derive(Debug)]
pub(super) struct Tray {
    conn: Connection,
    shared: Arc<Shared>,
}

impl Tray {
    /// Publishes the icon (`rgba`: `size`×`size`, straight alpha) and asks
    /// the panel's StatusNotifierWatcher to show it.
    pub(super) fn show(
        rgba: &[u8],
        size: u32,
        state: TrayState,
        events: Arc<Mutex<Vec<DesktopEvent>>>,
        waker: Waker,
    ) -> Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(state),
            revision: AtomicU32::new(1),
            events,
            waker,
        });
        let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
        let conn = zbus::blocking::connection::Builder::session()
            .map_err(bus_err("session bus"))?
            .name(name.as_str())
            .map_err(bus_err("bus name"))?
            .serve_at(
                ITEM_PATH,
                Item {
                    shared: Arc::clone(&shared),
                    pixmap: argb(rgba, size),
                },
            )
            .map_err(bus_err("serve StatusNotifierItem"))?
            .serve_at(
                MENU_PATH,
                Menu {
                    shared: Arc::clone(&shared),
                },
            )
            .map_err(bus_err("serve dbusmenu"))?
            .build()
            .map_err(bus_err("session bus"))?;
        conn.call_method(
            Some("org.kde.StatusNotifierWatcher"),
            "/StatusNotifierWatcher",
            Some("org.kde.StatusNotifierWatcher"),
            "RegisterStatusNotifierItem",
            &(name.as_str(),),
        )
        .map_err(|e| Error::Os {
            call: "RegisterStatusNotifierItem",
            message: format!("{e} (no tray: the panel has no StatusNotifierItem host)"),
        })?;
        Ok(Self { conn, shared })
    }

    /// Updates the menu (Pause or Resume, Next enabled) when it changed.
    pub(super) fn set_state(&self, state: TrayState) {
        let Ok(mut current) = self.shared.state.lock() else {
            return;
        };
        if *current == state {
            return;
        }
        *current = state;
        drop(current);
        let revision = self.shared.revision.fetch_add(1, Ordering::SeqCst) + 1;
        if let Err(e) = self.conn.emit_signal(
            None::<&str>,
            MENU_PATH,
            MENU_INTERFACE,
            "LayoutUpdated",
            &(revision, 0i32),
        ) {
            tracing::debug!("tray menu update: {e}");
        }
    }
}

/// Images as StatusNotifierItem carries them: width, height, ARGB32
/// (big-endian) pixels.
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

/// `rgba` as a StatusNotifierItem pixmap.
fn argb(rgba: &[u8], size: u32) -> Pixmaps {
    let bytes = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, a]| [a, r, g, b])
        .collect();
    let side = i32::try_from(size).unwrap_or(0);
    vec![(side, side, bytes)]
}

struct Item {
    shared: Arc<Shared>,
    pixmap: Pixmaps,
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    /// A click on the icon opens the UI.
    fn activate(&self, _x: i32, _y: i32) {
        self.shared.send(TrayCommand::OpenUi);
    }

    fn secondary_activate(&self, _x: i32, _y: i32) {}

    fn scroll(&self, _delta: i32, _orientation: &str) {}

    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }

    #[zbus(property)]
    fn id(&self) -> &str {
        "diorama"
    }

    #[zbus(property)]
    fn title(&self) -> &str {
        "Diorama"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }

    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        self.pixmap.clone()
    }

    #[zbus(property)]
    fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        let text = if self.shared.state().paused {
            "Diorama (paused)"
        } else {
            "Diorama"
        };
        (String::new(), Vec::new(), text.into(), String::new())
    }

    /// The menu opens on right click; a left click activates.
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(MENU_PATH).unwrap_or_default()
    }
}

/// Menu entries: id, label, command, enabled. Id 4 is a separator.
fn entries(state: TrayState) -> [(i32, &'static str, Option<TrayCommand>, bool); 5] {
    let pause = if state.paused {
        ("Resume", TrayCommand::Resume)
    } else {
        ("Pause", TrayCommand::Pause)
    };
    [
        (1, "Open Diorama", Some(TrayCommand::OpenUi), true),
        (2, "Next wallpaper", Some(TrayCommand::Next), state.can_next),
        (3, pause.0, Some(pause.1), true),
        (4, "", None, true),
        (5, "Quit", Some(TrayCommand::Quit), true),
    ]
}

type Props = HashMap<String, OwnedValue>;

fn value(v: Value<'_>) -> Option<OwnedValue> {
    OwnedValue::try_from(v).ok()
}

/// dbusmenu properties of one entry.
fn props(label: &str, command: Option<TrayCommand>, enabled: bool) -> Props {
    let mut p = Props::new();
    let mut put = |k: &str, v: Value<'_>| {
        if let Some(v) = value(v) {
            p.insert(k.into(), v);
        }
    };
    match command {
        Some(_) => {
            put("label", Value::from(label));
            put("enabled", Value::from(enabled));
        }
        None => put("type", Value::from("separator")),
    }
    p
}

struct Menu {
    shared: Arc<Shared>,
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    /// The whole (flat) menu whatever part is asked for.
    fn get_layout(
        &self,
        _parent: i32,
        _depth: i32,
        _properties: Vec<String>,
    ) -> (u32, (i32, Props, Vec<OwnedValue>)) {
        let children = entries(self.shared.state())
            .into_iter()
            .filter_map(|(id, label, command, enabled)| {
                value(Value::from((
                    id,
                    props(label, command, enabled),
                    Vec::<OwnedValue>::new(),
                )))
            })
            .collect();
        let mut root = Props::new();
        if let Some(v) = value(Value::from("submenu")) {
            root.insert("children-display".into(), v);
        }
        (
            self.shared.revision.load(Ordering::SeqCst),
            (0, root, children),
        )
    }

    fn get_group_properties(&self, ids: Vec<i32>, _properties: Vec<String>) -> Vec<(i32, Props)> {
        entries(self.shared.state())
            .into_iter()
            .filter(|e| ids.is_empty() || ids.contains(&e.0))
            .map(|(id, label, command, enabled)| (id, props(label, command, enabled)))
            .collect()
    }

    fn get_property(&self, id: i32, name: &str) -> OwnedValue {
        entries(self.shared.state())
            .into_iter()
            .find(|e| e.0 == id)
            .and_then(|(_, label, command, enabled)| props(label, command, enabled).remove(name))
            .or_else(|| value(Value::from("")))
            .unwrap_or_else(|| OwnedValue::from(0u8))
    }

    fn event(&self, id: i32, event_id: &str, _data: Value<'_>, _timestamp: u32) {
        if event_id != "clicked" {
            return;
        }
        let state = self.shared.state();
        if let Some((_, _, Some(command), true)) = entries(state).into_iter().find(|e| e.0 == id) {
            self.shared.send(command);
        }
    }

    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        for (id, event_id, data, timestamp) in events {
            self.event(id, &event_id, Value::from(data), timestamp);
        }
        Vec::new()
    }

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }

    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixmap_is_argb_big_endian() {
        let px = argb(&[1, 2, 3, 4, 5, 6, 7, 8], 1);
        assert_eq!(px[0].2, vec![4, 1, 2, 3, 8, 5, 6, 7]);
    }

    #[test]
    fn menu_follows_state() {
        let paused = entries(TrayState {
            paused: true,
            can_next: false,
        });
        assert_eq!(paused[2].1, "Resume");
        assert!(!paused[1].3, "Next is disabled without a playlist");
        assert!(props("", None, true).contains_key("type"));
    }
}
