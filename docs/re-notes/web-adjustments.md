# WE's own options for web wallpapers

Every web wallpaper in WE gets a few options of WE's own, stored next to its
property choices (`<user>.wproperties[<page>].MonitorN` in `config.json`),
plus a `language` general property. Lumen's implementation: the page script
(`apps/lumen-webhost/src/shim.js`, `adjust`) and `crates/lumen-we/src/language.rs`.

## Method (2026-10-26)

Behaviour of WE 2.8.42's web wallpaper host (`bin/webwallpaper64.exe`), read
for behaviour only, nothing copied; checked against the values WE stored in
this machine's `config.json`.

## Colour filter

Keys: `wec_e` (bool), `wec_con`, `wec_brs`, `wec_sa`, `wec_hue` (numbers;
WE stores e.g. `wec_con` 100, `wec_brs` 14).

- WE keeps the last value of each and updates only keys present as
  numbers (or, for `wec_e`, as a bool).
- Factors: contrast, brightness, saturation = value / 50 (50 → unchanged);
  hue = (value / 50 − 1) · 180 degrees (50 → 0°, range ±180°).
- When anything changed, WE runs in the page:
  `document.documentElement.style.filter = 'contrast(c) brightness(b) saturate(s) hue-rotate(h deg)'`,
  or `= ''` when `wec_e` is off. Off by default (nothing is written until a
  change).

## Horizontal flip

`alignmentfliph` (bool): true → `documentElement.style.transform = 'scaleX(-1)'`
and `overflow-y: hidden`; false → `transform = ''`.

The same keys are stored for scenes (2811044789 has `wec_brs`, 3161563267
`alignmentfliph`), and `wallpaper64.exe` lists them among its video and
display options; how scenes and videos apply them is not decoded.

## `language`

WE's settings hold `general.user.language` (`default` or a choice) and
`steamlanguage` (Steam's client language, e.g. `english`). Before
`applyGeneralProperties`, WE replaces the general properties' `language` with
one of 36 codes from a table pairing codes with Steam language names
(`en-us`/`english`, `de-de`/`german`, `pt-br`/`brazilian`, `ko-kr`/`koreana`,
`zh-chs`/`schinese`, `zh-cht`/`tchinese`, …, `be-by`/`belarusian`); Steam's
`latam` maps to `es-es`. With `default`, Steam's language picks the code;
codes outside the table become `en-us` (index 0). `steamlanguage` itself is
removed before the page sees it.

Lumen sends WE's code when WE's config is available and otherwise maps the
OS display language to the nearest code (`en-GB` → `en-us`, `zh-Hant-TW` →
`zh-cht`, unknown → `en-us`).
