//! Texture keys → premultiplied RGBA8: image files (JPEG through libjpeg-turbo, decoded at
//! 1/k with its DCT scaling; PNG) and text (text.rs).
use std::cell::RefCell;

use crate::canvas::TexKey;
use crate::gpu::Decoded;
use crate::text;

thread_local! {
    static JPEG: RefCell<turbojpeg::Decompressor> = RefCell::new(turbojpeg::Decompressor::new().unwrap());
}

/// A texture key → its pixels. A file that can't be read becomes a magenta square (and a
/// message), not a crash mid-render.
pub fn decode(key: &TexKey) -> Decoded {
    match key {
        TexKey::Img(p, k) => file(p, *k).unwrap_or_else(|e| {
            eprintln!("image {p}: {e}");
            Decoded { w: 1, h: 1, data: vec![255, 0, 255, 255] }
        }),
        TexKey::Text { para, color, alpha } => {
            let r = text::raster(para, *color, f32::from_bits(*alpha));
            Decoded { w: r.w as u32, h: r.h as u32, data: r.data }
        }
    }
}

fn file(p: &str, k: u32) -> anyhow::Result<Decoded> {
    let data = std::fs::read(p)?;
    if data.starts_with(&[0xFF, 0xD8]) {
        return JPEG.with(|d| {
            let mut d = d.borrow_mut();
            let sf = turbojpeg::ScalingFactor::new(1, k.max(1) as usize);
            d.set_scaling_factor(sf)?;
            let hd = d.read_header(&data)?.scaled(sf);
            let mut img = turbojpeg::Image {
                pixels: vec![0u8; 4 * hd.width * hd.height],
                width: hd.width,
                pitch: 4 * hd.width,
                height: hd.height,
                format: turbojpeg::PixelFormat::RGBA,
            };
            d.decompress(&data, img.as_deref_mut())?;
            Ok(Decoded { w: hd.width as u32, h: hd.height as u32, data: img.pixels })
        });
    }
    let mut dec = png::Decoder::new(std::io::Cursor::new(&data));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut r = dec.read_info()?;
    let mut buf = vec![0; r.output_buffer_size().ok_or_else(|| anyhow::anyhow!("PNG too large"))?];
    let info = r.next_frame(&mut buf)?;
    let (w, h) = (info.width, info.height);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    let px = &buf[..info.buffer_size()];
    match info.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(px),
        png::ColorType::Rgb => px.chunks(3).for_each(|c| rgba.extend_from_slice(&[c[0], c[1], c[2], 255])),
        png::ColorType::GrayscaleAlpha => px.chunks(2).for_each(|c| rgba.extend_from_slice(&[c[0], c[0], c[0], c[1]])),
        png::ColorType::Grayscale => px.iter().for_each(|&c| rgba.extend_from_slice(&[c, c, c, 255])),
        t => anyhow::bail!("PNG colour type {t:?}"),
    }
    for c in rgba.chunks_mut(4) {
        let a = c[3] as u32;
        for v in &mut c[..3] {
            *v = ((*v as u32 * a + 127) / 255) as u8;
        }
    }
    if k > 1 {
        let (dw, dh) = ((w / k).max(1), (h / k).max(1));
        return Ok(Decoded { w: dw, h: dh, data: text::resize_premul(rgba, w, h, dw, dh) });
    }
    Ok(Decoded { w, h, data: rgba })
}
