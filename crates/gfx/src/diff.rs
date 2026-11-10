//! Pixel comparison of two RGBA8 frames: mismatch counts, a visual diff
//! image and the bounding boxes of the regions that changed.
//!
//! Used by reftests (Acid2, WPT-style `==` references), comparisons with
//! screenshots from other browsers and the agent interface ("did the page
//! visually change after this click, and where?").

/// A rectangle of changed pixels (pixel coordinates, end-exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Changed pixels inside the rectangle.
    pub pixels: u64,
}

impl Region {
    pub fn right(&self) -> u32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> u32 {
        self.y + self.h
    }
}

/// A borrowed RGBA8 frame (tightly packed rows).
#[derive(Debug, Clone, Copy)]
pub struct FrameRef<'a> {
    pub rgba: &'a [u8],
    pub width: u32,
    pub height: u32,
}

impl<'a> FrameRef<'a> {
    pub fn new(rgba: &'a [u8], width: u32, height: u32) -> FrameRef<'a> {
        assert!(rgba.len() >= (width as usize) * (height as usize) * 4, "frame buffer too small");
        FrameRef { rgba, width, height }
    }

    /// Pixel at (x, y) composited over white (alpha is ignored for opaque
    /// frames), or `None` outside the frame.
    fn px(&self, x: u32, y: u32) -> Option<[u8; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = ((y as usize) * (self.width as usize) + x as usize) * 4;
        let p = &self.rgba[i..i + 4];
        if p[3] == 255 {
            return Some([p[0], p[1], p[2]]);
        }
        let a = p[3] as u32;
        let over = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        Some([over(p[0]), over(p[1]), over(p[2])])
    }
}

/// Decode an encoded image (PNG, JPEG, ...) into straight RGBA8.
pub fn decode_rgba(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("cannot decode image: {e}"))?.to_rgba8();
    let (w, h) = img.dimensions();
    Ok((img.into_raw(), w, h))
}

/// Encode RGBA8 pixels as PNG.
pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::write_buffer_with_format(&mut out, rgba, width, height, image::ExtendedColorType::Rgba8, image::ImageFormat::Png)
        .map_err(|e| format!("PNG encoding failed: {e}"))?;
    Ok(out.into_inner())
}

/// Comparison options.
#[derive(Debug, Clone, Copy)]
pub struct DiffOptions {
    /// Largest per-channel difference still counted as equal (0 = exact).
    pub tolerance: u8,
    /// Changed pixels closer than this many pixels are merged into one
    /// region.
    pub merge_distance: u32,
}

impl Default for DiffOptions {
    fn default() -> Self {
        DiffOptions { tolerance: 0, merge_distance: 8 }
    }
}

/// Result of comparing two frames.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffResult {
    /// Compared area (the union of both frame sizes).
    pub width: u32,
    pub height: u32,
    /// Pixels that differ (pixels present in only one frame count).
    pub mismatched: u64,
    pub total: u64,
    /// Largest per-channel difference seen.
    pub max_delta: u8,
    /// Bounding boxes of the changed areas.
    pub regions: Vec<Region>,
}

impl DiffResult {
    /// Percentage of mismatched pixels (0..=100).
    pub fn percent(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.mismatched as f64 * 100.0 / self.total as f64
        }
    }

    pub fn identical(&self) -> bool {
        self.mismatched == 0
    }
}

/// Per-pixel mismatch mask over the union of both frames.
fn mask(a: FrameRef, b: FrameRef, tolerance: u8) -> (u32, u32, Vec<bool>, u64, u8) {
    let w = a.width.max(b.width);
    let h = a.height.max(b.height);
    let mut m = vec![false; (w as usize) * (h as usize)];
    let mut count = 0u64;
    let mut max_delta = 0u8;
    for y in 0..h {
        for x in 0..w {
            let diff = match (a.px(x, y), b.px(x, y)) {
                (Some(p), Some(q)) => {
                    let d = p.iter().zip(q.iter()).map(|(u, v)| u.abs_diff(*v)).max().unwrap_or(0);
                    max_delta = max_delta.max(d);
                    d > tolerance
                }
                _ => {
                    max_delta = 255;
                    true
                }
            };
            if diff {
                m[(y as usize) * (w as usize) + x as usize] = true;
                count += 1;
            }
        }
    }
    (w, h, m, count, max_delta)
}

