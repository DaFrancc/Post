//! Profile pictures and banners (design section 7): what an app makes from
//! the image its user picks, and what it accepts from others.
//!
//! The sender decodes the image with limits, crops it to shape, shrinks it
//! to fit and encodes it again as PNG or JPEG, so whatever was picked, what
//! leaves the device is a plain image within the limits, with no metadata
//! (EXIF, location) left in it. A receiver decodes it fully, with limits
//! set to the largest allowed size, before showing it: an image claiming to
//! be small but decoding huge (a "decompression bomb") is refused there.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder as _, ImageFormat, ImageReader, Limits};

use crate::{Error, Result};

/// Which picture: they differ in shape and size.
#[post_api_macros::api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PictureKind {
    /// The round profile picture: square.
    Avatar,
    /// The banner across the top of a profile: three times as wide as tall.
    Banner,
}

impl PictureKind {
    /// The largest it may be, in pixels (width, height).
    pub const fn max_size(self) -> (u32, u32) {
        match self {
            Self::Avatar => (512, 512),
            Self::Banner => (1500, 500),
        }
    }

    /// The largest its file may be, in bytes.
    pub const fn max_bytes(self) -> usize {
        match self {
            Self::Avatar => 200 * 1024,
            Self::Banner => 500 * 1024,
        }
    }

    /// Its shape, width over height.
    pub const fn aspect(self) -> f64 {
        match self {
            Self::Avatar => 1.0,
            Self::Banner => 3.0,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Avatar => "profile picture",
            Self::Banner => "banner",
        }
    }
}

/// The largest file a picture may be made from.
pub const MAX_SOURCE_BYTES: usize = 40 * 1024 * 1024;
/// The widest or tallest image a picture may be made from.
const MAX_SOURCE_SIDE: u32 = 16_384;
/// The most memory decoding a picked image may take.
const MAX_SOURCE_ALLOC: u64 = 512 * 1024 * 1024;

/// The part of an image to use, as fractions of its width and height (of
/// the image as it is shown, turned upright). The app's cropper gives it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// A picture ready to encrypt and send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    /// The encoded image.
    pub bytes: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `image/png` or `image/jpeg`.
    pub mime: &'static str,
}

fn unreadable() -> Error {
    Error::Picture("that file is not an image Post can read (PNG, JPEG, WebP or GIF)".into())
}

/// Make a picture of `kind` from the image file `source`: the `crop`
/// part of it (else its middle, in shape), shrunk to fit and encoded within
/// the size limit.
pub fn prepare(kind: PictureKind, source: &[u8], crop: Option<Crop>) -> Result<Picture> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(Error::Picture(format!("that image is over {} MB; pick a smaller one", MAX_SOURCE_BYTES / 1024 / 1024)));
    }
    let mut reader = ImageReader::new(Cursor::new(source)).with_guessed_format().map_err(|_| unreadable())?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif)) {
        return Err(unreadable());
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_SIDE);
    limits.max_image_height = Some(MAX_SOURCE_SIDE);
    limits.max_alloc = Some(MAX_SOURCE_ALLOC);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|_| unreadable())?;
    // Photos are often stored sideways with a note saying which way is up.
    let orientation = decoder.orientation().map_err(|_| unreadable())?;
    let mut img = DynamicImage::from_decoder(decoder).map_err(|_| unreadable())?;
    img.apply_orientation(orientation);
    let img = crop_to_shape(&img, kind.aspect(), crop);
    let (max_w, max_h) = kind.max_size();
    let img = if img.width() > max_w || img.height() > max_h { img.resize(max_w, max_h, FilterType::Lanczos3) } else { img };
    encode_within(kind, img)
}

/// The `crop` part of `img`, made exactly `aspect` (width over height)
/// around its centre; with no crop, the largest such part from the middle.
fn crop_to_shape(img: &DynamicImage, aspect: f64, crop: Option<Crop>) -> DynamicImage {
    let (w, h) = (f64::from(img.width()), f64::from(img.height()));
    let c = crop.unwrap_or(Crop { x: 0.0, y: 0.0, width: 1.0, height: 1.0 });
    let fraction = |v: f64| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    let (x0, y0) = (fraction(c.x) * w, fraction(c.y) * h);
    let (mut cw, mut ch) = ((fraction(c.width) * w).min(w - x0), (fraction(c.height) * h).min(h - y0));
    if cw < 1.0 || ch < 1.0 {
        (cw, ch) = (w, h);
    }
    let (cx, cy) = (x0 + cw / 2.0, y0 + ch / 2.0);
    if cw / ch > aspect {
        cw = ch * aspect;
    } else {
        ch = cw / aspect;
    }
    let left = (cx - cw / 2.0).clamp(0.0, (w - cw).max(0.0));
    let top = (cy - ch / 2.0).clamp(0.0, (h - ch).max(0.0));
    img.crop_imm(left as u32, top as u32, (cw.round() as u32).max(1), (ch.round() as u32).max(1))
}

