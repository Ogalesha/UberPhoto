//! Conversione colore verso sRGB con Little CMS.

use lcms2::{InfoType, Intent, Locale, PixelFormat, Profile, Transform};

use crate::error::ExportError;

/// Profilo ICC sRGB generato da Little CMS.
pub fn profilo_srgb() -> Vec<u8> {
    Profile::new_srgb().icc().unwrap_or_else(|_| Vec::new())
}

/// Converte i pixel RGB in sRGB secondo il profilo ICC di origine.
///
/// `rgba` è il buffer RGBA8 della fotografia; l'alfa non viene toccato.
/// Ritorna `true` se è avvenuta una conversione reale. I profili non RGB
/// (es. CMYK) non sono convertibili qui e vengono ignorati con un avviso.
pub fn converti_in_srgb(profilo: &[u8], rgba: &mut [u8]) -> Result<bool, ExportError> {
    if profilo.is_empty() || rgba.len() % 4 != 0 {
        return Ok(false);
    }
    let origine = Profile::new_icc(profilo)
        .map_err(|err| ExportError::Color(format!("profilo ICC illeggibile: {err}")))?;
    let descrizione = origine
        .info(InfoType::Description, Locale::none())
        .unwrap_or_default()
        .to_lowercase();
    if descrizione.contains("srgb") {
        return Ok(false);
    }
    if origine.color_space() != lcms2::ColorSpaceSignature::RgbData {
        return Ok(false);
    }

    let destinazione = Profile::new_srgb();
    let trasformazione = Transform::new(
        &origine,
        PixelFormat::RGB_8,
        &destinazione,
        PixelFormat::RGB_8,
        Intent::Perceptual,
    )
    .map_err(|err| ExportError::Color(format!("trasformazione ICC non riuscita: {err}")))?;

    let pixel = rgba.len() / 4;
    let mut rgb = Vec::with_capacity(pixel * 3);
    for chunk in rgba.chunks_exact(4) {
        rgb.extend_from_slice(&chunk[..3]);
    }
    let mut convertito = vec![0u8; pixel * 3];
    trasformazione.transform_pixels(&rgb, &mut convertito);

    for (indice, chunk) in rgba.chunks_exact_mut(4).enumerate() {
        chunk[..3].copy_from_slice(&convertito[indice * 3..indice * 3 + 3]);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn il_profilo_srgb_generato_e_valido() {
        let bytes = profilo_srgb();
        assert!(bytes.len() > 100, "profilo sRGB vuoto");
        let profilo = Profile::new_icc(&bytes).unwrap();
        let descrizione = profilo
            .info(InfoType::Description, Locale::none())
            .unwrap_or_default()
            .to_lowercase();
        assert!(descrizione.contains("srgb"), "{descrizione}");
    }

    #[test]
    fn un_profilo_assente_non_cambia_i_pixel() {
        let mut rgba = vec![10, 20, 30, 255, 40, 50, 60, 128];
        let prima = rgba.clone();
        assert!(!converti_in_srgb(&[], &mut rgba).unwrap());
        assert_eq!(rgba, prima);
    }

    #[test]
    fn il_profilo_srgb_non_riconverte_i_pixel() {
        let mut rgba = vec![10, 20, 30, 255, 40, 50, 60, 128];
        let prima = rgba.clone();
        assert!(!converti_in_srgb(&profilo_srgb(), &mut rgba).unwrap());
        assert_eq!(rgba, prima);
    }
}