/// Compare two frames.
pub fn diff(a: FrameRef, b: FrameRef, opts: &DiffOptions) -> DiffResult {
    let (w, h, m, mismatched, max_delta) = mask(a, b, opts.tolerance);
    let regions = if mismatched == 0 { Vec::new() } else { regions_of(&m, w, h, opts.merge_distance) };
    DiffResult { width: w, height: h, mismatched, total: (w as u64) * (h as u64), max_delta, regions }
}

/// Bounding boxes of the areas that differ between two frames (e.g. the
/// page before and after a click). Empty when nothing changed.
pub fn changed_regions(a: FrameRef, b: FrameRef, opts: &DiffOptions) -> Vec<Region> {
    diff(a, b, opts).regions
}

/// Render a visual diff: unchanged pixels as a faded grayscale of `a`,
/// differing pixels in solid red, region outlines in magenta. RGBA8 of the
/// union size.
pub fn diff_image(a: FrameRef, b: FrameRef, opts: &DiffOptions) -> (DiffResult, Vec<u8>) {
    let (w, h, m, mismatched, max_delta) = mask(a, b, opts.tolerance);
    let regions = if mismatched == 0 { Vec::new() } else { regions_of(&m, w, h, opts.merge_distance) };
    let mut out = vec![255u8; (w as usize) * (h as usize) * 4];
    for y in 0..h {
        for x in 0..w {
            let i = (y as usize) * (w as usize) + x as usize;
            let o = &mut out[i * 4..i * 4 + 4];
            if m[i] {
                o.copy_from_slice(&[255, 0, 0, 255]);
            } else if let Some(p) = a.px(x, y).or_else(|| b.px(x, y)) {
                let l = (p[0] as u32 * 30 + p[1] as u32 * 59 + p[2] as u32 * 11) / 100;
                let v = (191 + l / 4) as u8;
                o.copy_from_slice(&[v, v, v, 255]);
            }
        }
    }
    for r in &regions {
        let outline = |out: &mut Vec<u8>, x: u32, y: u32| {
            if x < w && y < h {
                let i = ((y as usize) * (w as usize) + x as usize) * 4;
                if !m[i / 4] {
                    out[i..i + 4].copy_from_slice(&[255, 0, 255, 255]);
                }
            }
        };
        let (x0, y0) = (r.x.saturating_sub(1), r.y.saturating_sub(1));
        let (x1, y1) = (r.right(), r.bottom());
        for x in x0..=x1 {
            outline(&mut out, x, y0);
            outline(&mut out, x, y1);
        }
        for y in y0..=y1 {
            outline(&mut out, x0, y);
            outline(&mut out, x1, y);
        }
    }
    (DiffResult { width: w, height: h, mismatched, total: (w as u64) * (h as u64), max_delta, regions }, out)
}

