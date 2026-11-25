//! Picking a colour from anywhere on the screen, with the desktop portal's
//! picker (GNOME, KDE, wlroots desktops).

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use zbus::MatchRule;
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::message::Type;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::{Error, Result};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const REQUEST: &str = "org.freedesktop.portal.Request";

/// Without an answer by then, it is cancelled.
const PATIENCE: Duration = Duration::from_secs(120);

/// Lets the user pick a colour on the screen; `None` if they did not. Blocks.
pub fn pick_color() -> Result<Option<[u8; 3]>> {
    let (answer, answered) = mpsc::channel();
    std::thread::spawn(move || answer.send(ask()));
    let Ok(answer) = answered.recv_timeout(PATIENCE) else {
        return Ok(None);
    };
    answer.map_err(|e| Error::Os {
        call: "PickColor",
        message: format!("this desktop has no colour picker for programs ({e})"),
    })
}

fn ask() -> zbus::Result<Option<[u8; 3]>> {
    let conn = Connection::session()?;
    // Listening comes before asking: the answer may be quick.
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface(REQUEST)?
        .member("Response")?
        .build();
    let answers = MessageIterator::for_match_rule(rule, &conn, None)?;
    let portal = Proxy::new(
        &conn,
        PORTAL,
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Screenshot",
    )?;
    let options: HashMap<&str, Value<'_>> = HashMap::new();
    let request: OwnedObjectPath = portal.call("PickColor", &("", options))?;
    for message in answers {
        let message = message?;
        let ours = message
            .header()
            .path()
            .is_some_and(|p| p.as_str() == request.as_str());
        if !ours {
            continue;
        }
        let (code, results): (u32, HashMap<String, OwnedValue>) = message.body().deserialize()?;
        let color = results.get("color").filter(|_| code == 0);
        return Ok(color
            .and_then(|c| <(f64, f64, f64)>::try_from(c.try_clone().ok()?).ok())
            .map(|(r, g, b)| [r, g, b].map(byte)));
    }
    Ok(None)
}

fn byte(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A desktop's side, as the portal does it.
    struct Desktop;

    #[zbus::interface(name = "org.freedesktop.portal.Screenshot")]
    impl Desktop {
        async fn pick_color(
            &self,
            _parent: String,
            _options: HashMap<String, OwnedValue>,
            #[zbus(connection)] conn: &zbus::Connection,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            let path = "/org/freedesktop/portal/desktop/request/1_1/t";
            let results = HashMap::from([("color", Value::from((1.0f64, 0.5f64, 0.0f64)))]);
            conn.emit_signal(None::<&str>, path, REQUEST, "Response", &(0u32, results))
                .await?;
            Ok(OwnedObjectPath::try_from(path).unwrap())
        }
    }

    /// `dbus-run-session cargo test -p diorama-platform dropper -- --ignored`
    /// (never on a desktop's own bus).
    #[test]
    #[ignore = "needs a session bus of its own"]
    fn the_portals_answer_is_the_colour() {
        let _desktop = zbus::blocking::connection::Builder::session()
            .unwrap()
            .serve_at("/org/freedesktop/portal/desktop", Desktop)
            .unwrap()
            .name(PORTAL)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(pick_color().unwrap(), Some([255, 128, 0]));
    }

    #[test]
    fn colours_become_bytes() {
        assert_eq!([0.0, 0.5, 1.0, 7.0, -1.0].map(byte), [0, 128, 255, 255, 0]);
    }
}
