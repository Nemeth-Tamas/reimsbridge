use std::error::Error;
use std::io::Cursor;

use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, RgbaImage,
    codecs::{
        bmp::BmpDecoder,
        png::{PngDecoder, PngEncoder},
    },
};

use crate::clipboard_bundle::{canonical_png, valid_image_dimensions};

pub fn dib_to_png(bytes: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    let bmp = add_bitmap_file_header(bytes)?;
    let decoder = if let Some(ref bmp) = bmp {
        BmpDecoder::new(Cursor::new(bmp.as_slice()))?
    } else {
        BmpDecoder::new_without_file_header(Cursor::new(bytes))?
    };
    let (width, height) = decoder.dimensions();
    if !valid_image_dimensions(width, height) {
        return Err("Clipboard bitmap dimensions exceed the limit".into());
    }
    let image = DynamicImage::from_decoder(decoder)?.into_rgba8();
    rgba_to_png(&image)
}

fn add_bitmap_file_header(dib: &[u8]) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
    let Some(info) = dib.get(..40) else {
        return Ok(None);
    };
    let header_size = u32::from_le_bytes(info[..4].try_into()?);
    let compression = u32::from_le_bytes(info[16..20].try_into()?);
    if !matches!(header_size, 108 | 124) || compression != 3 {
        return Ok(None);
    }

    // In V4/V5 DIBs the channel masks are part of the header.
    // Supply the actual pixel offset instead of guessing from mask count.
    let color_table_size = u64::from(u32::from_le_bytes(info[32..36].try_into()?)) * 4;
    let offset = u64::from(header_size) + color_table_size;
    if offset > dib.len() as u64 {
        return Err("Packed clipboard bitmap is truncated".into());
    }

    let size = u32::try_from(dib.len() + 14)?;
    let mut bmp = Vec::with_capacity(size as usize);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&size.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&u32::try_from(offset + 14)?.to_le_bytes());
    bmp.extend_from_slice(dib);
    Ok(Some(bmp))
}

pub fn rgba_to_png(image: &RgbaImage) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut png = Vec::new();
    PngEncoder::new(&mut png).write_image(
        image,
        image.width(),
        image.height(),
        ExtendedColorType::Rgba8,
    )?;
    canonical_png(&png)?;
    Ok(png)
}

pub fn png_to_dib(bytes: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    let bytes = canonical_png(bytes)?;
    let image = DynamicImage::from_decoder(PngDecoder::new(Cursor::new(bytes))?)?.into_rgba8();
    Ok(encode_dib_v5(&image))
}

fn encode_dib_v5(image: &RgbaImage) -> Vec<u8> {
    const HEADER_SIZE: usize = 124;
    let mut dib = vec![0u8; HEADER_SIZE];
    dib.reserve(image.as_raw().len());
    dib[0..4].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
    dib[4..8].copy_from_slice(&(image.width() as i32).to_le_bytes());
    dib[8..12].copy_from_slice(&(-(image.height() as i32)).to_le_bytes());
    dib[12..14].copy_from_slice(&1u16.to_le_bytes());
    dib[14..16].copy_from_slice(&32u16.to_le_bytes());
    dib[16..20].copy_from_slice(&3u32.to_le_bytes()); // BI_BITFIELDS
    dib[20..24].copy_from_slice(&(image.as_raw().len() as u32).to_le_bytes());
    dib[40..44].copy_from_slice(&0x00ff_0000u32.to_le_bytes());
    dib[44..48].copy_from_slice(&0x0000_ff00u32.to_le_bytes());
    dib[48..52].copy_from_slice(&0x0000_00ffu32.to_le_bytes());
    dib[52..56].copy_from_slice(&0xff00_0000u32.to_le_bytes());
    dib[56..60].copy_from_slice(&0x7352_4742u32.to_le_bytes()); // LCS_sRGB
    dib[108..112].copy_from_slice(&4u32.to_le_bytes()); // LCS_GM_IMAGES

    for pixel in image.pixels() {
        let [red, green, blue, alpha] = pixel.0;
        dib.extend_from_slice(&[blue, green, red, alpha]);
    }
    dib
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dib_png_round_trip_preserves_alpha_and_row_order() {
        let pixels = [255, 0, 0, 128, 0, 255, 0, 0, 0, 0, 255, 255, 12, 34, 56, 91];
        let original = RgbaImage::from_raw(2, 2, pixels.to_vec()).unwrap();
        let png = dib_to_png(&encode_dib_v5(&original)).unwrap();
        let dib = png_to_dib(&png).unwrap();

        assert_eq!(dib, encode_dib_v5(&original));
        assert_eq!(
            canonical_png(&[png.as_slice(), &[0, 0]].concat()).unwrap(),
            png
        );
    }
}