/// Group changed pixels into rectangles: pixels are binned into tiles of
/// `merge` px, 8-connected tiles form one region whose box is then
/// tightened to its changed pixels; overlapping boxes are merged.
fn regions_of(m: &[bool], w: u32, h: u32, merge: u32) -> Vec<Region> {
    let tile = merge.max(1);
    let tw = w.div_ceil(tile);
    let th = h.div_ceil(tile);
    let mut tiles = vec![false; (tw as usize) * (th as usize)];
    for y in 0..h {
        for x in 0..w {
            if m[(y as usize) * (w as usize) + x as usize] {
                tiles[((y / tile) as usize) * (tw as usize) + (x / tile) as usize] = true;
            }
        }
    }
    let mut seen = vec![false; tiles.len()];
    let mut out: Vec<Region> = Vec::new();
    let mut stack = Vec::new();
    for start in 0..tiles.len() {
        if !tiles[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let (mut tx0, mut ty0, mut tx1, mut ty1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        while let Some(t) = stack.pop() {
            let (tx, ty) = ((t % tw as usize) as u32, (t / tw as usize) as u32);
            tx0 = tx0.min(tx);
            ty0 = ty0.min(ty);
            tx1 = tx1.max(tx);
            ty1 = ty1.max(ty);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (tx as i64 + dx, ty as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= tw as i64 || ny >= th as i64 {
                        continue;
                    }
                    let n = (ny as usize) * (tw as usize) + nx as usize;
                    if tiles[n] && !seen[n] {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
        // Tighten to the changed pixels inside the tile block.
        let (px0, py0) = (tx0 * tile, ty0 * tile);
        let (px1, py1) = (((tx1 + 1) * tile).min(w), ((ty1 + 1) * tile).min(h));
        let (mut x0, mut y0, mut x1, mut y1, mut n) = (u32::MAX, u32::MAX, 0u32, 0u32, 0u64);
        for y in py0..py1 {
            for x in px0..px1 {
                if m[(y as usize) * (w as usize) + x as usize] {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                    n += 1;
                }
            }
        }
        if n > 0 {
            out.push(Region { x: x0, y: y0, w: x1 - x0, h: y1 - y0, pixels: n });
        }
    }
    // Merge overlapping boxes (tightening can make neighbours overlap).
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for i in 0..out.len() {
            for j in i + 1..out.len() {
                let (a, b) = (out[i], out[j]);
                if a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom() {
                    let x = a.x.min(b.x);
                    let y = a.y.min(b.y);
                    out[i] = Region { x, y, w: a.right().max(b.right()) - x, h: a.bottom().max(b.bottom()) - y, pixels: a.pixels + b.pixels };
                    out.remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
    out.sort_by_key(|r| (r.y, r.x));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let p = f(x, y);
                v.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
        v
    }

    #[test]
    fn identical_frames() {
        let a = frame(10, 10, |_, _| [1, 2, 3]);
        let r = diff(FrameRef::new(&a, 10, 10), FrameRef::new(&a, 10, 10), &DiffOptions::default());
        assert!(r.identical());
        assert_eq!(r.percent(), 0.0);
        assert!(r.regions.is_empty());
    }

    #[test]
    fn counts_and_regions() {
        let a = frame(100, 50, |_, _| [255, 255, 255]);
        let b = frame(100, 50, |x, y| if (10..20).contains(&x) && (5..8).contains(&y) || (80..82).contains(&x) && (40..50).contains(&y) { [0, 0, 0] } else { [255, 255, 255] });
        let r = diff(FrameRef::new(&a, 100, 50), FrameRef::new(&b, 100, 50), &DiffOptions::default());
        assert_eq!(r.mismatched, 30 + 20);
        assert!((r.percent() - 1.0).abs() < 1e-9);
        assert_eq!(r.regions, vec![Region { x: 10, y: 5, w: 10, h: 3, pixels: 30 }, Region { x: 80, y: 40, w: 2, h: 10, pixels: 20 }]);
        let (_, img) = diff_image(FrameRef::new(&a, 100, 50), FrameRef::new(&b, 100, 50), &DiffOptions::default());
        assert_eq!(&img[(5 * 100 + 10) * 4..(5 * 100 + 10) * 4 + 4], &[255, 0, 0, 255]);
    }

    #[test]
    fn tolerance_and_size_mismatch() {
        let a = frame(4, 4, |_, _| [100, 100, 100]);
        let b = frame(4, 4, |_, _| [102, 100, 99]);
        assert_eq!(diff(FrameRef::new(&a, 4, 4), FrameRef::new(&b, 4, 4), &DiffOptions { tolerance: 2, ..Default::default() }).mismatched, 0);
        assert_eq!(diff(FrameRef::new(&a, 4, 4), FrameRef::new(&b, 4, 4), &DiffOptions::default()).mismatched, 16);
        let c = frame(4, 5, |_, _| [100, 100, 100]);
        let r = diff(FrameRef::new(&a, 4, 4), FrameRef::new(&c, 4, 5), &DiffOptions::default());
        assert_eq!((r.width, r.height, r.mismatched), (4, 5, 4));
    }

    #[test]
    fn nearby_changes_merge_into_one_region() {
        let a = frame(40, 40, |_, _| [0, 0, 0]);
        let b = frame(40, 40, |x, y| if (x == 3 && y == 3) || (x == 9 && y == 4) { [9, 9, 9] } else { [0, 0, 0] });
        let r = changed_regions(FrameRef::new(&a, 40, 40), FrameRef::new(&b, 40, 40), &DiffOptions::default());
        assert_eq!(r, vec![Region { x: 3, y: 3, w: 7, h: 2, pixels: 2 }]);
    }
}
