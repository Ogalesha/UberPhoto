use image::RgbaImage;

/// Applica l'orientamento EXIF (tag 1..=8) a un'immagine RGBA.
///
/// I valori 5 e 7 (trasposta e trasversa) non hanno un equivalente diretto
/// in `image::imageops`, quindi sono implementati pixel per pixel; per
/// un'immagine di anteprima il costo è trascurabile.
pub fn apply_orientation(image: RgbaImage, orientation: u16) -> RgbaImage {
    use image::imageops;

    match orientation {
        2 => imageops::flip_horizontal(&image),
        3 => imageops::rotate180(&image),
        4 => imageops::flip_vertical(&image),
        5 => transpose(&image),
        6 => imageops::rotate90(&image),
        7 => transverse(&image),
        8 => imageops::rotate270(&image),
        _ => image,
    }
}

/// Orientamento EXIF 5: riflessione lungo la diagonale principale.
fn transpose(image: &RgbaImage) -> RgbaImage {
    let (width, height) = image.dimensions();
    let mut out = RgbaImage::new(height, width);
    for y in 0..height {
        for x in 0..width {
            out.put_pixel(y, x, *image.get_pixel(x, y));
        }
    }
    out
}

/// Orientamento EXIF 7: riflessione lungo la diagonale secondaria.
fn transverse(image: &RgbaImage) -> RgbaImage {
    let (width, height) = image.dimensions();
    let mut out = RgbaImage::new(height, width);
    for y in 0..height {
        for x in 0..width {
            out.put_pixel(height - 1 - y, width - 1 - x, *image.get_pixel(x, y));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// 3x2 con un valore distinto per pixel: il canale rosso è l'indice.
    fn sample() -> RgbaImage {
        RgbaImage::from_fn(3, 2, |x, y| Rgba([(y * 3 + x) as u8, 0, 0, 255]))
    }

    #[test]
    fn orientamento_1_non_modifica() {
        let src = sample();
        let out = apply_orientation(src.clone(), 1);
        assert_eq!(out, src);
    }

    #[test]
    fn orientamento_sconosciuto_non_modifica() {
        let src = sample();
        let out = apply_orientation(src.clone(), 9);
        assert_eq!(out, src);
    }

    #[test]
    fn orientamento_6_equivale_a_rotazione_90_oraria() {
        let src = sample();
        let out = apply_orientation(src.clone(), 6);
        assert_eq!(out.dimensions(), (2, 3));
        // EXIF 6: il pixel (x, y) va in (h-1-y, x).
        for y in 0..src.height() {
            for x in 0..src.width() {
                assert_eq!(
                    *out.get_pixel(src.height() - 1 - y, x),
                    *src.get_pixel(x, y)
                );
            }
        }
    }

    #[test]
    fn orientamento_8_equivale_a_rotazione_90_antioraria() {
        let src = sample();
        let out = apply_orientation(src.clone(), 8);
        assert_eq!(out.dimensions(), (2, 3));
        // EXIF 8: il pixel (x, y) va in (y, w-1-x).
        for y in 0..src.height() {
            for x in 0..src.width() {
                assert_eq!(*out.get_pixel(y, src.width() - 1 - x), *src.get_pixel(x, y));
            }
        }
    }

    #[test]
    fn orientamento_3_ruota_di_180() {
        let src = sample();
        let out = apply_orientation(src.clone(), 3);
        assert_eq!(out.dimensions(), (3, 2));
        for y in 0..src.height() {
            for x in 0..src.width() {
                assert_eq!(
                    *out.get_pixel(src.width() - 1 - x, src.height() - 1 - y),
                    *src.get_pixel(x, y)
                );
            }
        }
    }

    #[test]
    fn orientamenti_5_e_7_differiscono_tra_loro() {
        let src = sample();
        let five = apply_orientation(src.clone(), 5);
        let seven = apply_orientation(src, 7);
        assert_eq!(five.dimensions(), seven.dimensions());
        assert_ne!(five.into_raw(), seven.into_raw());
    }
}
