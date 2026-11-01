//! Power and session state: battery from the kernel's power supplies (what
//! UPower reads), lock and unlock from logind over D-Bus.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use super::wayland::Waker;
use crate::DesktopEvent;

const LOGIN1: &str = "org.freedesktop.login1";
const SESSION: &str = "org.freedesktop.login1.Session";

/// Running on battery: no mains or USB supply is online and a battery is
/// discharging. Machines without a battery never are.
pub(super) fn on_battery(power_supplies: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(power_supplies) else {
        return false;
    };
    let read = |dir: &Path, name: &str| {
        std::fs::read_to_string(dir.join(name)).map_or_else(|_| String::new(), |s| s.trim().into())
    };
    let mut discharging = false;
    for entry in entries.flatten() {
        let dir = entry.path();
        match read(&dir, "type").as_str() {
            "Mains" | "USB" if read(&dir, "online") == "1" => return false,
            "Battery" if read(&dir, "scope") != "Device" => {
                discharging |= read(&dir, "status") == "Discharging";
            }
            _ => {}
        }
    }
    discharging
}

/// Watches the logind session Diorama runs in on a thread of its own and
/// queues `SessionLocked`/`SessionUnlocked` into `events`. Without logind
/// (or a session) it logs why and nothing is queued.
pub(super) fn watch_lock(events: Arc<Mutex<Vec<DesktopEvent>>>, waker: Waker) {
    let spawned = std::thread::Builder::new()
        .name("logind".into())
        .spawn(move || {
            if let Err(e) = watch(&events, &waker) {
                tracing::info!("no lock events from logind: {e}");
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("logind thread: {e}");
    }
}

fn watch(events: &Mutex<Vec<DesktopEvent>>, waker: &Waker) -> zbus::Result<()> {
    let conn = Connection::system()?;
    let path = session_path(&conn)?;
    tracing::debug!("lock events from logind session {}", path.as_str());
    let push = |locked: bool| {
        let e = if locked {
            DesktopEvent::SessionLocked
        } else {
            DesktopEvent::SessionUnlocked
        };
        if let Ok(mut events) = events.lock() {
            events.push(e);
        }
        waker.wake();
    };
    // Subscribe before reading the current state, so no change falls between.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(LOGIN1)?
        .path(path.as_str())?
        .build();
    let messages = MessageIterator::for_match_rule(rule, &conn, None)?;
    let session = proxy(&conn, path.as_str(), SESSION)?;
    if session.get_property::<bool>("LockedHint").unwrap_or(false) {
        push(true);
    }
    for msg in messages {
        let msg = msg?;
        let header = msg.header();
        let Some(member) = header.member() else {
            continue;
        };
        match member.as_str() {
            // Asks the session's locker to lock (`loginctl lock-session`).
            "Lock" => push(true),
            "Unlock" => push(false),
            // Set by lockers (and desktops) once the lock screen is up or gone.
            "PropertiesChanged" => {
                let Ok((_, changed, _)) =
                    msg.body()
                        .deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>()
                else {
                    continue;
                };
                if let Some(Ok(locked)) = changed.get("LockedHint").map(bool::try_from) {
                    push(locked);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// The session to follow: ours (`XDG_SESSION_ID`, else our process), else
/// the user's graphical session, else the user's first one. A daemon
/// started by a systemd user unit belongs to no session.
fn session_path(conn: &Connection) -> zbus::Result<OwnedObjectPath> {
    let manager = proxy(
        conn,
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )?;
    if let Ok(id) = std::env::var("XDG_SESSION_ID")
        && let Ok(path) = manager.call::<_, _, OwnedObjectPath>("GetSession", &(id,))
    {
        return Ok(path);
    }
    if let Ok(path) =
        manager.call::<_, _, OwnedObjectPath>("GetSessionByPID", &(std::process::id(),))
    {
        return Ok(path);
    }
    // `user/self` only resolves for callers inside a session.
    let uid = rustix::process::getuid().as_raw();
    let user: OwnedObjectPath = manager.call("GetUser", &(uid,))?;
    let user = proxy(conn, user.as_str(), "org.freedesktop.login1.User")?;
    if let Ok((_, path)) = user.get_property::<(String, OwnedObjectPath)>("Display")
        && path.as_str() != "/"
    {
        return Ok(path);
    }
    user.get_property::<Vec<(String, OwnedObjectPath)>>("Sessions")?
        .into_iter()
        .next()
        .map(|(_, path)| path)
        .ok_or_else(|| zbus::Error::Failure("the user has no logind session".into()))
}

/// A logind proxy that reads properties when asked (no cache to fill).
fn proxy<'a>(conn: &Connection, path: &'a str, interface: &'a str) -> zbus::Result<Proxy<'a>> {
    zbus::blocking::proxy::Builder::new(conn)
        .destination(LOGIN1)?
        .path(path)?
        .interface(interface)?
        .cache_properties(CacheProperties::No)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supply(root: &Path, name: &str, files: &[(&str, &str)]) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for (file, value) in files {
            std::fs::write(dir.join(file), format!("{value}\n")).unwrap();
        }
    }

    fn root(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("diorama-power-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn battery_only_when_unplugged_and_discharging() {
        let dir = root("laptop");
        supply(&dir, "AC", &[("type", "Mains"), ("online", "1")]);
        supply(&dir, "BAT0", &[("type", "Battery"), ("status", "Charging")]);
        assert!(!on_battery(&dir));
        supply(&dir, "AC", &[("type", "Mains"), ("online", "0")]);
        supply(
            &dir,
            "BAT0",
            &[("type", "Battery"), ("status", "Discharging")],
        );
        assert!(on_battery(&dir));
        // A wireless mouse's battery is not the machine's.
        let dir = root("desktop");
        supply(
            &dir,
            "hidpp_battery_0",
            &[
                ("type", "Battery"),
                ("scope", "Device"),
                ("status", "Discharging"),
            ],
        );
        assert!(!on_battery(&dir));
        assert!(!on_battery(&dir.join("missing")));
    }
}
