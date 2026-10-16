# SceneScript — RE notes

Sources: `ui/dist/monaco/autocomplete/lib.sceneScript.d.ts` (editor typings, 2571 lines), `assets/scripts/**` (helper JS), corpus `scene.json` files. Used by 32/55 corpus scenes (1017 scripted values).

## How scripts are attached
Any bindable value in `scene.json` can be an object instead of a literal:

```json
"bloomstrength": { "script": "'use strict';\n…\nexport function update() { … }\nexport function init(value) { … }", "value": 1.0 }
"bloomthreshold": { "user": "new_property", "value": 0.7 }
```

- `value` — initial/fallback value.
- `user` — bound to a user property from `project.json` `general.properties` (2276 bindings in corpus).
- `script` — an **ES module** (`'use strict'`, `export function …`). Property scripts export:
  - `init(value)` — called once with the initial value.
  - `update(value)` — called every frame; the **return value becomes the property value**.
  - optional `applyUserProperties(props)`, `cursor*` handlers, `destroy()`, etc. (see `IComponent`).
- Also `scriptproperties` alongside `script` (seen in corpus; exposes per-script editable settings) — TBD.

## Globals
| global | type | notes |
|---|---|---|
| `thisLayer` | `ILayer` | layer owning the script |
| `thisScene` | `IScene` | scene: layers, camera, bloom/parallax/shake settings |
| `engine` | `IEngine` | `registerAudioBuffers(16\|32\|64)`, `frametime`, `runtime`, `timeOfDay`, `userProperties`, `screenResolution`, `canvasSize`, `setTimeout`, `setInterval`, `isWallpaper`/`isScreensaver`/`isRunningInEditor`, `openUserShortcut`, `registerAsset` |
| `input` | `IInput` | `cursorWorldPosition`, `cursorScreenPosition`, `cursorLeftDown` |
| `console` | `IConsole` | `log`, `error` |
| `localStorage` | `ILocalStorage` | `get`/`set`/`delete`/`clear` — per-wallpaper persistence |
| `shared` | `Object` | shared across all scripts in the scene |
| `renderContext` | `IRenderContext` | empty in typings |

## Component callbacks (`IComponent`)
`init`, `update`, `destroy`, `resizeScreen`, `applyUserProperties`, `applyGeneralSettings`, `cursorEnter`/`Leave`/`Move`/`Down`/`Up`/`Click` (`CursorEvent`: `worldPosition`, `localPosition`, `hitBox`), media integration: `mediaStatusChanged`, `mediaPlaybackChanged`, `mediaPropertiesChanged`, `mediaThumbnailChanged`, `mediaTimelineChanged`.

## Object model (members only; see typings for signatures)
- **Math:** `Vec2/3/4` (arithmetic, `dot`, `cross`, `reflect`, `refract`, `mix`, `clamp`, `smoothStep`, …), `Mat3/4` (`translation`, `rotate`, `scale`, `inverse`, `decompose`, `extractEuler`, …). JS implementations ship in `assets/scripts/jsclasses/baseclasses.js` and `jsmodules/{wemath,wevector,wecolor}.js`.
- **ILayer:** `origin`, `angles`, `scale`, `parallaxDepth`, `name`, `visible`, `getTransformMatrix`, `lookAt`, `setParent`/`getParent`/`getChildren`, attachments; plus image/text/particle/model/sound/effect/camera facets below.
- **IImageLayer:** `alpha`, `color`, `alignment`, texture animation, video texture, animation layers, bones (puppet warp physics), blend shapes.
- **ITextLayer:** `text`, `color`, `alpha`, `opaquebackground`, `backgroundcolor`, `pointsize`, `font`, `padding`, alignment, `anchor`, row/width limits.
- **IParticleSystem:** `play`/`pause`/`stop`, `emitParticles`, `instance` (`alpha`, `size`, `count`, `speed`, `lifetime`, `rate`, `colorn`, `controlpoint0..7`).
- **IEffectLayer / IEffect:** `getEffect`, `getMaterial`, `setMaterialProperty`, `executeMaterialFunction`, `visible`.
- **IScene:** `getLayer`, `getLayerByID`, `enumerateLayers`, `createLayer`, `destroyLayer`, `sortLayer`, camera transforms, `createModelData`, and all `general` scene settings (bloom, clear color, parallax, shake, fov…).
- **Media events:** title/artist/album, thumbnail colors (`primaryColor` …), playback state, timeline.

## Findings while implementing (Phase 3)
- Scripts see **angles in degrees** (e.g. `value.z = … * 360`), while `scene.json` stores radians (static values never exceed π).
- `thisLayer` is per module; scripts reach other objects with `thisScene.getLayer(name)` / `enumerateLayers()` and change them directly (377 `getLayer` calls in the corpus).
- `scriptproperties` next to `script` hold the user's values for the module's `scriptProperties` (often bound to user properties); WE applies them with `_Internal.updateScriptProperties(module, json)` from `baseclasses.js`.
- Scripts read scene settings on `thisScene` (`cameraparallax`, `clearenabled`, `camerashake`, …) and create helper layers with `thisScene.createLayer(…)` (visualiser bars, clock shadows).
- Many scripts are larger than 4 KiB (the old string limit); scripts now have their own 1 MiB cap.
- Clocks and dates follow the wall clock (`new Date()`); for golden tests the capture time is the `.rdc` file's modification time.

## Lumen implementation
- `lumen-script`: QuickJS via `rquickjs`; WE's `baseclasses.js` and `WEMath`/`WEVector`/`WEColor` modules loaded from the user's install (never vendored); Lumen's own `prelude.js` provides `thisScene`, layer objects (origin/scale/angles/visible/alpha/color/text/size, parent/children, effects), `engine` (runtime, frametime, timeOfDay, userProperties, canvasSize, timers, silent audio buffers), `input` (cursor), `console`, in-memory `localStorage`, `shared`, and a host-driven `Date`.
- Each script is an ES module prefixed with `const thisLayer = …` on its first line (line numbers in errors stay right). `init`, then `applyUserProperties(engine.userProperties)`; every frame `update(currentValue)`.
- Limits: 128 MiB heap; 2 s budget to load, 20 ms per `update()` call. QuickJS interrupts are uncatchable, so the host calls each `update()` separately: a runaway script stops only itself. A script that throws three times is disabled.
- A frame returns one JSON change set (nodes, layer alpha/colour/text, effect visibility, effect constants); `lumend` turns node changes into layer transforms (`node_world ∘ local`) and calls `SceneRenderer::update_layer`.
- Not yet: cursor/media/audio events (`cursorClick`, `media*`), audio spectrum data (silent), texture animations (stub), runtime-created layers are not drawn, `localStorage` is not persisted.

## Implementation notes for Phase 3
- Runtime: QuickJS (rquickjs) with **ES module** support; one module per script value, evaluated in a per-scene context so `shared` works.
- Load WE's helper JS from the user's install via the VFS mount (do not vendor).
- `update` runs on the sim thread each tick; return values are written back into the scene IR property before extract.
- Enforce memory limit + interrupt-based time budget per tick (security section of the plan).
- Generate an API coverage table from the typings (member list above) to track progress.
