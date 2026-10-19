//! Coordinate GPS canoniche, indipendenti dalla UI e dai servizi esterni.
//!
//! La validazione avviene alla costruzione: una `GpsLocation` esiste solo se
//! latitudine, longitudine e (quando presente) altitudine sono valori finiti
//! e nei limiti dello standard.

use std::fmt;

/// Coordinate geografiche in gradi decimali; latitudine a sud e longitudine
/// a ovest sono negative. L'altitudine è in metri sul livello del mare.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GpsLocation {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: Option<f64>,
}

/// Errore di validazione delle coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpsError {
    LatitudeOutOfRange,
    LongitudeOutOfRange,
    NotFinite,
    AltitudeNotFinite,
}

impl fmt::Display for GpsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LatitudeOutOfRange => f.write_str("la latitudine va da -90 a +90"),
            Self::LongitudeOutOfRange => f.write_str("la longitudine va da -180 a +180"),
            Self::NotFinite => f.write_str("le coordinate devono essere numeri finiti"),
            Self::AltitudeNotFinite => f.write_str("l'altitudine deve essere un numero finito"),
        }
    }
}

impl std::error::Error for GpsError {}

impl GpsLocation {
    /// Crea coordinate validate.
    pub fn new(latitude: f64, longitude: f64, altitude: Option<f64>) -> Result<Self, GpsError> {
        if !latitude.is_finite() || !longitude.is_finite() {
            return Err(GpsError::NotFinite);
        }
        if !(-90.0..=90.0).contains(&latitude) {
            return Err(GpsError::LatitudeOutOfRange);
        }
        if !(-180.0..=180.0).contains(&longitude) {
            return Err(GpsError::LongitudeOutOfRange);
        }
        if let Some(altitude) = altitude {
            if !altitude.is_finite() {
                return Err(GpsError::AltitudeNotFinite);
            }
        }
        Ok(Self {
            latitude,
            longitude,
            altitude,
        })
    }

    /// True se le coordinate sono valide (per dati deserializzati o letti
    /// da fonti esterne).
    pub fn is_valid(&self) -> bool {
        Self::new(self.latitude, self.longitude, self.altitude).is_ok()
    }

    /// Coordinate formattate per la UI: `44.494900°, 11.342600°`.
    pub fn format_decimal(&self) -> String {
        format!("{:.6}, {:.6}", self.latitude, self.longitude)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_coordinate_valide_vengono_accettate() {
        let location = GpsLocation::new(44.4949, 11.3426, Some(250.0)).unwrap();
        assert_eq!(location.altitude, Some(250.0));
        assert!(GpsLocation::new(-90.0, 180.0, None).is_ok());
        assert!(GpsLocation::new(-33.8688, -70.6693, None).is_ok());
    }

    #[test]
    fn i_valori_fuori_intervallo_vengono_rifiutati() {
        assert_eq!(
            GpsLocation::new(90.0001, 0.0, None).unwrap_err(),
            GpsError::LatitudeOutOfRange
        );
        assert_eq!(
            GpsLocation::new(0.0, -180.5, None).unwrap_err(),
            GpsError::LongitudeOutOfRange
        );
        assert_eq!(
            GpsLocation::new(f64::NAN, 0.0, None).unwrap_err(),
            GpsError::NotFinite
        );
        assert_eq!(
            GpsLocation::new(0.0, 0.0, Some(f64::INFINITY)).unwrap_err(),
            GpsError::AltitudeNotFinite
        );
    }

    #[test]
    fn la_formattazione_usa_sei_decimali() {
        let location = GpsLocation::new(44.4949, 11.3426, None).unwrap();
        assert_eq!(location.format_decimal(), "44.494900, 11.342600");
    }
}