/// Encode as PNG where it has see-through parts, else JPEG, stepping the
/// quality and then the size down until it fits the limit.
fn encode_within(kind: PictureKind, mut img: DynamicImage) -> Result<Picture> {
    let transparent = img.color().has_alpha() && img.to_rgba8().pixels().any(|p| p.0[3] < 255);
    let max = kind.max_bytes();
    loop {
        let (width, height) = (img.width(), img.height());
        if transparent {
            let mut out = Vec::new();
            img.to_rgba8()
                .write_with_encoder(PngEncoder::new(&mut out))
                .map_err(|e| Error::Picture(format!("encoding the {} failed: {e}", kind.name())))?;
            if out.len() <= max {
                return Ok(Picture { bytes: out, width, height, mime: "image/png" });
            }
        } else {
            let rgb = img.to_rgb8();
            for quality in [90, 82, 74, 66] {
                let mut out = Vec::new();
                rgb.write_with_encoder(JpegEncoder::new_with_quality(&mut out, quality))
                    .map_err(|e| Error::Picture(format!("encoding the {} failed: {e}", kind.name())))?;
                if out.len() <= max {
                    return Ok(Picture { bytes: out, width, height, mime: "image/jpeg" });
                }
            }
        }
        if width.min(height) <= 64 {
            return Err(Error::Picture(format!("could not make the {} small enough", kind.name())));
        }
        img = img.resize(width * 4 / 5, height * 4 / 5, FilterType::Lanczos3);
    }
}

