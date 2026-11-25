//! Presets: what every monitor shows, saved under a number and brought back
//! by it, or by a global hotkey (the modifiers and the number's digit).

use anyhow::{Context, Result};
use diorama_ipc::{HotkeyStatus, Modifiers, Preset, SavedRun, SavedWallpaper};
use diorama_platform::HotkeyModifiers;

use crate::session::Look;
use crate::{Daemon, Slot, same_path, slots};

/// Longest name kept.
const NAME_CHARS: usize = 60;

/// The saved presets, by number.
#[derive(Debug, Default)]
pub struct Presets(Vec<Preset>);

impl Presets {
    /// Drops what cannot be used (a hand-edited state file).
    pub fn new(mut saved: Vec<Preset>) -> Self {
        saved.retain(|p| Preset::NUMBERS.contains(&p.number) && !p.wallpapers.is_empty());
        saved.sort_by_key(|p| p.number);
        saved.dedup_by_key(|p| p.number);
        Self(saved)
    }

    pub fn list(&self) -> &[Preset] {
        &self.0
    }

    pub fn get(&self, number: u8) -> Result<&Preset> {
        self.0
            .iter()
            .find(|p| p.number == number)
            .with_context(|| format!("no preset {number}"))
    }

    /// Adds preset `number` or replaces its wallpapers; `None` keeps its name.
    pub fn save(
        &mut self,
        number: u8,
        name: Option<&str>,
        wallpapers: Vec<SavedWallpaper>,
    ) -> Result<()> {
        anyhow::ensure!(
            Preset::NUMBERS.contains(&number),
            "presets are numbered {} to {}",
            Preset::NUMBERS.start(),
            Preset::NUMBERS.end()
        );
        anyhow::ensure!(!wallpapers.is_empty(), "no monitor shows a wallpaper");
        match self.0.iter_mut().find(|p| p.number == number) {
            Some(preset) => {
                preset.wallpapers = wallpapers;
                if let Some(name) = name {
                    preset.name = tidy(name);
                }
            }
            None => {
                self.0.push(Preset {
                    number,
                    name: tidy(name.unwrap_or_default()),
                    wallpapers,
                });
                self.0.sort_by_key(|p| p.number);
            }
        }
        Ok(())
    }

    pub fn rename(&mut self, number: u8, name: &str) -> Result<()> {
        self.get(number)?;
        for preset in self.0.iter_mut().filter(|p| p.number == number) {
            preset.name = tidy(name);
        }
        Ok(())
    }

    pub fn delete(&mut self, number: u8) -> Result<()> {
        self.get(number)?;
        self.0.retain(|p| p.number != number);
        Ok(())
    }
}

fn tidy(name: &str) -> String {
    name.trim().chars().take(NAME_CHARS).collect()
}

/// `shown` is what `wanted` asks for, so it can stay as it is. A playlist
/// counts as the same whatever it has moved on to.
fn same(shown: &SavedWallpaper, wanted: &SavedWallpaper) -> bool {
    let monitors = |w: &SavedWallpaper| {
        let mut ids = w.outputs.clone();
        ids.sort();
        ids
    };
    let list = |w: &SavedWallpaper| w.playlist.as_ref().map(|r: &SavedRun| r.name.clone());
    monitors(shown) == monitors(wanted)
        && shown.layout == wanted.layout
        && list(shown) == list(wanted)
        && (shown.playlist.is_some()
            || (same_path(&shown.path, &wanted.path)
                && shown.kind == wanted.kind
                && shown.fit == wanted.fit))
}

fn keys(modifiers: Modifiers) -> HotkeyModifiers {
    let (ctrl, alt, shift) = match modifiers {
        Modifiers::CtrlAlt => (true, true, false),
        Modifiers::CtrlShift => (true, false, true),
        Modifiers::AltShift => (false, true, true),
        Modifiers::CtrlAltShift => (true, true, true),
    };
    HotkeyModifiers { ctrl, alt, shift }
}

