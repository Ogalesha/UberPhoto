//! Geolocalizzazione (Fase 4).
//!
//! Questo crate ospiterà:
//! - il tipo `GpsLocation` (con validazione di latitudine/longitudine/altitudine);
//! - l'integrazione della mappa OpenStreetMap tramite `walkers`;
//! - la ricerca geografica opzionale (Nominatim) nel rispetto dei limiti
//!   del servizio: niente autocomplete, rate limiting, User-Agent
//!   identificativo, cache locale;
//! - il geotagging multiplo.
//!
//! Stato attuale: scaffolding, nessuna funzionalità implementata.
