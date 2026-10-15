//! Text layout and glyph rasterization for text layers.
//!
//! The layout rules reproduce Wallpaper Engine's text objects as measured on
//! RenderDoc captures (`docs/re-notes/text.md`):
//! - glyphs are shaped with HarfBuzz rules (kerning) and rasterized per glyph
//!   into a single-channel atlas; each glyph bitmap sits at the rounded pen
//!   position plus its bearing;
//! - line metrics follow FreeType's rounding: ascender rounded up, descender
//!   down, line height = round(ascender − descender + line gap);
//! - a line's width is the right edge of its last glyph bitmap, measured from
//!   the pen start; lines are aligned individually around the anchor;
//! - vertical anchors: top = first line's top (baseline + ascender), bottom =
//!   last baseline − descender, center = half an ascender above the first
//!   baseline (lifted by half the extra rows).
//!
//! Coordinates are pixels relative to the anchor (the text object's origin),
//! y up.

mod atlas;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub use atlas::Atlas;
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::Format;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a usable font file")]
    BadFont,
}

static NEXT_FONT_ID: AtomicU64 = AtomicU64::new(1);

/// A font face loaded from memory (TrueType/OpenType, collection index).
#[derive(Clone)]
pub struct Font {
    id: u64,
    data: Arc<[u8]>,
    index: u32,
    /// Shaping view of the same face (`None` only in tests).
    shaping: Option<harfrust::Font>,
    units_per_em: f32,
    ascender: f32,
    descender: f32,
    line_gap: f32,
}

impl std::fmt::Debug for Font {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Font")
            .field("id", &self.id)
            .field("bytes", &self.data.len())
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}

impl Font {
    pub fn new(data: Arc<[u8]>, index: u32) -> Result<Self, Error> {
        swash::FontRef::from_index(&data, index as usize).ok_or(Error::BadFont)?;
        let shaping = harfrust::Font::new(data.to_vec(), index).ok_or(Error::BadFont)?;
        let metrics = shaping.metrics();
        // FreeType sizes lines from `hhea`, falling back to OS/2 when it is empty.
        let line = metrics
            .hhea_line
            .filter(|l| l.ascender.to_f32() != 0.0 || l.descender.to_f32() != 0.0)
            .or_else(|| metrics.h_line())
            .ok_or(Error::BadFont)?;
        Ok(Self {
            id: NEXT_FONT_ID.fetch_add(1, Ordering::Relaxed),
            units_per_em: f32::from(metrics.units_per_em.max(1)),
            ascender: line.ascender.to_f32(),
            descender: line.descender.to_f32(),
            line_gap: line.line_gap.to_f32(),
            shaping: Some(shaping),
            data,
            index,
        })
    }

    /// Line metrics at `pixel_size` pixels per em, rounded like FreeType.
    pub fn metrics(&self, pixel_size: f32) -> LineMetrics {
        let s = pixel_size / self.units_per_em;
        LineMetrics {
            ascender: (self.ascender * s).ceil(),
            descender: (self.descender * s).floor(),
            line_height: ((self.ascender - self.descender + self.line_gap) * s).round(),
        }
    }
}

/// Rounded pixel metrics of one line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineMetrics {
    /// Baseline to line top (≥ 0).
    pub ascender: f32,
    /// Baseline to the lowest descent (≤ 0).
    pub descender: f32,
    /// Baseline-to-baseline distance.
    pub line_height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HAlign {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VAlign {
    Top,
    #[default]
    Center,
    Bottom,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    /// Em size in pixels.
    pub pixel_size: f32,
    pub halign: HAlign,
    pub valign: VAlign,
    /// Wrap lines wider than this many pixels at spaces.
    pub max_width: Option<f32>,
    /// Keep at most this many lines.
    pub max_rows: Option<usize>,
    /// End a truncated text with "...".
    pub ellipsis: bool,
}

/// One glyph bitmap placed relative to the anchor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad {
    /// Pixel rectangle `[x0, y0, x1, y1]`, y up.
    pub rect: [f32; 4],
    /// Atlas pixel rectangle `[x, y, width, height]`, y down.
    pub atlas: [u32; 4],
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    pub quads: Vec<Quad>,
    /// Text box `[x0, y0, x1, y1]` relative to the anchor, y up: lines'
    /// horizontal extent × rows of line height (render targets add padding).
    pub bounds: [f32; 4],
    pub rows: usize,
}

impl Layout {
    pub fn width(&self) -> f32 {
        self.bounds[2] - self.bounds[0]
    }

    pub fn height(&self) -> f32 {
        self.bounds[3] - self.bounds[1]
    }
}

/// Shapes, rasterizes and lays out text; owns the glyph atlas its layouts
/// refer to.
pub struct TextEngine {
    scale: ScaleContext,
    atlas: Atlas,
}

