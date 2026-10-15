# P11 — GPU device lost / sleep

- **Pain point:** Black screen after driver reset or sleep.
- **Fix:** Device-lost callback or any failed frame → drop the session and the old device → GPU re-init (retried every second; waits up to 10 s for the same adapter before accepting another) → session rebuild; scenes are re-imported.
- **Automated:** no

## Steps
1. Run a scene.
2. Force a TDR (`dxcap -forcetdr`, admin) or update/restart the driver.
3. Sleep and resume.

## Pass when
Wallpaper returns within seconds in both cases.

## Last run
2026-10-15: first run failed — the device-lost callback fired mid-frame and the failed present (`surface lost and could not be reconfigured`) ended the daemon. Fixed (frame errors trigger a GPU reset; re-init failures retry). Rerun passed: wallpaper back 1.6 s after `dxcap -forcetdr`; sleep/resume also passed. That run re-created the device on the integrated GPU. Cause: the lost device was still alive during re-init, and D3D12 returns the existing (removed) device for an adapter, so the RTX 4060 looked unusable. The old device is now dropped first; rerun: back on the RTX 4060, wallpaper shown 2.2 s after the reset.
