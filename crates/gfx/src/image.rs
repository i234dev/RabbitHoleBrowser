//! Decoded raster images (RGBA8), including animations (GIF, APNG,
//! animated WebP), AVIF (pure-Rust AV1 via `rav1d`), ICC colour
//! management (via `moxcms`) and decode-time downscaling of huge images.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Image {
    /// Pixel dimensions of `rgba` (may be smaller than the natural size
    /// when a huge image was downscaled at decode time).
    pub width: u32,
    pub height: u32,
    /// Straight (non-premultiplied) RGBA, row-major (first frame of an
    /// animation; use [`Image::pixels`] for the current frame).
    pub rgba: Vec<u8>,
    /// Intrinsic size in CSS pixels (what layout and `naturalWidth` see).
    pub natural_width: u32,
    pub natural_height: u32,
    /// Frames of an animated image (GIF / APNG / animated WebP).
    pub anim: Option<Arc<Animation>>,
    /// Placeholder for a resource that failed to load or decode (layout
    /// shows the broken-image icon / alt text instead).
    pub broken: bool,
}

/// Frames of an animated image plus its playback position. The current
/// frame is shared by every holder of the image, so the engine can advance
/// it without rebuilding layout or display lists.
#[derive(Debug)]
pub struct Animation {
    pub frames: Vec<AnimFrame>,
    /// Number of full plays (0 = forever).
    pub loop_count: u32,
    current: AtomicUsize,
    started: Mutex<Option<Instant>>,
}

#[derive(Debug, Clone)]
pub struct AnimFrame {
    pub rgba: Vec<u8>,
    pub delay_ms: u32,
}

/// Limits applied while decoding.
#[derive(Debug, Clone, Copy)]
pub struct DecodeOptions {
    /// Downscale images with more pixels than this (natural size kept).
    pub max_pixels: u64,
    /// Upper bound for all frames of an animation together (bytes).
    pub max_animation_bytes: usize,
    /// Apply embedded ICC profiles (convert to sRGB).
    pub color_manage: bool,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        DecodeOptions { max_pixels: 4096 * 4096, max_animation_bytes: 96 * 1024 * 1024, color_manage: true }
    }
}

/// Container format sniffed from the first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    WebP,
    Bmp,
    Ico,
    Avif,
    Svg,
    Unknown,
}

/// Sniff an image format from its magic bytes (MIME sniffing).
pub fn sniff(bytes: &[u8]) -> Format {
    let b = bytes;
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Format::Png
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Format::Jpeg
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Format::Gif
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Format::WebP
    } else if b.starts_with(b"BM") {
        Format::Bmp
    } else if b.starts_with(&[0, 0, 1, 0]) || b.starts_with(&[0, 0, 2, 0]) {
        Format::Ico
    } else if b.len() >= 12 && &b[4..8] == b"ftyp" && b[8..b.len().min(64)].windows(4).any(|w| w == b"avif" || w == b"avis") {
        Format::Avif
    } else {
        let head = &b[..b.len().min(1024)];
        let s = String::from_utf8_lossy(head);
        if s.contains("<svg") {
            Format::Svg
        } else {
            Format::Unknown
        }
    }
}

/// Image MIME types we can decode (for `<source type>` / `image-set(type())`).
pub fn supports_mime(mime: &str) -> bool {
    let m = mime.trim().to_ascii_lowercase();
    let m = m.split(';').next().unwrap_or("").trim();
    matches!(
        m,
        "image/png"
            | "image/apng"
            | "image/jpeg"
            | "image/jpg"
            | "image/pjpeg"
            | "image/gif"
            | "image/webp"
            | "image/bmp"
            | "image/x-icon"
            | "image/vnd.microsoft.icon"
            | "image/avif"
            | "image/svg+xml"
    )
}

impl Image {
    /// Decode PNG/APNG/JPEG/GIF/WebP/BMP/ICO/AVIF with default limits.
    /// Returns None on failure or for unsupported formats (e.g. SVG).
    pub fn decode(bytes: &[u8]) -> Option<Image> {
        Image::decode_with(bytes, &DecodeOptions::default())
    }

