# refcap — RenderDoc reference captures of Wallpaper Engine

Scripts run inside RenderDoc's embedded Python (`qrenderdoc.exe --python <script>`).
Outputs contain Workshop content: write them to the git-ignored `scratch/` folder only.

## Capture
1. Back up `<WE install>/config.json` (switching wallpapers changes your selection).
2. Close Wallpaper Engine.
3. Run (PowerShell):
   ```powershell
   $env:REFCAP_WE_DIR  = "C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine"
   $env:REFCAP_OUT_DIR = "$PWD\scratch\captures"
   $env:REFCAP_SCENES  = "<...>\431960\1132505365\project.json;<...>\431960\2134765860\project.json"
   & "C:\Program Files\RenderDoc\qrenderdoc.exe" --python tools\refcap\capture.py
   ```
   `capture.py` launches `wallpaper64.exe` injected, switches scenes with `-control openWallpaper`, captures one frame each, writes `manifest.json` and `capture.log`.
4. Close the injected WE, restore `config.json`, start WE normally.

## Analyze
```powershell
& "C:\Program Files\RenderDoc\qrenderdoc.exe" --python tools\refcap\analyze.py
```
Writes `<capture>.json` (draws, render targets, formats, blend state, cbuffer values) and `<capture>.final.png` per capture. Findings are summarized in `docs/re-notes/renderdoc-captures.md`.

Text layers: `$env:REFCAP_CAPTURES = "<a.rdc>;<b.rdc>"` then
`qrenderdoc.exe --python toolsefcap	extdraws.py` writes `<capture>.text.json`
(font draws with post-VS glyph quads, and every draw reading a text render target)
and `<capture>.atlas_<id>.png`. See `docs/re-notes/text.md`.
