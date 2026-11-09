//! Decoded raster images (RGBA8).

#[derive(Debug, Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// Straight (non-premultiplied) RGBA, row-major.
    pub rgba: Vec<u8>,
}

impl Image {
    /// Decode PNG/JPEG/GIF (first frame)/WebP/BMP/ICO. Returns None on
    /// failure or for unsupported formats (e.g. SVG).
    pub fn decode(bytes: &[u8]) -> Option<Image> {
        unimplemented!()
    }

    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Image {
        Image { width, height, rgba }
    }
}