    pub fn decode_with(bytes: &[u8], opts: &DecodeOptions) -> Option<Image> {
        let fmt = sniff(bytes);
        let img = match fmt {
            Format::Svg => return None,
            Format::Avif => crate::avif::decode(bytes)?,
            Format::Gif | Format::Png | Format::WebP => match decode_animated(bytes, fmt, opts) {
                Some(Ok(img)) => img,
                Some(Err(())) => return None,
                None => decode_still(bytes, opts)?,
            },
            _ => decode_still(bytes, opts)?,
        };
        if img.width == 0 || img.height == 0 {
            return None;
        }
        Some(img.limit_pixels(opts.max_pixels))
    }

    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Image {
        debug_assert_eq!(rgba.len(), (width * height * 4) as usize);
        Image { width, height, rgba, natural_width: width, natural_height: height, anim: None, broken: false }
    }

    /// Marker image for a failed load (1×1 transparent, natural size 0).
    pub fn broken() -> Image {
        let mut i = Image::new(1, 1, vec![0; 4]);
        i.natural_width = 0;
        i.natural_height = 0;
        i.broken = true;
        i
    }

    /// Same pixels with a different intrinsic (CSS) size, e.g. a 2x
    /// rasterization of an SVG or a `srcset` candidate with density 2.
    pub fn with_natural_size(mut self, w: u32, h: u32) -> Image {
        self.natural_width = w.max(1);
        self.natural_height = h.max(1);
        self
    }

    /// Intrinsic size in CSS pixels.
    pub fn natural_size(&self) -> (f32, f32) {
        (self.natural_width as f32, self.natural_height as f32)
    }

    /// Pixels of the frame currently shown.
    #[inline]
    pub fn pixels(&self) -> &[u8] {
        match &self.anim {
            Some(a) => {
                let i = a.current.load(Ordering::Relaxed);
                a.frames.get(i).map(|f| f.rgba.as_slice()).unwrap_or(&self.rgba)
            }
            None => &self.rgba,
        }
    }

    pub fn is_animated(&self) -> bool {
        self.anim.as_ref().map(|a| a.frames.len() > 1).unwrap_or(false)
    }

    pub fn current_frame(&self) -> usize {
        self.anim.as_ref().map(|a| a.current.load(Ordering::Relaxed)).unwrap_or(0)
    }

    /// Show frame `i` (no-op for still images).
    pub fn set_frame(&self, i: usize) {
        if let Some(a) = &self.anim {
            a.current.store(i.min(a.frames.len().saturating_sub(1)), Ordering::Relaxed);
        }
    }

    /// Advance an animation to the frame due at `now` (the clock starts the
    /// first time this is called). Returns (frame changed, time until the
    /// next frame change; None when the animation finished or is still).
    pub fn update_animation(&self, now: Instant) -> (bool, Option<Duration>) {
        let Some(a) = &self.anim else { return (false, None) };
        if a.frames.len() < 2 {
            return (false, None);
        }
        let start = {
            let mut s = a.started.lock().unwrap();
            *s.get_or_insert(now)
        };
        let elapsed = now.saturating_duration_since(start).as_millis() as u64;
        let total: u64 = a.frames.iter().map(|f| f.delay_ms as u64).sum::<u64>().max(1);
        let (idx, next) = if a.loop_count > 0 && elapsed >= total * a.loop_count as u64 {
            (a.frames.len() - 1, None)
        } else {
            let mut t = elapsed % total;
            let mut idx = 0;
            for (i, f) in a.frames.iter().enumerate() {
                idx = i;
                if t < f.delay_ms as u64 {
                    break;
                }
                t -= f.delay_ms as u64;
            }
            let remaining = (a.frames[idx].delay_ms as u64).saturating_sub(t).max(1);
            (idx, Some(Duration::from_millis(remaining)))
        };
        let old = a.current.swap(idx, Ordering::Relaxed);
        (old != idx, next)
    }

