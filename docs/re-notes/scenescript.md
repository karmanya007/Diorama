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

## Implementation notes for Phase 3
- Runtime: QuickJS (rquickjs) with **ES module** support; one module per script value, evaluated in a per-scene context so `shared` works.
- Load WE's helper JS from the user's install via the VFS mount (do not vendor).
- `update` runs on the sim thread each tick; return values are written back into the scene IR property before extract.
- Enforce memory limit + interrupt-based time budget per tick (security section of the plan).
- Generate an API coverage table from the typings (member list above) to track progress.
