//! Shared image decoding, white-background compositing and scan PDF encoding.
use crate::model::{AppResult, ScanOptions};
use image::DynamicImage;
use std::io::Write;

pub fn load_image(path: &str) -> AppResult<DynamicImage> {
    let mut reader = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16000);
    limits.max_image_height = Some(16000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().map_err(|e| format!("图片无法打开：{e}"))
}
pub fn white_rgb(img: DynamicImage) -> image::RgbImage {
    // JPEGs and most scanner output already have RGB pixels. Reuse their buffer
    // instead of allocating RGBA and RGB copies for each high-resolution page.
    if let DynamicImage::ImageRgb8(rgb) = img {
        return rgb;
    }
    let rgba = img.into_rgba8();
    image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let p = rgba.get_pixel(x, y).0;
        let a = p[3] as u16;
        image::Rgb([0, 1, 2].map(|i| ((p[i] as u16 * a + 255 * (255 - a)) / 255) as u8))
    })
}

pub fn save_scan(img: DynamicImage, output: &str, options: &ScanOptions) -> AppResult<()> {
    let img = if options.color { img } else { img.grayscale() };
    if options.format != "pdf" {
        return img.save(output).map_err(|e| e.to_string());
    }
    let rgb = white_rgb(img);
    let (w, h) = rgb.dimensions();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode_image(&rgb)
        .map_err(|e| e.to_string())?;
    let width = w as f64 * 72.0 / options.dpi as f64;
    let height = h as f64 * 72.0 / options.dpi as f64;
    let content = format!("q {width:.3} 0 0 {height:.3} 0 0 cm /Im0 Do Q");
    let mut objects = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width:.3} {height:.3}] /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>").into_bytes()];
    let mut stream = format!("<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n", jpeg.len()).into_bytes();
    stream.extend(jpeg);
    stream.extend(b"\nendstream");
    objects.push(stream);
    objects.push(
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        )
        .into_bytes(),
    );
    let mut bytes = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = vec![0];
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        writeln!(bytes, "{} 0 obj", i + 1).map_err(|e| e.to_string())?;
        bytes.extend(obj);
        bytes.extend(b"\nendobj\n");
    }
    let xref = bytes.len();
    write!(bytes, "xref\n0 {}\n0000000000 65535 f \n", offsets.len()).map_err(|e| e.to_string())?;
    for offset in offsets.iter().skip(1) {
        writeln!(bytes, "{offset:010} 00000 n ").map_err(|e| e.to_string())?;
    }
    write!(
        bytes,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        offsets.len()
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(output, bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_images_reuse_pixels_and_transparency_composites_on_white() {
        let rgb = image::RgbImage::from_pixel(2, 2, image::Rgb([10, 20, 30]));
        let pixels = rgb.as_ptr();
        let result = white_rgb(DynamicImage::ImageRgb8(rgb));
        assert_eq!(result.as_ptr(), pixels);
        assert_eq!(result.get_pixel(0, 0).0, [10, 20, 30]);

        let rgba =
            image::RgbaImage::from_raw(3, 1, vec![10, 20, 30, 255, 10, 20, 30, 0, 10, 20, 30, 128])
                .unwrap();
        let result = white_rgb(DynamicImage::ImageRgba8(rgba));
        assert_eq!(result.get_pixel(0, 0).0, [10, 20, 30]);
        assert_eq!(result.get_pixel(1, 0).0, [255, 255, 255]);
        assert_eq!(result.get_pixel(2, 0).0, [132, 137, 142]);
    }
}
