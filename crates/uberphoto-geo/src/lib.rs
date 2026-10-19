//! Geolocalizzazione (Fase 4).
//!
//! - coordinate canoniche [`uberphoto_core::GpsLocation`] (validate nel
//!   crate `uberphoto-core`);
//! - analisi dei valori inseriti manualmente (`parse_coordinate`,
//!   `parse_altitude`);
//! - ricerca geografica opzionale tramite Nominatim nel rispetto delle sue
//!   policy: niente autocomplete, rate limiting a una richiesta al secondo,
//!   User-Agent identificativo, cache locale persistente;
//! - [`GeoService`] su thread dedicato per non bloccare la UI.
//!
//! La mappa OpenStreetMap interattiva vive nel crate `uberphoto-ui`
//! (dipendenza `walkers`, che è a sua volta basata su egui).

mod nominatim;
mod parse;
mod service;

pub use nominatim::{
    default_user_agent, AddressDetails, GeoError, GeocodeResult, LocationSuggestion,
    NominatimClient, DEFAULT_BASE_URL,
};
pub use parse::{parse_altitude, parse_coordinate, Axis};
pub use service::{GeoOutcome, GeoRequest, GeoService};
