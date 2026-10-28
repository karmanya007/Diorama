//! Single-channel glyph atlas (like WE's `R8_UNORM` font textures): glyph
//! bitmaps packed in shelves, growing by doubling when full.

use std::collections::HashMap;

const INITIAL: u32 = 256;
const MAX: u32 = 8192;
/// Empty pixels between glyphs so bilinear sampling never bleeds.
const GAP: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    pub font: u64,
    pub glyph: u16,
    /// Pixel size as `f32` bits.
    pub size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Entry {
    pub pos: [u32; 2],
    pub size: [u32; 2],
    /// Bitmap left / top relative to the pen (y up).
    pub bearing: [i32; 2],
}

#[derive(Debug, Clone, Copy)]
struct Shelf {
    y: u32,
    height: u32,
    x: u32,
}

#[derive(Clone)]
pub struct Atlas {
    size: u32,
    pixels: Vec<u8>,
    shelves: Vec<Shelf>,
    next_y: u32,
    entries: HashMap<Key, Entry>,
    generation: u64,
}

impl std::fmt::Debug for Atlas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Atlas")
            .field("size", &self.size)
            .field("glyphs", &self.entries.len())
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl Atlas {
    pub(crate) fn new() -> Self {
        Self {
            size: INITIAL,
            pixels: vec![0; (INITIAL * INITIAL) as usize],
            shelves: Vec::new(),
            next_y: 0,
            entries: HashMap::new(),
            generation: 0,
        }
    }

    /// Width and height in pixels (square).
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Coverage, one byte per pixel, rows top to bottom.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Changes whenever pixels or the size change.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn get(&self, key: &Key) -> Option<Entry> {
        self.entries.get(key).copied()
    }

    /// Stores a glyph bitmap (`width × height` coverage bytes). Blank and
    /// unplaceable glyphs are remembered with an empty size.
    pub(crate) fn insert(
        &mut self,
        key: Key,
        left: i32,
        top: i32,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> Entry {
        let mut entry = Entry {
            pos: [0, 0],
            size: [0, 0],
            bearing: [left, top],
        };
        let valid = width > 0 && height > 0 && data.len() >= (width * height) as usize;
        if valid && let Some(pos) = self.allocate(width, height) {
            for row in 0..height {
                let src = (row * width) as usize;
                let dst = ((pos[1] + row) * self.size + pos[0]) as usize;
                self.pixels[dst..dst + width as usize]
                    .copy_from_slice(&data[src..src + width as usize]);
            }
            entry.pos = pos;
            entry.size = [width, height];
            self.generation += 1;
        }
        self.entries.insert(key, entry);
        entry
    }

    fn allocate(&mut self, width: u32, height: u32) -> Option<[u32; 2]> {
        loop {
            let size = self.size;
            if let Some(shelf) = self
                .shelves
                .iter_mut()
                .find(|s| s.height >= height && s.x + width <= size)
            {
                let pos = [shelf.x, shelf.y];
                shelf.x += width + GAP;
                return Some(pos);
            }
            if self.next_y + height <= size && width <= size {
                self.shelves.push(Shelf {
                    y: self.next_y,
                    height,
                    x: width + GAP,
                });
                let pos = [0, self.next_y];
                self.next_y += height + GAP;
                return Some(pos);
            }
            if size >= MAX {
                return None;
            }
            self.grow();
        }
    }

    /// Doubles the atlas; existing glyphs keep their pixel positions.
    fn grow(&mut self) {
        let old = self.size;
        let new = old * 2;
        let mut pixels = vec![0; (new * new) as usize];
        for row in 0..old {
            let src = (row * old) as usize;
            let dst = (row * new) as usize;
            pixels[dst..dst + old as usize].copy_from_slice(&self.pixels[src..src + old as usize]);
        }
        self.pixels = pixels;
        self.size = new;
        self.generation += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(glyph: u16) -> Key {
        Key {
            font: 1,
            glyph,
            size: 0,
        }
    }

    #[test]
    fn packs_and_keeps_positions_when_growing() {
        let mut atlas = Atlas::new();
        let bitmap = vec![255u8; 100 * 100];
        let first = atlas.insert(key(0), 1, 90, 100, 100, &bitmap);
        assert_eq!(first.pos, [0, 0]);
        let second = atlas.insert(key(1), 0, 0, 100, 100, &bitmap);
        assert_eq!(second.pos, [101, 0]);
        // Third does not fit in a 256 row next to the others, nor below → grow.
        for g in 2..8 {
            atlas.insert(key(g), 0, 0, 100, 100, &bitmap);
        }
        assert!(atlas.size() > 256);
        assert_eq!(atlas.get(&key(0)), Some(first));
        assert_eq!(atlas.pixels()[0], 255);
        assert_eq!(atlas.pixels()[(atlas.size() * 99 + 99) as usize], 255);
    }

    #[test]
    fn blank_glyphs_have_no_pixels() {
        let mut atlas = Atlas::new();
        let generation = atlas.generation();
        let e = atlas.insert(key(3), 0, 0, 0, 0, &[]);
        assert_eq!(e.size, [0, 0]);
        assert_eq!(atlas.generation(), generation);
        assert_eq!(atlas.get(&key(3)), Some(e));
    }
}
