# P7 — Stutter with mixed refresh rates

- **Pain point:** A single vsync-locked loop stutters on 144 Hz + 60 Hz setups.
- **Fix:** Mailbox presentation per output (FIFO fallback); content paced by its own clock.
- **Automated:** unit test `prefers_mailbox_then_fifo`

## Steps
1. Monitors with different refresh rates (here 180 Hz + 144 Hz).
2. `RUST_LOG=info,lumen_gpu=debug lumend --scene <dir> --fps 240`.
3. Check the log shows `present_mode=Mailbox` per surface; watch both monitors.

## Pass when
No stalls or judder on either monitor; CPU/GPU stay modest.

## Last run
2026-10-15: both surfaces Mailbox, `--fps 240` scene at 7% of a core / 5.2% GPU, no stalls.