    /// Pixel at (x, y) of the current frame as [r, g, b, a]; out of bounds
    /// → transparent.
    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0, 0, 0, 0];
        }
        let p = self.pixels();
        let i = ((y * self.width + x) * 4) as usize;
        [p[i], p[i + 1], p[i + 2], p[i + 3]]
    }

    /// Bytes held by this image's pixel buffers.
    pub fn memory_bytes(&self) -> usize {
        self.rgba.len() + self.anim.as_ref().map(|a| a.frames.iter().map(|f| f.rgba.len()).sum()).unwrap_or(0)
    }

    /// Downscale (box filter) so the pixel count stays under `max_pixels`;
    /// the natural size is preserved.
    pub fn limit_pixels(self, max_pixels: u64) -> Image {
        let px = self.width as u64 * self.height as u64;
        if px <= max_pixels || max_pixels == 0 {
            return self;
        }
        let s = (max_pixels as f64 / px as f64).sqrt();
        let nw = ((self.width as f64 * s).floor() as u32).max(1);
        let nh = ((self.height as f64 * s).floor() as u32).max(1);
        self.resized(nw, nh)
    }

    /// Box-filtered copy at `nw`×`nh` pixels (natural size preserved).
    pub fn resized(&self, nw: u32, nh: u32) -> Image {
        let rgba = resize_rgba(&self.rgba, self.width, self.height, nw, nh);
        let anim = self.anim.as_ref().map(|a| {
            Arc::new(Animation::new(
                a.frames.iter().map(|f| AnimFrame { rgba: resize_rgba(&f.rgba, self.width, self.height, nw, nh), delay_ms: f.delay_ms }).collect(),
                a.loop_count,
            ))
        });
        Image { width: nw, height: nh, rgba, natural_width: self.natural_width, natural_height: self.natural_height, anim, broken: self.broken }
    }

    /// Encode the current frame as PNG.
    pub fn to_png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let _ = image::write_buffer_with_format(
            &mut std::io::Cursor::new(&mut out),
            self.pixels(),
            self.width,
            self.height,
            image::ExtendedColorType::Rgba8,
            image::ImageFormat::Png,
        );
        out
    }
}

impl Animation {
    pub fn new(frames: Vec<AnimFrame>, loop_count: u32) -> Animation {
        Animation { frames, loop_count, current: AtomicUsize::new(0), started: Mutex::new(None) }
    }
}

