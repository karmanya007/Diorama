# P11 — GPU device lost / sleep

- **Pain point:** Black screen after driver reset or sleep.
- **Fix:** Device-lost callback → full GPU + session rebuild; scenes are re-imported.
- **Automated:** no

## Steps
1. Run a scene.
2. Force a TDR (`dxcap -forcetdr`, admin) or update/restart the driver.
3. Sleep and resume.

## Pass when
Wallpaper returns within seconds in both cases.

## Last run
Pending: needs a supervised run (forced TDR requires admin).
