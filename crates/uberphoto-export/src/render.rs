//! Rendering dei pixel: decodifica, profilo ICC, ricetta, ridimensionamento
//! e codifica nel formato scelto.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::codecs::tiff::TiffEncoder;
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, ImageEncoder, RgbaImage};
use uberphoto_image::{apply_recipe_rgba, load_image_rgba, EditRecipe};

use crate::error::ExportError;
use crate::icc::converti_in_srgb;
use crate::settings::{ExportFormat, ExportSettings};

/// Esito del rendering di un singolo file.
#[derive(Debug)]
pub struct RenderOutput {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub converted_profile: bool,
    pub resized: bool,
}

/// Applica la ricetta e scrive il file esportato (senza metadati).
///
/// L'immagine viene riportata in sRGB quando il profilo ICC di origine non lo
/// è, poi la ricetta lavora in sRGB; infine ridimensionamento e codifica.
pub fn render_export(
    source: &Path,
    settings: &ExportSettings,
    recipe: &EditRecipe,
    profilo_icc: Option<&[u8]>,
) -> Result<RenderOutput, ExportError> {
    let loaded = load_image_rgba(source, None).map_err(ExportError::Image)?;
    let mut rgba = loaded.rgba;
    let width = loaded.width;
    let height = loaded.height;

    let converted_profile = match profilo_icc {
        Some(profilo) => converti_in_srgb(profilo, &mut rgba)?,
        None => false,
    };

    let regolata = apply_recipe_rgba(&rgba, width, height, recipe);

    let (final_width, final_height) = settings
        .resize
        .target_size(regolata.width, regolata.height)
        .unwrap_or((regolata.width, regolata.height));
    let resized = (final_width, final_height) != (regolata.width, regolata.height);
    let pixel = if resized {
        let immagine = RgbaImage::from_raw(regolata.width, regolata.height, regolata.rgba)
            .ok_or_else(|| ExportError::Invalid("buffer RGBA non coerente".to_owned()))?;
        image::imageops::resize(&immagine, final_width, final_height, FilterType::Lanczos3)
    } else {
        RgbaImage::from_raw(regolata.width, regolata.height, regolata.rgba)
            .ok_or_else(|| ExportError::Invalid("buffer RGBA non coerente".to_owned()))?
    };

    let destination = settings.destination_for(source);
    std::fs::create_dir_all(&destination)?;
    let nome = settings
        .naming
        .file_name(source, settings.format.extension());
    let path = percorso_unico(&destination, &nome);
    codifica(&path, &pixel, settings)?;

    let bytes = std::fs::metadata(&path)?.len();
    Ok(RenderOutput {
        path,
        width: final_width,
        height: final_height,
        bytes,
        converted_profile,
        resized,
    })
}

/// Evita di sovrascrivere file esistenti aggiungendo `-2`, `-3`, …
fn percorso_unico(directory: &Path, nome: &str) -> PathBuf {
    let candidate = directory.join(nome);
    if !candidate.exists() {
        return candidate;
    }
    let percorso = Path::new(nome);
    let stem = percorso
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "foto".to_owned());
    let estensione = percorso
        .extension()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    for indice in 2..10_000 {
        let nome = if estensione.is_empty() {
            format!("{stem}-{indice}")
        } else {
            format!("{stem}-{indice}.{estensione}")
        };
        let candidate = directory.join(nome);
        if !candidate.exists() {
            return candidate;
        }
    }
    candidate
}

fn codifica(path: &Path, pixel: &RgbaImage, settings: &ExportSettings) -> Result<(), ExportError> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    let (width, height) = pixel.dimensions();

    match settings.format {
        ExportFormat::Jpeg => {
            let rgb = image::RgbImage::from_fn(width, height, |x, y| {
                let p = pixel.get_pixel(x, y);
                image::Rgb([p[0], p[1], p[2]])
            });
            let qualita = settings.quality.clamp(1, 100);
            let mut encoder = JpegEncoder::new_with_quality(&mut writer, qualita);
            encoder
                .encode(&rgb, width, height, ExtendedColorType::Rgb8)
                .map_err(|err| ExportError::Image(err.to_string()))?;
        }
        ExportFormat::Png => {
            let encoder = PngEncoder::new_with_quality(
                &mut writer,
                CompressionType::Best,
                PngFilter::Adaptive,
            );
            encoder
                .write_image(pixel, width, height, ExtendedColorType::Rgba8)
                .map_err(|err| ExportError::Image(err.to_string()))?;
        }
        ExportFormat::Webp => {
            let encoder = WebPEncoder::new_lossless(&mut writer);
            encoder
                .write_image(pixel, width, height, ExtendedColorType::Rgba8)
                .map_err(|err| ExportError::Image(err.to_string()))?;
        }
        ExportFormat::Tiff => {
            let encoder = TiffEncoder::new(&mut writer);
            encoder
                .write_image(pixel, width, height, ExtendedColorType::Rgba8)
                .map_err(|err| ExportError::Image(err.to_string()))?;
        }
    }

    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{NameMode, Naming};

    fn crea_foto(path: &Path, width: u32, height: u32) {
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        });
        image
            .save_with_format(path, image::ImageFormat::Jpeg)
            .unwrap();
    }

    #[test]
    fn esporta_jpeg_ridimensionato_e_non_sovrascrive() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("scatto.jpg");
        crea_foto(&source, 400, 300);
        let destinazione = dir.path().join("web");
        let settings = ExportSettings {
            destination: destinazione.clone(),
            resize: crate::settings::ResizeRules {
                enabled: true,
                max_width: 200,
                max_height: 200,
                keep_aspect: true,
            },
            naming: Naming {
                mode: NameMode::KeepName,
                ..Naming::default()
            },
            ..ExportSettings::default()
        };

        let primo = render_export(&source, &settings, &EditRecipe::default(), None).unwrap();
        assert_eq!((primo.width, primo.height), (200, 150));
        assert_eq!(primo.path, destinazione.join("scatto.jpg"));
        assert!(primo.bytes > 0);

        let secondo = render_export(&source, &settings, &EditRecipe::default(), None).unwrap();
        assert_eq!(secondo.path, destinazione.join("scatto-2.jpg"));
        assert!(secondo.resized);
        assert!(!secondo.converted_profile);
    }

    #[test]
    fn esporta_png_e_webp_con_la_ricetta_applicata() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("foto.jpg");
        crea_foto(&source, 40, 30);
        let recipe = EditRecipe {
            rotation: 90,
            ..EditRecipe::default()
        };

        let png = ExportSettings {
            destination: dir.path().join("out"),
            format: ExportFormat::Png,
            resize: crate::settings::ResizeRules {
                enabled: false,
                ..Default::default()
            },
            ..ExportSettings::default()
        };
        let risultato = render_export(&source, &png, &recipe, None).unwrap();
        assert_eq!((risultato.width, risultato.height), (30, 40));
        let riaperta = image::open(&risultato.path).unwrap();
        assert_eq!(riaperta.width(), 30);

        let webp = ExportSettings {
            format: ExportFormat::Webp,
            ..png.clone()
        };
        let risultato = render_export(&source, &webp, &recipe, None).unwrap();
        assert!(risultato.path.extension().unwrap() == "webp");
        assert!(image::open(&risultato.path).is_ok());
    }
}
