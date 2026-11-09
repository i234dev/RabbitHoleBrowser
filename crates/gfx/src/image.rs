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
        let img = image::load_from_memory(bytes).ok()?;
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();
        if width == 0 || height == 0 {
            return None;
        }
        Some(Image {
            width,
            height,
            rgba: rgba.into_raw(),
        })
    }

    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Image {
        debug_assert_eq!(rgba.len(), (width * height * 4) as usize);
        Image { width, height, rgba }
    }

    /// Pixel at (x, y) as [r, g, b, a]; out of bounds → transparent.
    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0, 0, 0, 0];
        }
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
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
}
