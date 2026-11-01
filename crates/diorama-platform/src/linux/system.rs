//! Session integration: autostart through the XDG autostart folder and the
//! local time of day.

use std::path::PathBuf;

use crate::{Error, Result};

/// `$XDG_CONFIG_HOME/autostart/diorama.desktop`: desktops start every entry
/// there at sign-in (and list it in their startup settings).
fn autostart_file() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|d| d.join("autostart").join("diorama.desktop"))
}

fn io_err(call: &'static str) -> impl FnOnce(std::io::Error) -> Error {
    move |e| Error::Os {
        call,
        message: e.to_string(),
    }
}

/// Starts `command` at sign-in, or stops doing so (`None`).
pub fn set_autostart(command: Option<&str>) -> Result<()> {
    let file = autostart_file().ok_or(Error::DesktopNotFound("home folder"))?;
    let Some(command) = command else {
        return match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io_err("remove")(e)),
            _ => Ok(()),
        };
    };
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(io_err("create autostart folder"))?;
    }
    let tmp = file.with_extension("desktop.tmp");
    std::fs::write(&tmp, desktop_entry(command)).map_err(io_err("write autostart entry"))?;
    std::fs::rename(&tmp, &file).map_err(io_err("rename autostart entry"))
}

/// The command started at sign-in, if any.
pub fn autostart() -> Result<Option<String>> {
    let Some(file) = autostart_file() else {
        return Ok(None);
    };
    match std::fs::read_to_string(&file) {
        Ok(text) => Ok(exec(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_err("read autostart entry")(e)),
    }
}

fn desktop_entry(command: &str) -> String {
    // `%` starts a field code in `Exec`; a literal one is doubled.
    format!(
        "[Desktop Entry]\nType=Application\nName=Diorama\nComment=Animated wallpapers\n\
         Exec={}\nIcon=diorama\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
        command.replace('%', "%%")
    )
}

/// The `Exec` command of an enabled entry (`Hidden=true` means removed).
fn exec(entry: &str) -> Option<String> {
    let mut command = None;
    let mut in_entry = false;
    for line in entry.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if !in_entry {
        } else if line == "Hidden=true" {
            return None;
        } else if let Some(c) = line.strip_prefix("Exec=") {
            command = Some(c.replace("%%", "%"));
        }
    }
    command
}

/// Steam's install folder: native (`~/.local/share/Steam`, `~/.steam/steam`)
/// or the Flatpak's.
pub fn steam_dir() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ]
    .into_iter()
    .find(|p| p.is_dir())
}

/// Fraction of the local day elapsed, 0..1 (`g_Daytime`).
pub fn day_fraction() -> f32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as libc::time_t;
    let mut tm = std::mem::MaybeUninit::<libc::tm>::zeroed();
    // SAFETY: `localtime_r` (which loads the time zone on first use) writes
    // into `tm`, a valid local, and returns null on failure.
    let tm = unsafe {
        if libc::localtime_r(&secs, tm.as_mut_ptr()).is_null() {
            return (now.as_secs() % 86_400) as f32 / 86_400.0;
        }
        tm.assume_init()
    };
    let seconds = f64::from(tm.tm_hour) * 3600.0
        + f64::from(tm.tm_min) * 60.0
        + f64::from(tm.tm_sec.min(59))
        + f64::from(now.subsec_millis()) / 1000.0;
    (seconds / 86_400.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_entry_round_trips() {
        let command = "\"/opt/Diorama 100%/dioramad\" --background";
        let entry = desktop_entry(command);
        assert!(entry.contains("Exec=\"/opt/Diorama 100%%/dioramad\" --background\n"));
        assert_eq!(exec(&entry).as_deref(), Some(command));
        assert_eq!(exec(&format!("{entry}Hidden=true\n")), None);
        // Exec of another group does not count.
        assert_eq!(
            exec("[Desktop Action x]\nExec=other\n[Desktop Entry]\nName=D\n"),
            None
        );
    }

    #[test]
    fn day_fraction_is_a_fraction() {
        let f = day_fraction();
        assert!((0.0..1.0).contains(&f), "{f}");
    }
}
