//! Scene sound objects: one player per sound, at the daemon's volume times
//! the sound's own, paused with the scene, started and stopped by scripts.

use lumen_media::{AudioPlayer, MediaSource, PlaybackMode};
use lumen_scene::{Scene, Sound, SoundMode};
use lumen_script::SoundCommand;

pub struct SceneSounds {
    sounds: Vec<SoundSlot>,
    /// Daemon volume 0..1; 0 opens no audio device at all.
    master: f32,
    paused: bool,
}

struct SoundSlot {
    sound: Sound,
    /// Volume set by scripts (starts as the sound's own).
    volume: f32,
    /// Should be playing (start state, then scripts).
    wanted: bool,
    player: Option<AudioPlayer>,
}

impl SceneSounds {
    /// `None` when the scene has no sounds.
    pub fn new(scene: &Scene) -> Option<Self> {
        let sounds: Vec<SoundSlot> = scene
            .sounds
            .iter()
            .map(|s| SoundSlot {
                volume: s.volume,
                wanted: !s.start_silent,
                sound: s.clone(),
                player: None,
            })
            .collect();
        (!sounds.is_empty()).then_some(Self {
            sounds,
            master: 0.0,
            paused: false,
        })
    }

    /// Sets the daemon volume (0..1): 0 closes every player.
    pub fn set_master(&mut self, master: f32) {
        self.master = master.clamp(0.0, 1.0);
        for i in 0..self.sounds.len() {
            self.sync(i);
        }
    }

    /// Pauses all sounds with the scene.
    pub fn set_paused(&mut self, paused: bool) {
        if paused != self.paused {
            self.paused = paused;
            for i in 0..self.sounds.len() {
                self.sync(i);
            }
        }
    }

    /// Reopens players whose device went away or changed.
    pub fn check_devices(&mut self) {
        for i in 0..self.sounds.len() {
            if self.sounds[i]
                .player
                .as_ref()
                .is_some_and(AudioPlayer::needs_reopen)
            {
                tracing::info!(sound = %self.sounds[i].sound.name, "audio device changed; reopening");
                self.sounds[i].player = None;
                self.sync(i);
            }
        }
    }

    /// Whether each sound is playing, for scripts (`isPlaying()`); without
    /// players (volume 0) what scripts asked for.
    pub fn playing(&self) -> Vec<bool> {
        self.sounds
            .iter()
            .map(|s| s.player.as_ref().map_or(s.wanted, AudioPlayer::is_playing))
            .collect()
    }

    pub fn command(&mut self, command: SoundCommand) {
        let i = match command {
            SoundCommand::Play(i)
            | SoundCommand::Pause(i)
            | SoundCommand::Stop(i)
            | SoundCommand::Volume(i, _) => i,
        };
        let Some(slot) = self.sounds.get_mut(i) else {
            return;
        };
        match command {
            SoundCommand::Play(_) => {
                slot.wanted = true;
                // A finished (single) list plays again from the start.
                if let Some(p) = &slot.player
                    && !p.is_playing()
                {
                    p.set_paused(false);
                    if !p.is_playing() {
                        p.restart();
                    }
                }
            }
            SoundCommand::Pause(_) => slot.wanted = false,
            SoundCommand::Stop(_) => {
                slot.wanted = false;
                if let Some(p) = &slot.player {
                    p.restart();
                }
            }
            SoundCommand::Volume(_, v) => {
                slot.volume = v.clamp(0.0, 1.0);
                if let Some(p) = &slot.player {
                    p.set_volume(self.master * slot.volume);
                }
            }
        }
        self.sync(i);
    }

    /// Brings sound `i`'s player in line with master volume, pause and intent.
    fn sync(&mut self, i: usize) {
        let (master, paused) = (self.master, self.paused);
        let slot = &mut self.sounds[i];
        if master <= 0.0 {
            slot.player = None;
            return;
        }
        if slot.player.is_none() && slot.wanted {
            slot.player = spawn(&slot.sound, master * slot.volume);
        }
        if let Some(p) = &slot.player {
            p.set_volume(master * slot.volume);
            p.set_paused(paused || !slot.wanted);
        }
    }
}

fn spawn(sound: &Sound, volume: f32) -> Option<AudioPlayer> {
    let tracks = sound
        .tracks
        .iter()
        .map(|(name, data)| MediaSource::Memory {
            data: data.clone(),
            name: name.clone(),
        })
        .collect();
    let mode = match sound.mode {
        SoundMode::Loop => PlaybackMode::Loop,
        SoundMode::Once => PlaybackMode::Once,
        SoundMode::Random { min_gap, max_gap } => PlaybackMode::Random { min_gap, max_gap },
    };
    match AudioPlayer::spawn_playlist(tracks, mode, volume) {
        Ok(p) => {
            tracing::info!(sound = %sound.name, volume, "sound playing");
            Some(p)
        }
        Err(e) => {
            tracing::info!(sound = %sound.name, "sound unavailable: {e}");
            None
        }
    }
}
