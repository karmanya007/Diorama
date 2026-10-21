# Audio spectrum (`g_AudioSpectrum*`, script audio buffers)

How Wallpaper Engine turns what the system plays into band levels, as far as
captures show. Lumen's implementation: `crates/lumen-media/src/spectrum.rs`.

## Method (2026-10-21)

- 30 RenderDoc captures of 2176097362 while music played (`REFCAP_FRAMES=30`,
  0.3 s apart; `capture.py` now records `trigger_ms`/`done_ms` per frame).
- The default output's loopback audio recorded alongside with millisecond
  timestamps (`capture_probe.rs`, `record_pcm`), deleted after the fit.
- WE's per-frame `g_AudioSpectrum16Left/Right` read from the captures (the
  value most draws share; two draws per frame carry stale garbage), then
  candidate analyses scored against them offline, and the Rust analyzer
  replayed over the same recording (`replay_against_we`).

## Findings

- WE's levels do not follow playback volume. The music was quiet (−40 dBFS
  RMS, peaks −23 dBFS, about 30 dB below mastered level), yet WE's 16 bands
  averaged 0.42 and reached 1.0. A fixed dB scale (Lumen's old −60..0 dB)
  gave a mean of 0.05 for the same audio. WE normalises automatically.
- The upper bands are nearly as busy as the bass (band 15 mean 0.18,
  bands 9–14 0.30–0.55), far more than music's natural treble roll-off.
- Values are near-instantaneous: averaging or peak-holding the analysis
  over 100–400 ms before the frame made the fit worse.
- WE's config has `audioinputvolume` (50) and `audioinputthreshold` (0) in
  its general settings, probably a gain and gate on top.

Best simple model (mean error 0.21, mean per-band correlation 0.45, mean
level 0.39 vs WE 0.42; the old analyzer: 0.38, 0.18, 0.05):

1. 2048-point Hann FFT of the newest samples per channel.
2. 64 bands spaced logarithmically from 50 Hz to 12 kHz, mean magnitude per
   band (50 Hz–12 kHz fitted better than 30 Hz–16 kHz or linear layouts).
3. +3 dB per octave around 1 kHz.
4. One automatic gain per channel: levels read against the channel's
   running peak over 60 dB, the peak falling 2 dB/s (floor −100 dB).
5. Instant rise, exponential fall (6/s) of the displayed value.

A gain per band fitted worse (0.23) and lights every band of a steady tone
at full scale. The remaining error (correlation 0.45) is partly timing (the
captured frame's moment is known to about ±50 ms) and partly WE's exact
algorithm, which is not recovered: `AudioProcessor` in `wallpaper64.exe`
(WASAPI callbacks) is where to look.
