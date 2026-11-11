//! "Print to PDF": a minimal PDF writer of our own. Each page is the
//! rasterized page content (our renderer's pixels) embedded as a
//! Flate-compressed RGB image filling an A4 page. Text is not selectable
//! (the page is an image), but the output prints and views anywhere.

use rabbithole_engine::gfx::{Canvas, Color, FontContext};
use rabbithole_engine::layout::DisplayList;

/// A4 in PDF points (1/72 in).
pub const A4: (f32, f32) = (595.28, 841.89);

/// One page image: RGB bytes, `width × height` pixels.
pub struct PageImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl PageImage {
    /// Take the pixels of a canvas (0x00RRGGBB).
    pub fn from_canvas(c: &Canvas) -> PageImage {
        let mut rgb = Vec::with_capacity(c.data.len() * 3);
        for p in &c.data {
            rgb.push((p >> 16) as u8);
            rgb.push((p >> 8) as u8);
            rgb.push(*p as u8);
        }
        PageImage { width: c.width, height: c.height, rgb }
    }
}

/// Serialize pages into a PDF document; each image is scaled to the page
/// width and placed at the top of a page of size `page` (points).
pub fn write_pdf(pages: &[PageImage], page: (f32, f32), title: &str) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut offsets: Vec<usize> = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n");
    // Objects: 1 catalog, 2 pages, 3 info, then per page: page, content, image.
    let n = pages.len().max(1);
    let page_obj = |i: usize| 4 + i * 3;
    let obj = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(out.len());
        let id = offsets.len();
        out.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    obj(&mut out, &mut offsets, b"<< /Type /Catalog /Pages 2 0 R >>");
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", page_obj(i))).collect();
    obj(&mut out, &mut offsets, format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), n).as_bytes());
    obj(&mut out, &mut offsets, format!("<< /Title {} /Producer (RabbitHole {}) >>", pdf_string(title), env!("CARGO_PKG_VERSION")).as_bytes());
    for i in 0..n {
        let (pw, ph) = page;
        let mut content = String::new();
        if let Some(img) = pages.get(i) {
            let w = pw;
            let h = pw * img.height as f32 / img.width.max(1) as f32;
            content = format!("q\n{w:.2} 0 0 {h:.2} 0 {:.2} cm\n/Im0 Do\nQ\n", ph - h);
        }
        obj(
            &mut out,
            &mut offsets,
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2} {ph:.2}] /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>", page_obj(i) + 2, page_obj(i) + 1).as_bytes(),
        );
        let mut c = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        c.extend_from_slice(content.as_bytes());
        c.extend_from_slice(b"\nendstream");
        obj(&mut out, &mut offsets, &c);
        let (w, h, data) = match pages.get(i) {
            Some(img) => (img.width, img.height, miniz_oxide::deflate::compress_to_vec_zlib(&img.rgb, 6)),
            None => (1, 1, miniz_oxide::deflate::compress_to_vec_zlib(&[255, 255, 255], 6)),
        };
        let mut b = format!("<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n", data.len()).into_bytes();
        b.extend_from_slice(&data);
        b.extend_from_slice(b"\nendstream");
        obj(&mut out, &mut offsets, &b);
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1).as_bytes());
    out
}

/// A PDF literal string (ASCII; other characters become '?').
fn pdf_string(s: &str) -> String {
    let mut out = String::from("(");
    for c in s.chars().take(200) {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            ' '..='~' => out.push(c),
            _ => out.push('?'),
        }
    }
    out.push(')');
    out
}

/// Rasterize a laid-out page into A4-proportioned page images:
/// `css_width` wide (CSS px) at `scale` device px per CSS px.
pub fn paginate(list: &DisplayList, fonts: &FontContext, css_width: f32, content_height: f32, background: Color, scale: f32) -> Vec<PageImage> {
    let page_css_h = css_width * A4.1 / A4.0;
    let w = (css_width * scale).ceil().max(1.0) as u32;
    let mut pages = Vec::new();
    let mut y = 0.0;
    let total = content_height.max(1.0);
    while y < total && pages.len() < 200 {
        let h_css = page_css_h.min(total - y).max(1.0);
        let h = (h_css * scale).ceil().max(1.0) as u32;
        let mut canvas = Canvas::new(w, h);
        canvas.clear(Color::WHITE);
        if background.a > 0 {
            canvas.fill_rect(rabbithole_engine::gfx::Rect::new(0.0, 0.0, w as f32, h as f32), background);
        }
        canvas.push_transform([scale, 0.0, 0.0, scale, 0.0, 0.0]);
        // Fixed boxes repeat at the top of every page, as when printing.
        rabbithole_engine::layout::paint_with_fixed_origin(list, &mut canvas, fonts, 0.0, y, (0.0, 0.0));
        canvas.pop_transform();
        pages.push(PageImage::from_canvas(&canvas));
        y += page_css_h;
    }
    pages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_wellformed_pdf() {
        let mut c = Canvas::new(4, 6);
        c.clear(Color::rgb(255, 0, 0));
        let pages = vec![PageImage::from_canvas(&c), PageImage::from_canvas(&c)];
        assert_eq!(&pages[0].rgb[..3], &[255, 0, 0]);
        let pdf = write_pdf(&pages, A4, "Test (page) é");
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.contains("/Count 2"));
        assert!(text.contains("/Title (Test \\(page\\) ?)"));
        assert!(text.trim_end().ends_with("%%EOF"));
        // Every xref offset points at its object.
        let xref_at: usize = text.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
        let xref = &text[xref_at..];
        for (i, line) in xref.lines().skip(3).take_while(|l| l.ends_with(" n ")).enumerate() {
            let off: usize = line[..10].parse().unwrap();
            assert!(text[off..].starts_with(&format!("{} 0 obj", i + 1)), "object {} at {off}", i + 1);
        }
        // The image stream inflates back to the pixels.
        let start = pdf.windows(7).position(|w| w == b"/Image ").unwrap();
        let s = start + pdf[start..].windows(7).position(|w| w == b"stream\n").unwrap() + 7;
        let e = s + pdf[s..].windows(10).position(|w| w == b"\nendstream").unwrap();
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&pdf[s..e]).unwrap();
        assert_eq!(raw.len(), 4 * 6 * 3);
    }
}