impl Daemon<'_> {
    /// Registers the hotkeys of the presets there are (only those: each one
    /// taken is one no other program can have).
    pub(crate) fn register_hotkeys(&mut self) {
        let numbers: Vec<u8> = if self.hotkeys.enabled {
            self.presets.list().iter().map(|p| p.number).collect()
        } else {
            Vec::new()
        };
        let modifiers = keys(self.hotkeys.modifiers);
        self.hotkeys_taken = self.platform.set_hotkeys(modifiers, &numbers);
    }

    pub(crate) fn hotkey_status(&self) -> HotkeyStatus {
        HotkeyStatus {
            hotkeys: self.hotkeys,
            supported: self.platform.hotkeys_supported(),
            taken: self.hotkeys_taken.clone(),
        }
    }

    pub(crate) fn on_hotkey(&mut self, number: u8) {
        tracing::info!(number, "preset hotkey");
        if let Err(e) = self.use_preset(number) {
            tracing::warn!("preset {number}: {e:#}");
        }
    }

    pub(crate) fn save_preset(&mut self, number: u8, name: Option<String>) -> Result<()> {
        let shown = self.slots.iter().map(Slot::saved).collect();
        self.presets.save(number, name.as_deref(), shown)?;
        self.register_hotkeys();
        self.persist();
        Ok(())
    }

    pub(crate) fn rename_preset(&mut self, number: u8, name: &str) -> Result<()> {
        self.presets.rename(number, name)?;
        self.persist();
        Ok(())
    }

    pub(crate) fn delete_preset(&mut self, number: u8) -> Result<()> {
        self.presets.delete(number)?;
        self.register_hotkeys();
        self.persist();
        Ok(())
    }

    /// Shows what preset `number` holds. Wallpapers already up where they
    /// belong stay; the others are replaced once the new ones have drawn.
    /// What fails to load is left out; if nothing loads, nothing changes.
    pub(crate) fn use_preset(&mut self, number: u8) -> Result<()> {
        let mut wanted = self.presets.get(number)?.wallpapers.clone();
        for w in &mut wanted {
            // A playlist deleted since: the wallpaper it was at.
            let gone = |r: &SavedRun| !self.playlists.iter().any(|p| p.name == r.name);
            w.playlist = w.playlist.take().filter(|r| !gone(r));
        }
        let claims: Vec<Vec<String>> = wanted.iter().map(|w| w.outputs.clone()).collect();
        let outputs = self.platform.outputs()?;
        anyhow::ensure!(
            slots::assign(&claims, &outputs)
                .iter()
                .any(|group| !group.is_empty()),
            "none of its monitors is connected"
        );

        // Everything new is loaded before anything old goes.
        let shown: Vec<SavedWallpaper> = self.slots.iter().map(Slot::saved).collect();
        let mut kept = vec![false; shown.len()];
        let mut plan: Vec<Result<usize, Box<Slot>>> = Vec::new();
        let mut failed = Vec::new();
        for w in wanted {
            let stays = (0..shown.len()).find(|&i| !kept[i] && same(&shown[i], &w));
            if let Some(i) = stays {
                kept[i] = true;
                plan.push(Ok(i));
                continue;
            }
            match self.saved_slot(w) {
                Ok(slot) => plan.push(Err(Box::new(slot))),
                Err(e) => failed.push(format!("{e:#}")),
            }
        }
        if plan.is_empty() {
            anyhow::bail!("{}", failed.join("; "));
        }

        let mut old: Vec<Option<Slot>> = self.slots.drain(..).map(Some).collect();
        let mut new = Vec::new();
        for step in plan {
            match step {
                Ok(i) => {
                    if let Some(mut slot) = old.get_mut(i).and_then(Option::take) {
                        // It gets another chance if it could not be shown.
                        slot.failures = 0;
                        self.slots.push(slot);
                    }
                }
                Err(mut slot) => {
                    // Taken from the first frame, if it lands on the primary monitor.
                    slot.fallback_pending = self.os_fallback;
                    new.push(self.slots.len());
                    self.slots.push(*slot);
                }
            }
        }
        for mut slot in old.into_iter().flatten() {
            if let Some(session) = slot.session.take() {
                crate::retire(&mut self.retiring, session);
            }
        }
        self.rebuild(false);
        for &i in &new {
            self.slots[i].placed(self.os_fallback, self.volume);
        }
        // New wallpapers that cannot show leave their monitors to the others.
        for &i in new.iter().rev() {
            if let Some(e) = self.slots[i].error.clone() {
                let slot = self.slots.remove(i);
                failed.push(format!("{}: {e}", slot.wallpaper.path.display()));
                self.needs_rebuild = true;
            }
        }
        self.persist();
        anyhow::ensure!(failed.is_empty(), "{}", failed.join("; "));
        Ok(())
    }

    /// A remembered wallpaper, loaded but not yet shown.
    pub(crate) fn saved_slot(&self, saved: SavedWallpaper) -> Result<Slot> {
        let look = Look {
            fit: saved.fit,
            layout: saved.layout,
        };
        let wallpaper = self
            .load(&saved.path, Some(saved.kind), look)
            .with_context(|| saved.path.display().to_string())?;
        let mut slot = Slot::new(wallpaper, saved.outputs, self.fps);
        slot.run = saved.playlist.and_then(|r| self.resume(&r));
        Ok(slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diorama_ipc::{Fit, Layout, MediaKind};

    fn wallpaper(path: &str, outputs: &[&str]) -> SavedWallpaper {
        SavedWallpaper {
            path: path.into(),
            kind: MediaKind::Image,
            fit: Fit::Fill,
            layout: Layout::PerOutput,
            outputs: outputs.iter().map(|s| (*s).to_owned()).collect(),
            playlist: None,
        }
    }

    fn playing(mut w: SavedWallpaper, name: &str, index: usize) -> SavedWallpaper {
        w.playlist = Some(SavedRun {
            name: name.into(),
            index,
        });
        w
    }

    #[test]
    fn presets_are_numbered_and_keep_their_names() {
        let mut p = Presets::default();
        p.save(3, Some(" work "), vec![wallpaper("a", &[])])
            .unwrap();
        p.save(1, None, vec![wallpaper("b", &[])]).unwrap();
        assert_eq!(
            p.list().iter().map(|p| p.number).collect::<Vec<_>>(),
            [1, 3]
        );
        assert_eq!(p.get(3).unwrap().name, "work");
        p.save(3, None, vec![wallpaper("c", &[])]).unwrap();
        assert_eq!(p.get(3).unwrap().name, "work");
        assert_eq!(p.get(3).unwrap().wallpapers[0].path.as_os_str(), "c");
        p.rename(3, &"x".repeat(200)).unwrap();
        assert_eq!(p.get(3).unwrap().name.len(), NAME_CHARS);

        assert!(p.save(0, None, vec![wallpaper("a", &[])]).is_err());
        assert!(p.save(10, None, vec![wallpaper("a", &[])]).is_err());
        assert!(p.save(2, None, Vec::new()).is_err());
        assert!(p.rename(2, "x").is_err());
        p.delete(3).unwrap();
        assert!(p.delete(3).is_err());
        assert!(p.get(3).is_err());
    }

    #[test]
    fn unusable_saved_presets_are_dropped() {
        let preset = |number, wallpapers| Preset {
            number,
            name: String::new(),
            wallpapers,
        };
        let p = Presets::new(vec![
            preset(255, vec![wallpaper("a", &[])]),
            preset(4, vec![wallpaper("b", &[])]),
            preset(4, vec![wallpaper("c", &[])]),
            preset(2, Vec::new()),
        ]);
        assert_eq!(p.list().len(), 1);
        assert_eq!(p.get(4).unwrap().wallpapers[0].path.as_os_str(), "b");
    }

    #[test]
    fn what_already_shows_stays() {
        let a = wallpaper("a", &["M1", "M2"]);
        assert!(same(&a, &wallpaper("a", &["M2", "M1"])));
        assert!(!same(&a, &wallpaper("a", &["M1"])));
        assert!(!same(&a, &wallpaper("b", &["M1", "M2"])));
        let fit = SavedWallpaper {
            fit: Fit::Stretch,
            ..a.clone()
        };
        assert!(!same(&a, &fit));
        // Playlists carry on, whatever they are at; others start.
        let day = playing(a.clone(), "day", 0);
        let moved = playing(wallpaper("z", &["M1", "M2"]), "day", 5);
        assert!(same(&moved, &day));
        assert!(!same(&a, &day));
        assert!(!same(&playing(a, "night", 0), &day));
    }
}
