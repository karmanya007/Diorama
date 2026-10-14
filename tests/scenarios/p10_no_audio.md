# P10 — Video fails without an audio device

- **Pain point:** WE logs MF_E_NO_AUDIO_PLAYBACK_DEVICE and video stops.
- **Fix:** Video never opens an audio device; the soundtrack is a separate optional player that degrades to silence.
- **Automated:** no

## Steps
1. Disable all playback devices.
2. `lumend --video <mp4> --volume 50`.

## Pass when
Video plays; log says audio unavailable.

## Last run
2026-10-14: passed; device unplug/default change reopens audio.