impl std::fmt::Debug for TextEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextEngine")
            .field("atlas", &self.atlas)
            .finish_non_exhaustive()
    }
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// A glyph after shaping: pen position in pixels from the line start.
#[derive(Debug, Clone, Copy)]
struct Shaped {
    glyph: u16,
    x: f32,
    y: f32,
}

/// A glyph bitmap relative to the pen start of its line, y up.
#[derive(Debug, Clone, Copy)]
struct LineGlyph {
    rect: [f32; 4],
    atlas: [u32; 4],
}

impl TextEngine {
    pub fn new() -> Self {
        Self {
            scale: ScaleContext::new(),
            atlas: Atlas::new(),
        }
    }

    pub fn atlas(&self) -> &Atlas {
        &self.atlas
    }

    pub fn layout(&mut self, font: &Font, text: &str, style: &Style) -> Layout {
        let Some(shaping) = &font.shaping else {
            return Layout::default();
        };
        let shaper = harfrust::ShaperFont::new(shaping);
        let px = style.pixel_size.max(0.0);
        let scale = px / font.units_per_em;
        let metrics = font.metrics(px);
        let shape = |s: &str| shape(&shaper, s, scale);
        let advance = |s: &str| shape(s).1;

        let mut lines: Vec<String> = Vec::new();
        for paragraph in text.split('\n') {
            let paragraph = paragraph.trim_end_matches('\r');
            match style.max_width {
                Some(max) => lines.extend(wrap(paragraph, max, advance)),
                None => lines.push(paragraph.to_owned()),
            }
        }
        if let Some(rows) = style.max_rows.filter(|&r| r > 0)
            && lines.len() > rows
        {
            lines.truncate(rows);
            if style.ellipsis
                && let Some(last) = lines.last_mut()
            {
                *last = ellipsize(last, style.max_width, advance);
            }
        }

        let mut placed = Vec::with_capacity(lines.len());
        for line in &lines {
            let glyphs: Vec<LineGlyph> = shape(line)
                .0
                .into_iter()
                .filter_map(|g| self.place(font, g, px))
                .collect();
            let right = glyphs.iter().map(|g| g.rect[2]).fold(0.0, f32::max);
            let left = glyphs.iter().map(|g| g.rect[0]).fold(0.0, f32::min);
            placed.push((glyphs, left, right));
        }

        let widths: Vec<f32> = placed.iter().map(|(_, _, right)| *right).collect();
        let origins = line_origins(metrics, &widths, style.halign, style.valign);
        let mut layout = Layout {
            rows: placed.len(),
            ..Layout::default()
        };
        let (mut x0, mut x1) = (f32::MAX, f32::MIN);
        for ((glyphs, left, right), [ox, oy]) in placed.iter().zip(&origins) {
            x0 = x0.min(ox + left);
            x1 = x1.max(ox + right);
            layout.quads.extend(glyphs.iter().map(|g| Quad {
                rect: [
                    g.rect[0] + ox,
                    g.rect[1] + oy,
                    g.rect[2] + ox,
                    g.rect[3] + oy,
                ],
                atlas: g.atlas,
            }));
        }
        let first = origins.first().map_or(0.0, |o| o[1]);
        let last = origins.last().map_or(0.0, |o| o[1]);
        layout.bounds = [
            x0,
            last - (metrics.line_height - metrics.ascender),
            x1,
            first + metrics.ascender,
        ];
        layout
    }

    /// Rasterizes (or reuses) one glyph; `None` for blank glyphs (spaces).
    fn place(&mut self, font: &Font, g: Shaped, px: f32) -> Option<LineGlyph> {
        let key = atlas::Key {
            font: font.id,
            glyph: g.glyph,
            size: px.to_bits(),
        };
        let entry = match self.atlas.get(&key) {
            Some(entry) => entry,
            None => {
                let swash_font = swash::FontRef::from_index(&font.data, font.index as usize)?;
                let mut scaler = self.scale.builder(swash_font).size(px).hint(false).build();
                let image = Render::new(&[Source::Outline])
                    .format(Format::Alpha)
                    .render(&mut scaler, g.glyph)?;
                let p = image.placement;
                self.atlas
                    .insert(key, p.left, p.top, p.width, p.height, &image.data)
            }
        };
        if entry.size[0] == 0 || entry.size[1] == 0 {
            return None;
        }
        let x = g.x.round() + entry.bearing[0] as f32;
        let top = g.y.round() + entry.bearing[1] as f32;
        Some(LineGlyph {
            rect: [x, top - entry.size[1] as f32, x + entry.size[0] as f32, top],
            atlas: [entry.pos[0], entry.pos[1], entry.size[0], entry.size[1]],
        })
    }
}

