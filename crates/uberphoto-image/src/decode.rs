use std::path::Path;

use image::{ImageReader, RgbaImage};

use crate::orientation::apply_orientation;

/// Immagine decodificata, orientata e (opzionalmente) scalata.
///
/// `source_*` riporta le dimensioni reali della fotografia (dopo
/// l'orientamento); `width`/`height` quelle del buffer RGBA restituito.
#[derive(Debug)]
pub struct LoadedImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub source_width: u32,
    pub source_height: u32,
}

/// Decodifica `path` in RGBA, applica l'orientamento EXIF e, se `max_dim`
/// è specificato, riduce l'immagine perché il lato maggiore non superi
/// quel valore (nessun ingrandimento).
///
/// Gli errori sono stringhe in italiano, pronte per la UI.
pub fn load_image_rgba(path: &Path, max_dim: Option<u32>) -> Result<LoadedImage, String> {
    let reader = ImageReader::open(path)
        .map_err(|err| format!("impossibile aprire il file: {err}"))?
        .with_guessed_format()
        .map_err(|err| format!("formato non riconosciuto: {err}"))?;

    let decoded = reader
        .decode()
        .map_err(|err| format!("decodifica fallita: {err}"))?;

    let oriented = apply_orientation(decoded.into_rgba8(), read_orientation(path));
    let (source_width, source_height) = oriented.dimensions();

    let final_image = match max_dim {
        Some(max) => downscale(oriented, max),
        None => oriented,
    };
    let (width, height) = final_image.dimensions();

    Ok(LoadedImage {
        rgba: final_image.into_raw(),
        width,
        height,
        source_width,
        source_height,
    })
}

/// Decodifica un'immagine in memoria (es. una risorsa incorporata) in RGBA,
/// senza lettura dell'orientamento EXIF, con riduzione opzionale.
pub fn decode_bytes_rgba(bytes: &[u8], max_dim: Option<u32>) -> Result<LoadedImage, String> {
    let decoded = image::load_from_memory(bytes)
        .map_err(|err| format!("decodifica fallita: {err}"))?
        .into_rgba8();
    let (source_width, source_height) = decoded.dimensions();

    let final_image = match max_dim {
        Some(max) => downscale(decoded, max),
        None => decoded,
    };
    let (width, height) = final_image.dimensions();

    Ok(LoadedImage {
        rgba: final_image.into_raw(),
        width,
        height,
        source_width,
        source_height,
    })
}

/// Riduzione con filtro box (`imageops::thumbnail`): veloce e adeguata alle
/// miniature; per le anteprime grandi verrà valutato un filtro triangolare.
fn downscale(image: RgbaImage, max_dim: u32) -> RgbaImage {
    let (width, height) = image.dimensions();
    let largest = width.max(height);
    if largest <= max_dim {
        return image;
    }
    let scale = max_dim as f32 / largest as f32;
    let target_width = ((width as f32 * scale).round() as u32).max(1);
    let target_height = ((height as f32 * scale).round() as u32).max(1);
    image::imageops::thumbnail(&image, target_width, target_height)
}

/// Legge il tag EXIF `Orientation`; restituisce 1 (nessuna trasformazione)
/// se assente o non leggibile.
///
/// Questo è l'unico uso di un parser EXIF interno alla pipeline immagini:
/// la lettura/scrittura completa dei metadati passa da ExifTool (Fase 3).
fn read_orientation(path: &Path) -> u16 {
    use exif::{In, Reader, Tag};

    let Ok(file) = std::fs::File::open(path) else {
        return 1;
    };
    let mut buffered = std::io::BufReader::new(file);
    let Ok(exif) = Reader::new().read_from_container(&mut buffered) else {
        return 1;
    };
    exif.get_field(Tag::Orientation, In::PRIMARY)
        .and_then(|field| field.value.get_uint(0))
        .map(|value| value as u16)
        .filter(|value| (1..=8).contains(value))
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodifica_e_scala_rispettando_le_proporzioni() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("test.png");

        let image = RgbaImage::from_fn(400, 200, |x, _y| image::Rgba([x as u8, 0, 0, 255]));
        image.save(&path).expect("salvataggio png");

        let loaded = load_image_rgba(&path, Some(100)).expect("decodifica");
        assert_eq!((loaded.width, loaded.height), (100, 50));
        assert_eq!((loaded.source_width, loaded.source_height), (400, 200));
        assert_eq!(loaded.rgba.len(), 100 * 50 * 4);

        let full = load_image_rgba(&path, None).expect("decodifica completa");
        assert_eq!((full.width, full.height), (400, 200));
    }

    #[test]
    fn nessun_ingrandimento_se_max_dim_e_maggiore() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("piccola.png");

        RgbaImage::from_fn(50, 40, |_x, _y| image::Rgba([0, 0, 0, 255]))
            .save(&path)
            .expect("salvataggio png");

        let loaded = load_image_rgba(&path, Some(512)).expect("decodifica");
        assert_eq!((loaded.width, loaded.height), (50, 40));
    }

    #[test]
    fn errore_leggibile_per_file_non_immagine() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("rotto.jpg");
        std::fs::write(&path, b"non sono un jpeg").unwrap();

        let err = load_image_rgba(&path, None).expect_err("deve fallire");
        assert!(err.contains("decodifica fallita"), "errore inatteso: {err}");
    }

    #[test]
    fn decodifica_da_memoria_con_riduzione() {
        let image = RgbaImage::from_fn(20, 12, |_x, _y| image::Rgba([1, 2, 3, 255]));
        let mut buffer = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut buffer, image::ImageFormat::Png)
            .expect("codifica png in memoria");

        let loaded = decode_bytes_rgba(buffer.get_ref(), Some(10)).expect("decodifica");
        assert_eq!((loaded.width, loaded.height), (10, 6));
        assert_eq!((loaded.source_width, loaded.source_height), (20, 12));
        assert_eq!(loaded.rgba.len(), 10 * 6 * 4);
    }
}
