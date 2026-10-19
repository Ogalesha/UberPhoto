//! Analisi e formattazione di coordinate inserite manualmente.
//!
//! L'input accetta il punto o la virgola come separatore decimale (abitudine
//! italiana) e un segno opzionale; i limiti e i messaggi coincidono con la
//! validazione di [`uberphoto_core::GpsLocation`].

/// Asse a cui si riferisce un valore numerico.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Latitude,
    Longitude,
}

impl Axis {
    pub fn label(self) -> &'static str {
        match self {
            Self::Latitude => "latitudine",
            Self::Longitude => "longitudine",
        }
    }

    fn range(self) -> (f64, f64) {
        match self {
            Self::Latitude => (-90.0, 90.0),
            Self::Longitude => (-180.0, 180.0),
        }
    }
}

/// Interpreta un valore decimale in gradi; restituisce un errore in italiano
/// pronto per la UI.
pub fn parse_coordinate(input: &str, axis: Axis) -> Result<f64, String> {
    let testo = input.trim().replace(',', ".");
    if testo.is_empty() {
        return Err(format!("inserisci la {}", axis.label()));
    }
    let valore: f64 = testo.parse().map_err(|_| {
        format!(
            "{} non valida: usa i gradi decimali (es. 44.4949)",
            axis.label()
        )
    })?;
    if !valore.is_finite() {
        return Err(format!("{} non valida", axis.label()));
    }
    let (minimo, massimo) = axis.range();
    if !(minimo..=massimo).contains(&valore) {
        return Err(format!(
            "la {} va da {minimo:.0} a +{massimo:.0}",
            axis.label()
        ));
    }
    Ok(valore)
}

/// Interpreta l'altitudine in metri; il campo vuoto equivale a "non
/// impostata".
pub fn parse_altitude(input: &str) -> Result<Option<f64>, String> {
    let testo = input.trim().replace(',', ".");
    if testo.is_empty() {
        return Ok(None);
    }
    let valore: f64 = testo
        .parse()
        .map_err(|_| "altitudine non valida: usa i metri sul livello del mare".to_owned())?;
    if !valore.is_finite() {
        return Err("altitudine non valida".to_owned());
    }
    Ok(Some(valore))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_coordinate_accettano_punto_virgola_e_segno() {
        assert_eq!(
            parse_coordinate("44.4949", Axis::Latitude).unwrap(),
            44.4949
        );
        assert_eq!(
            parse_coordinate(" 44,4949 ", Axis::Latitude).unwrap(),
            44.4949
        );
        assert_eq!(
            parse_coordinate("-33.8688", Axis::Latitude).unwrap(),
            -33.8688
        );
        assert_eq!(parse_coordinate("+120.5", Axis::Longitude).unwrap(), 120.5);
    }

    #[test]
    fn le_coordinate_fuori_intervallo_vengono_rifiutate() {
        assert!(parse_coordinate("90.5", Axis::Latitude)
            .unwrap_err()
            .contains("da -90 a +90"));
        assert!(parse_coordinate("181", Axis::Longitude).is_err());
        assert!(parse_coordinate("", Axis::Latitude).is_err());
        assert!(parse_coordinate("nord", Axis::Latitude).is_err());
        assert!(parse_coordinate("NaN", Axis::Latitude).is_err());
    }

    #[test]
    fn l_altitudine_vuota_equivale_a_non_impostata() {
        assert_eq!(parse_altitude("").unwrap(), None);
        assert_eq!(parse_altitude(" 250,5 ").unwrap(), Some(250.5));
        assert_eq!(parse_altitude("-12").unwrap(), Some(-12.0));
        assert!(parse_altitude("su").is_err());
    }
}