/// Box-filter resample of a straight-alpha RGBA buffer (premultiplied
/// while averaging so transparent pixels don't bleed).
fn resize_rgba(src: &[u8], w: u32, h: u32, nw: u32, nh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    let sx = w as f32 / nw as f32;
    let sy = h as f32 / nh as f32;
    for y in 0..nh {
        let y0 = (y as f32 * sy) as u32;
        let y1 = (((y + 1) as f32 * sy).ceil() as u32).min(h).max(y0 + 1);
        for x in 0..nw {
            let x0 = (x as f32 * sx) as u32;
            let x1 = (((x + 1) as f32 * sx).ceil() as u32).min(w).max(x0 + 1);
            let (mut r, mut g, mut b, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for yy in y0..y1 {
                let row = (yy * w) as usize * 4;
                for xx in x0..x1 {
                    let i = row + xx as usize * 4;
                    let pa = src[i + 3] as u64;
                    r += src[i] as u64 * pa;
                    g += src[i + 1] as u64 * pa;
                    b += src[i + 2] as u64 * pa;
                    a += pa;
                    n += 1;
                }
            }
            let o = ((y * nw + x) * 4) as usize;
            if a > 0 {
                out[o] = (r / a) as u8;
                out[o + 1] = (g / a) as u8;
                out[o + 2] = (b / a) as u8;
                out[o + 3] = (a / n.max(1)) as u8;
            }
        }
    }
    out
}

fn decode_still(bytes: &[u8], opts: &DecodeOptions) -> Option<Image> {
    use image::ImageDecoder;
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let icc = if opts.color_manage { decoder.icc_profile().ok().flatten() } else { None };
    let dynimg = image::DynamicImage::from_decoder(decoder).ok()?;
    let rgba = dynimg.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut rgba = rgba.into_raw();
    if let Some(icc) = icc {
        apply_icc(&icc, &mut rgba);
    }
    Some(Image::new(width, height, rgba))
}

/// Convert RGBA pixels tagged with an RGB ICC profile to sRGB in place.
fn apply_icc(icc: &[u8], rgba: &mut [u8]) {
    let Ok(profile) = moxcms::ColorProfile::new_from_slice(icc) else { return };
    if profile.color_space != moxcms::DataColorSpace::Rgb {
        return;
    }
    let srgb = moxcms::ColorProfile::new_srgb();
    let Ok(t) = profile.create_transform_8bit(moxcms::Layout::Rgba, &srgb, moxcms::Layout::Rgba, moxcms::TransformOptions::default()) else { return };
    let mut out = vec![0u8; rgba.len()];
    if t.transform(rgba, &mut out).is_ok() {
        rgba.copy_from_slice(&out);
    }
}

/// Decode all frames of an animated GIF/APNG/WebP. `None` = not animated
/// (decode as a still image), `Some(Err)` = broken.
fn decode_animated(bytes: &[u8], fmt: Format, opts: &DecodeOptions) -> Option<Result<Image, ()>> {
    use image::AnimationDecoder;
    let cur = std::io::Cursor::new(bytes);
    let (frames, loops) = match fmt {
        Format::Gif => {
            let d = image::codecs::gif::GifDecoder::new(cur).ok()?;
            let loops = d.loop_count();
            (d.into_frames(), loops)
        }
        Format::Png => {
            let d = image::codecs::png::PngDecoder::new(cur).ok()?;
            if !d.is_apng().ok()? {
                return None;
            }
            let d = d.apng().ok()?;
            let loops = d.loop_count();
            (d.into_frames(), loops)
        }
        Format::WebP => {
            let d = image::codecs::webp::WebPDecoder::new(cur).ok()?;
            if !d.has_animation() {
                return None;
            }
            let loops = d.loop_count();
            (d.into_frames(), loops)
        }
        _ => return None,
    };
    let loop_count = match loops {
        image::metadata::LoopCount::Infinite => 0,
        image::metadata::LoopCount::Finite(n) => n.get(),
    };
    let mut out: Vec<AnimFrame> = Vec::new();
    let mut size = (0u32, 0u32);
    let mut bytes_used = 0usize;
    for f in frames {
        let Ok(f) = f else { break };
        let (num, den) = f.delay().numer_denom_ms();
        let mut delay = if den == 0 { 100 } else { num / den };
        // Browsers clamp very short GIF delays (0/10 ms) to 100 ms.
        if delay <= 10 {
            delay = 100;
        }
        let buf = f.into_buffer();
        if out.is_empty() {
            size = buf.dimensions();
        } else if buf.dimensions() != size {
            continue;
        }
        bytes_used += buf.len();
        if bytes_used > opts.max_animation_bytes && !out.is_empty() {
            break;
        }
        out.push(AnimFrame { rgba: buf.into_raw(), delay_ms: delay });
        if out.len() >= 2000 {
            break;
        }
    }
    if out.is_empty() {
        return Some(Err(()));
    }
    let (w, h) = size;
    if w == 0 || h == 0 {
        return Some(Err(()));
    }
    if out.len() == 1 {
        let f = out.pop().unwrap();
        return Some(Ok(Image::new(w, h, f.rgba)));
    }
    let first = out[0].rgba.clone();
    let mut img = Image::new(w, h, first);
    img.anim = Some(Arc::new(Animation::new(out, loop_count)));
    Some(Ok(img))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_png() {
        let mut buf = image::RgbaImage::new(3, 2);
        buf.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        buf.put_pixel(2, 1, image::Rgba([0, 0, 255, 128]));
        let mut bytes = Vec::new();
        buf.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        let img = Image::decode(&bytes).unwrap();
        assert_eq!((img.width, img.height), (3, 2));
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.pixel(2, 1), [0, 0, 255, 128]);
        assert_eq!(img.pixel(5, 5), [0, 0, 0, 0]);
        assert!(Image::decode(b"not an image").is_none());
        assert!(Image::decode(b"<svg xmlns='http://www.w3.org/2000/svg'/>").is_none());
    }

    #[test]
    fn decode_16bit_png() {
        let mut buf: image::ImageBuffer<image::Rgba<u16>, Vec<u16>> = image::ImageBuffer::new(2, 1);
        buf.put_pixel(0, 0, image::Rgba([65535, 0, 0, 65535]));
        buf.put_pixel(1, 0, image::Rgba([0, 32768, 0, 65535]));
        let mut bytes = Vec::new();
        buf.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        let img = Image::decode(&bytes).unwrap();
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0)[1], 128);
    }

    #[test]
    fn downscale_keeps_natural_size() {
        let mut buf = image::RgbaImage::new(400, 200);
        for p in buf.pixels_mut() {
            *p = image::Rgba([10, 20, 30, 255]);
        }
        let mut bytes = Vec::new();
        buf.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        let img = Image::decode_with(&bytes, &DecodeOptions { max_pixels: 100 * 50, ..Default::default() }).unwrap();
        assert_eq!((img.width, img.height), (100, 50));
        assert_eq!(img.natural_size(), (400.0, 200.0));
        assert_eq!(img.pixel(50, 25), [10, 20, 30, 255]);
    }

    pub(crate) fn gif_bytes() -> Vec<u8> {
        use image::codecs::gif::{GifEncoder, Repeat};
        let mut bytes = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut bytes);
            enc.set_repeat(Repeat::Infinite).unwrap();
            for (c, ms) in [([255u8, 0, 0, 255], 100u32), ([0, 0, 255, 255], 200)] {
                let mut b = image::RgbaImage::new(4, 4);
                for p in b.pixels_mut() {
                    *p = image::Rgba(c);
                }
                let frame = image::Frame::from_parts(b, 0, 0, image::Delay::from_numer_denom_ms(ms, 1));
                enc.encode_frame(frame).unwrap();
            }
        }
        bytes
    }

    #[test]
    fn animated_gif_frames_advance() {
        let img = Image::decode(&gif_bytes()).unwrap();
        assert!(img.is_animated());
        assert_eq!(img.anim.as_ref().unwrap().frames.len(), 2);
        let t0 = Instant::now();
        let (_, next) = img.update_animation(t0);
        assert_eq!(img.current_frame(), 0);
        assert!(img.pixel(0, 0)[0] > 200);
        assert!(next.unwrap() <= Duration::from_millis(100));
        let (changed, _) = img.update_animation(t0 + Duration::from_millis(150));
        assert!(changed);
        assert_eq!(img.current_frame(), 1);
        assert!(img.pixel(0, 0)[2] > 200);
        // Loops forever: 300 ms per cycle.
        img.update_animation(t0 + Duration::from_millis(320));
        assert_eq!(img.current_frame(), 0);
    }

    #[test]
    fn decode_avif() {
        let img = Image::decode(include_bytes!("../fixtures/white_1x1.avif")).expect("avif");
        assert_eq!((img.width, img.height), (1, 1));
        let p = img.pixel(0, 0);
        assert!(p[0] > 245 && p[1] > 245 && p[2] > 245 && p[3] == 255, "{p:?}");
        // Still image with an auxiliary alpha plane.
        let img = Image::decode(include_bytes!("../fixtures/alpha.avif")).expect("avif alpha");
        assert!(img.width > 0 && img.height > 0);
        assert!(img.rgba.chunks(4).any(|p| p[3] < 255), "alpha plane applied");
    }

    #[test]
    fn decode_ico_and_jpeg() {
        let mut buf = image::RgbaImage::new(16, 16);
        for p in buf.pixels_mut() {
            *p = image::Rgba([0, 200, 0, 255]);
        }
        let mut ico = Vec::new();
        buf.write_to(&mut std::io::Cursor::new(&mut ico), image::ImageFormat::Ico).unwrap();
        assert_eq!(sniff(&ico), Format::Ico);
        let img = Image::decode(&ico).unwrap();
        assert_eq!((img.width, img.height), (16, 16));
        assert_eq!(img.pixel(3, 3)[1], 200);
        let rgb = image::DynamicImage::ImageRgba8(buf).to_rgb8();
        let mut jpg = Vec::new();
        rgb.write_to(&mut std::io::Cursor::new(&mut jpg), image::ImageFormat::Jpeg).unwrap();
        let img = Image::decode(&jpg).unwrap();
        assert!(img.pixel(8, 8)[1] > 180 && img.pixel(8, 8)[0] < 40);
    }

    #[test]
    fn sniffing() {
        assert_eq!(sniff(&gif_bytes()), Format::Gif);
        assert_eq!(sniff(b"\0\0\0\x1cftypavif\0\0\0\0"), Format::Avif);
        assert_eq!(sniff(b"<?xml version='1.0'?><svg>"), Format::Svg);
        assert!(supports_mime("image/avif"));
        assert!(supports_mime("image/webp"));
        assert!(!supports_mime("image/jxl"));
    }
}