/// A picture this device holds (already checked), cropped from its middle
/// to the shape of `width`×`height` and scaled to exactly that many pixels:
/// RGBA, row by row.
///
/// This is how a picture should reach a screen, especially a small one,
/// and what Discord's image servers and Signal's thumbnailing do: shrunk
/// once, from the original, to the pixels it will fill, with a sharp
/// filter (Catmull-Rom), in linear light with alpha premultiplied, so
/// edges stay crisp and colours do not muddy. Letting a GPU sample a picture
/// many times its shown size instead skips most of its pixels, and fine
/// detail breaks up.
pub fn scaled(bytes: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let refuse = || Error::Picture("a held picture could not be read".into());
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err(Error::Picture("a picture can be drawn at 1 to 4096 pixels".into()));
    }
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|_| refuse())?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_SIDE);
    limits.max_image_height = Some(MAX_SOURCE_SIDE);
    reader.limits(limits);
    let img = reader.decode().map_err(|_| refuse())?;
    let img = crop_to_shape(&img, f64::from(width) / f64::from(height), None);

    // Into linear light, premultiplied, as floats.
    let to_linear: Vec<f32> = (0..=255u8).map(|v| srgb_to_linear(f32::from(v) / 255.0)).collect();
    let src = img.to_rgba8();
    let linear = image::ImageBuffer::<image::Rgba<f32>, Vec<f32>>::from_fn(src.width(), src.height(), |x, y| {
        let [r, g, b, a] = src.get_pixel(x, y).0;
        let a = f32::from(a) / 255.0;
        image::Rgba([to_linear[r as usize] * a, to_linear[g as usize] * a, to_linear[b as usize] * a, a])
    });
    // Catmull-Rom: nearly as sharp as Lanczos, with far less ringing. In
    // linear light, Lanczos left a dark halo around bright shapes, plain
    // to see on small avatars with hard edges (logos, drawings).
    let out = image::imageops::resize(&linear, width, height, FilterType::CatmullRom);
    // Back to sRGB bytes. The filter overshoots a little at edges: clamp.
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for p in out.pixels() {
        let [r, g, b, a] = p.0;
        let a = a.clamp(0.0, 1.0);
        let un = |c: f32| if a > 0.0 { (c / a).clamp(0.0, 1.0) } else { 0.0 };
        let byte = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
        rgba.extend_from_slice(&[byte(linear_to_srgb(un(r))), byte(linear_to_srgb(un(g))), byte(linear_to_srgb(un(b))), byte(a)]);
    }
    Ok(rgba)
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Check a received picture before it is kept or shown: within the size
/// limit, PNG or JPEG, and decoding (fully) to no more than the largest
/// allowed size. Returns its size in pixels.
pub fn check(kind: PictureKind, bytes: &[u8]) -> Result<(u32, u32)> {
    let refuse = || Error::Picture(format!("someone's {} was not a valid image", kind.name()));
    if bytes.len() > kind.max_bytes() {
        return Err(refuse());
    }
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|_| refuse())?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg)) {
        return Err(refuse());
    }
    let (max_w, max_h) = kind.max_size();
    let mut limits = Limits::default();
    limits.max_image_width = Some(max_w);
    limits.max_image_height = Some(max_h);
    limits.max_alloc = Some(u64::from(max_w) * u64::from(max_h) * 8);
    reader.limits(limits);
    let img = reader.decode().map_err(|_| refuse())?;
    Ok((img.width(), img.height()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage, Rgba, RgbaImage};

    fn png(img: &DynamicImage) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).expect("png");
        out
    }

    /// A photo-like image: noisy, so it compresses badly.
    fn noisy(w: u32, h: u32) -> DynamicImage {
        let mut seed = 1u32;
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |_, _| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let b = seed.to_le_bytes();
            Rgb([b[1], b[2], b[3]])
        }))
    }

    #[test]
    fn pictures_come_out_in_shape_and_within_limits() {
        let source = png(&noisy(2400, 1600));
        for kind in [PictureKind::Avatar, PictureKind::Banner] {
            let p = prepare(kind, &source, None).expect("prepare");
            assert!(p.bytes.len() <= kind.max_bytes(), "{kind:?}: {} bytes", p.bytes.len());
            let (max_w, max_h) = kind.max_size();
            assert!(p.width <= max_w && p.height <= max_h);
            let ratio = f64::from(p.width) / f64::from(p.height);
            assert!((ratio - kind.aspect()).abs() < 0.01, "{kind:?}: {}x{}", p.width, p.height);
            assert_eq!(p.mime, "image/jpeg");
            assert_eq!(check(kind, &p.bytes).expect("a receiver accepts it"), (p.width, p.height));
        }
    }

    #[test]
    fn a_crop_picks_the_part() {
        // Left half red, right half blue: cropping the right half gives blue.
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(400, 200, |x, _| if x < 200 { Rgb([255, 0, 0]) } else { Rgb([0, 0, 255]) }));
        let crop = Crop { x: 0.5, y: 0.0, width: 0.5, height: 1.0 };
        let p = prepare(PictureKind::Avatar, &png(&img), Some(crop)).expect("prepare");
        assert_eq!((p.width, p.height), (200, 200));
        let out = image::load_from_memory(&p.bytes).expect("decode").to_rgb8();
        let px = out.get_pixel(100, 100).0;
        assert!(px[2] > 200 && px[0] < 50, "{px:?}");
    }

    #[test]
    fn transparency_is_kept() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_fn(300, 300, |x, _| Rgba([10, 200, 90, if x < 150 { 0 } else { 255 }])));
        let p = prepare(PictureKind::Avatar, &png(&img), None).expect("prepare");
        assert_eq!(p.mime, "image/png");
        assert_eq!((p.width, p.height), (300, 300), "small enough already: not enlarged");
    }

    #[test]
    fn pictures_scale_to_exact_pixels_without_muddying() {
        // Fine black and white stripes average to mid grey in linear light,
        // which is about 188 in sRGB, not the 128 a naive average gives.
        let stripes =
            DynamicImage::ImageRgb8(RgbImage::from_fn(512, 512, |x, _| if x % 2 == 0 { Rgb([0, 0, 0]) } else { Rgb([255, 255, 255]) }));
        let small = scaled(&png(&stripes), 32, 32).expect("scaled");
        assert_eq!(small.len(), 32 * 32 * 4);
        let grey = small[4 * (16 * 32 + 16)];
        assert!((180..=196).contains(&grey), "{grey}");
        assert_eq!(small[4 * (16 * 32 + 16) + 3], 255, "opaque stays opaque");
        // A banner is cropped to the shape asked for, not squashed.
        let wide = scaled(&png(&noisy(1500, 500)), 300, 100).expect("banner");
        assert_eq!(wide.len(), 300 * 100 * 4);
        // See-through edges do not pick up a dark fringe.
        let dot = DynamicImage::ImageRgba8(RgbaImage::from_fn(64, 64, |x, _| Rgba([255, 255, 255, if x < 32 { 0 } else { 255 }])));
        let out = scaled(&png(&dot), 16, 16).expect("scaled");
        for px in out.chunks(4).filter(|p| p[3] > 0) {
            assert!(px[0] > 240, "fringe: {px:?}");
        }
        assert!(scaled(&png(&dot), 0, 16).is_err());
    }

    #[test]
    fn receivers_refuse_what_no_app_would_send() {
        // Larger than the largest allowed size.
        let big = png(&DynamicImage::ImageRgb8(RgbImage::new(600, 600)));
        assert!(check(PictureKind::Avatar, &big).is_err());
        assert!(check(PictureKind::Banner, &png(&DynamicImage::ImageRgb8(RgbImage::new(1500, 501)))).is_err());
        // Not an image, or a format apps do not send.
        assert!(check(PictureKind::Avatar, b"hello").is_err());
        let mut gif = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::new(10, 10)).write_to(&mut Cursor::new(&mut gif), ImageFormat::Gif).expect("gif");
        assert!(check(PictureKind::Avatar, &gif).is_err());
        // Over the byte limit.
        assert!(check(PictureKind::Avatar, &vec![0; PictureKind::Avatar.max_bytes() + 1]).is_err());
        // Something that is not an image at all is refused when picked.
        assert!(prepare(PictureKind::Avatar, b"not an image", None).is_err());
    }
}