/// Shaped glyphs and the line's total advance, in pixels.
fn shape(shaper: &harfrust::ShaperFont<'_, '_>, text: &str, scale: f32) -> (Vec<Shaped>, f32) {
    let mut buffer = harfrust::Buffer::new();
    buffer.push_str(text);
    buffer.guess_segment_properties();
    if harfrust::shape(shaper, &mut buffer, harfrust::ShapeOptions::new()).is_err() {
        return (Vec::new(), 0.0);
    }
    let mut pen = 0i32;
    let mut glyphs = Vec::with_capacity(buffer.len());
    for (info, pos) in buffer.glyph_infos().iter().zip(buffer.glyph_positions()) {
        glyphs.push(Shaped {
            glyph: u16::try_from(info.glyph_id).unwrap_or(0),
            x: (pen + pos.x_offset) as f32 * scale,
            y: pos.y_offset as f32 * scale,
        });
        pen += pos.x_advance;
    }
    (glyphs, pen as f32 * scale)
}

/// Greedy word wrap at spaces; words wider than `max` stay on their own line.
fn wrap(paragraph: &str, max: f32, width: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in paragraph.split(' ') {
        let candidate = if current.is_empty() {
            word.to_owned()
        } else {
            format!("{current} {word}")
        };
        if !current.is_empty() && width(&candidate) > max {
            lines.push(std::mem::replace(&mut current, word.to_owned()));
        } else {
            current = candidate;
        }
    }
    lines.push(current);
    lines
}

/// `line` + "...", shortened until it fits `max` (if given).
fn ellipsize(line: &str, max: Option<f32>, width: impl Fn(&str) -> f32) -> String {
    let mut chars: Vec<char> = line.trim_end().chars().collect();
    loop {
        let candidate: String = chars.iter().collect::<String>() + "...";
        if chars.is_empty() || max.is_none_or(|m| width(&candidate) <= m) {
            return candidate;
        }
        chars.pop();
    }
}

/// Pen start (x, baseline y) of each line relative to the anchor.
fn line_origins(m: LineMetrics, widths: &[f32], halign: HAlign, valign: VAlign) -> Vec<[f32; 2]> {
    let extra = widths.len().saturating_sub(1) as f32 * m.line_height;
    let first_baseline = match valign {
        VAlign::Top => -m.ascender,
        VAlign::Bottom => extra - m.descender,
        VAlign::Center => -m.ascender * 0.5 + extra * 0.5,
    };
    widths
        .iter()
        .enumerate()
        .map(|(i, &w)| {
            let x = match halign {
                HAlign::Left => 0.0,
                HAlign::Center => -w * 0.5,
                HAlign::Right => -w,
            };
            [x, first_baseline - i as f32 * m.line_height]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Quicksand Bold at 32 pt (133.3 px/em) as rounded by FreeType.
    const QUICKSAND: LineMetrics = LineMetrics {
        ascender: 127.0,
        descender: -29.0,
        line_height: 155.0,
    };

    #[test]
    fn vertical_anchors_match_captures() {
        // Top: the anchor is the line top.
        let top = line_origins(QUICKSAND, &[100.0], HAlign::Left, VAlign::Top);
        assert_eq!(top, vec![[0.0, -127.0]]);
        // Bottom: the anchor is the lowest descent.
        let bottom = line_origins(QUICKSAND, &[100.0], HAlign::Left, VAlign::Bottom);
        assert_eq!(bottom, vec![[0.0, 29.0]]);
        // Center: half an ascender above the baseline (scene 3431125458).
        let center = line_origins(QUICKSAND, &[100.0], HAlign::Center, VAlign::Center);
        assert_eq!(center, vec![[-50.0, -63.5]]);
    }

    #[test]
    fn lines_align_individually() {
        let o = line_origins(QUICKSAND, &[100.0, 40.0], HAlign::Right, VAlign::Top);
        assert_eq!(o, vec![[-100.0, -127.0], [-40.0, -282.0]]);
    }

    #[test]
    fn wraps_at_spaces() {
        let width = |s: &str| s.len() as f32 * 10.0;
        assert_eq!(wrap("aa bb cc", 50.0, width), vec!["aa bb", "cc"]);
        assert_eq!(wrap("toolongword x", 50.0, width), vec!["toolongword", "x"]);
        assert_eq!(wrap("", 50.0, width), vec![""]);
    }

    #[test]
    fn ellipsis_fits_width() {
        let width = |s: &str| s.len() as f32 * 10.0;
        assert_eq!(ellipsize("abcdefgh", Some(60.0), width), "abc...");
        assert_eq!(ellipsize("ab", None, width), "ab...");
    }

    #[test]
    fn metrics_round_like_freetype() {
        // hhea of Quicksand Bold: 950 / −216 / 0 per 1000 units.
        let font = Font {
            id: 0,
            data: Arc::from(Vec::new()),
            index: 0,
            shaping: None,
            units_per_em: 1000.0,
            ascender: 950.0,
            descender: -216.0,
            line_gap: 0.0,
        };
        assert_eq!(font.metrics(32.0 * 300.0 / 72.0), QUICKSAND);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Font::new(Arc::from(vec![0u8; 64]), 0).is_err());
    }
}
