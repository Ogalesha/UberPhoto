//! Client Nominatim (OpenStreetMap) con rate limiting, cache locale e
//! User-Agent identificativo, come richiesto dalle policy del servizio.
//!
//! Vincoli rispettati:
//! - niente autocomplete: le richieste partono solo su azione esplicita;
//! - al massimo una richiesta al secondo verso la rete;
//! - User-Agent identificativo con il sito del progetto;
//! - cache locale persistente su disco, così le stesse ricerche non si
//!   ripetono tra sessioni.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uberphoto_core::GpsLocation;

/// Endpoint pubblico di Nominatim.
pub const DEFAULT_BASE_URL: &str = "https://nominatim.openstreetmap.org";
/// Voci di cache mantenute su disco.
const MAX_CACHE_ENTRIES: usize = 400;
/// Risultati richiesti per la ricerca (niente query massive).
const SEARCH_LIMIT: usize = 5;

/// User-Agent identificativo richiesto dalle usage policy.
pub fn default_user_agent() -> String {
    format!(
        "UberPhoto/{} (+{})",
        env!("CARGO_PKG_VERSION"),
        "https://github.com/Ogalesha/UberPhoto"
    )
}

/// Errore del servizio geografico, già in forma leggibile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeoError {
    Network(String),
    RateLimited,
    Service(String),
    Cache(String),
}

impl std::fmt::Display for GeoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(message) => write!(f, "richiesta non riuscita: {message}"),
            Self::RateLimited => {
                f.write_str("Nominatim ha limitato le richieste: attendi qualche secondo e riprova")
            }
            Self::Service(message) => write!(f, "risposta non valida da Nominatim: {message}"),
            Self::Cache(message) => write!(f, "cache geografica non utilizzabile: {message}"),
        }
    }
}

impl std::error::Error for GeoError {}

/// Risultato di geocoding (ricerca o reverse), già normalizzato.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeocodeResult {
    pub display_name: String,
    pub latitude: f64,
    pub longitude: f64,
    /// Nome del luogo/POS, quando Nominatim lo fornisce (jsonv2).
    pub name: Option<String>,
    pub address: AddressDetails,
}

/// Componenti dell'indirizzo usate per compilare i campi di località.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AddressDetails {
    #[serde(default)]
    pub city: Option<String>,
    #[serde(default)]
    pub town: Option<String>,
    #[serde(default)]
    pub village: Option<String>,
    #[serde(default)]
    pub municipality: Option<String>,
    #[serde(default)]
    pub hamlet: Option<String>,
    #[serde(default)]
    pub suburb: Option<String>,
    #[serde(default)]
    pub neighbourhood: Option<String>,
    #[serde(default)]
    pub county: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub country_code: Option<String>,
}

/// Proposta di valori per i campi della scheda GPS (località di scatto).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocationSuggestion {
    pub place: Option<String>,
    pub sublocation: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub country_code: Option<String>,
}

impl GeocodeResult {
    pub fn location(&self) -> GpsLocation {
        GpsLocation {
            latitude: self.latitude,
            longitude: self.longitude,
            altitude: None,
        }
    }

    /// Euristica di mappatura verso i campi IPTC della località.
    pub fn location_suggestion(&self) -> LocationSuggestion {
        let address = &self.address;
        LocationSuggestion {
            place: self.name.clone(),
            sublocation: address
                .suburb
                .clone()
                .or_else(|| address.neighbourhood.clone())
                .or_else(|| address.hamlet.clone()),
            city: address
                .city
                .clone()
                .or_else(|| address.town.clone())
                .or_else(|| address.village.clone())
                .or_else(|| address.municipality.clone()),
            state: address.state.clone().or_else(|| address.county.clone()),
            country: address.country.clone(),
            country_code: address
                .country_code
                .as_deref()
                .map(str::trim)
                .filter(|code| !code.is_empty())
                .map(str::to_ascii_uppercase),
        }
    }
}

// ---------------------------------------------------------------------------
// Deserializzazione delle risposte jsonv2
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RawResult {
    #[serde(default)]
    display_name: String,
    lat: String,
    lon: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    address: AddressDetails,
}

fn parse_result(raw: RawResult) -> Option<GeocodeResult> {
    let latitude = raw.lat.trim().parse::<f64>().ok()?;
    let longitude = raw.lon.trim().parse::<f64>().ok()?;
    if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
        return None;
    }
    Some(GeocodeResult {
        display_name: raw.display_name,
        latitude,
        longitude,
        name: raw.name.filter(|name| !name.trim().is_empty()),
        address: raw.address,
    })
}

// ---------------------------------------------------------------------------
// Cache persistente
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CachedResponse {
    Search(Vec<GeocodeResult>),
    Reverse(Option<Box<GeocodeResult>>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    fetched: u64,
    response: CachedResponse,
}

fn adesso() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn carica_cache(path: &Path) -> HashMap<String, CacheEntry> {
    let Ok(testo) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str(&testo).unwrap_or_default()
}

fn salva_cache(path: &Path, cache: &HashMap<String, CacheEntry>) -> Result<(), GeoError> {
    let testo = serde_json::to_string(cache).map_err(|err| GeoError::Cache(err.to_string()))?;
    let temporaneo = path.with_extension("tmp");
    std::fs::write(&temporaneo, testo).map_err(|err| GeoError::Cache(err.to_string()))?;
    std::fs::rename(&temporaneo, path).map_err(|err| GeoError::Cache(err.to_string()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Client sincrono per Nominatim: pensato per girare su un thread dedicato.
pub struct NominatimClient {
    agent: ureq::Agent,
    base_url: String,
    /// Intervallo minimo tra due richieste di rete (policy: 1 al secondo).
    min_interval: Duration,
    last_request: Mutex<Option<Instant>>,
    cache: Mutex<HashMap<String, CacheEntry>>,
    cache_path: Option<PathBuf>,
}

impl NominatimClient {
    /// Client predefinito verso l'endpoint pubblico.
    pub fn new(cache_path: Option<PathBuf>) -> Self {
        Self::with_base_url(DEFAULT_BASE_URL, cache_path)
    }

    /// Client verso un endpoint alternativo (usato dai test e da istanze
    /// locali di Nominatim).
    pub fn with_base_url(base_url: &str, cache_path: Option<PathBuf>) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .user_agent(default_user_agent())
            .build();
        let cache = cache_path.as_deref().map(carica_cache).unwrap_or_default();
        Self {
            agent: config.into(),
            base_url: base_url.trim_end_matches('/').to_owned(),
            min_interval: Duration::from_secs(1),
            last_request: Mutex::new(None),
            cache: Mutex::new(cache),
            cache_path,
        }
    }

    /// Ricerca per testo libero; solo su richiesta esplicita dell'utente.
    pub fn search(&self, query: &str) -> Result<Vec<GeocodeResult>, GeoError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let key = format!("search:{}", query.to_lowercase());

        if let Some(CachedResponse::Search(results)) = self.cached(&key) {
            return Ok(results);
        }

        self.throttle();
        let mut response = self
            .agent
            .get(format!("{}/search", self.base_url))
            .query("q", query)
            .query("format", "jsonv2")
            .query("limit", SEARCH_LIMIT.to_string())
            .query("addressdetails", "1")
            .call()
            .map_err(errore_rete)?;
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|err| GeoError::Network(err.to_string()))?;
        let raw: Vec<RawResult> =
            serde_json::from_str(&body).map_err(|err| GeoError::Service(err.to_string()))?;
        let results: Vec<GeocodeResult> = raw.into_iter().filter_map(parse_result).collect();

        self.memorizza(key, CachedResponse::Search(results.clone()));
        Ok(results)
    }

    /// Reverse geocoding della posizione indicata.
    pub fn reverse(
        &self,
        latitude: f64,
        longitude: f64,
    ) -> Result<Option<GeocodeResult>, GeoError> {
        let key = format!("reverse:{latitude:.6},{longitude:.6}");

        if let Some(CachedResponse::Reverse(result)) = self.cached(&key) {
            return Ok(result.map(|result| *result));
        }

        self.throttle();
        let mut response = self
            .agent
            .get(format!("{}/reverse", self.base_url))
            .query("lat", latitude.to_string())
            .query("lon", longitude.to_string())
            .query("format", "jsonv2")
            .query("zoom", "18")
            .query("addressdetails", "1")
            .call()
            .map_err(errore_rete)?;
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|err| GeoError::Network(err.to_string()))?;

        // Il reverse di Nominatim risponde con un oggetto errore JSON quando
        // non trova nulla ("Unable to geocode") e codice 200.
        let value: serde_json::Value =
            serde_json::from_str(&body).map_err(|err| GeoError::Service(err.to_string()))?;
        let result = if value.get("error").is_some() {
            None
        } else {
            let raw: RawResult =
                serde_json::from_value(value).map_err(|err| GeoError::Service(err.to_string()))?;
            parse_result(raw)
        };

        self.memorizza(key, CachedResponse::Reverse(result.clone().map(Box::new)));
        Ok(result)
    }

    fn cached(&self, key: &str) -> Option<CachedResponse> {
        let cache = self.cache.lock().ok()?;
        cache.get(key).map(|entry| entry.response.clone())
    }

    fn memorizza(&self, key: String, response: CachedResponse) {
        let Ok(mut cache) = self.cache.lock() else {
            return;
        };
        cache.insert(
            key,
            CacheEntry {
                fetched: adesso(),
                response,
            },
        );
        if cache.len() > MAX_CACHE_ENTRIES {
            let mut voci: Vec<(String, u64)> = cache
                .iter()
                .map(|(key, entry)| (key.clone(), entry.fetched))
                .collect();
            voci.sort_by_key(|(_, fetched)| *fetched);
            let eccesso = cache.len() - MAX_CACHE_ENTRIES;
            for (key, _) in voci.into_iter().take(eccesso) {
                cache.remove(&key);
            }
        }
        if let Some(path) = &self.cache_path {
            if let Err(err) = salva_cache(path, &cache) {
                tracing::warn!("{err}");
            }
        }
    }

    /// Attende il tempo necessario a rispettare l'intervallo minimo tra
    /// richieste di rete.
    fn throttle(&self) {
        let Ok(mut last) = self.last_request.lock() else {
            return;
        };
        if let Some(previous) = *last {
            let elapsed = previous.elapsed();
            if elapsed < self.min_interval {
                std::thread::sleep(self.min_interval - elapsed);
            }
        }
        *last = Some(Instant::now());
    }
}

fn errore_rete(err: ureq::Error) -> GeoError {
    match err {
        ureq::Error::StatusCode(429) | ureq::Error::StatusCode(403) => GeoError::RateLimited,
        other => GeoError::Network(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Server HTTP minimale per i test: risponde con un JSON fisso e conta
    /// le richieste ricevute.
    fn server_finto(
        risposta: &'static str,
    ) -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let porta = listener.local_addr().unwrap().port();
        let richieste = Arc::new(AtomicUsize::new(0));
        let contatore = richieste.clone();
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                // Senza timeout il thread di test resta appeso alla chiusura.
                let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                contatore.fetch_add(1, Ordering::SeqCst);
                let body = risposta;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (format!("http://127.0.0.1:{porta}"), richieste, handle)
    }

    #[test]
    fn la_ricerca_interpreta_la_risposta_jsonv2() {
        let (base, richieste, _server) = server_finto(
            r#"[{"display_name":"Bologna, Emilia-Romagna, Italia","lat":"44.4937","lon":"11.3430",
                 "name":"Bologna","address":{"city":"Bologna","state":"Emilia-Romagna",
                 "country":"Italia","country_code":"it"}}]"#,
        );
        let client = NominatimClient::with_base_url(&base, None);
        let risultati = client.search("Bologna").unwrap();
        assert_eq!(richieste.load(Ordering::SeqCst), 1);
        assert_eq!(risultati.len(), 1);
        let primo = &risultati[0];
        assert!((primo.latitude - 44.4937).abs() < 1e-9);
        assert!((primo.longitude - 11.3430).abs() < 1e-9);
        assert_eq!(
            primo.location_suggestion().country_code.as_deref(),
            Some("IT")
        );
        assert_eq!(primo.location_suggestion().city.as_deref(), Some("Bologna"));

        // La seconda ricerca identica usa la cache: nessuna nuova richiesta.
        let ripetuta = client.search("Bologna").unwrap();
        assert_eq!(ripetuta, risultati);
        assert_eq!(richieste.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn il_reverse_gestisce_la_risposta_errore() {
        let (base, _, _server) = server_finto(r#"{"error":"Unable to geocode"}"#);
        let client = NominatimClient::with_base_url(&base, None);
        assert_eq!(client.reverse(0.0, 0.0).unwrap(), None);
    }

    #[test]
    fn il_rate_limiting_distanzia_le_richieste() {
        let (base, _, _server) = server_finto(r#"[]"#);
        let mut client = NominatimClient::with_base_url(&base, None);
        client.min_interval = Duration::from_millis(300);
        let inizio = Instant::now();
        client.search("prima").unwrap();
        client.search("seconda").unwrap();
        assert!(
            inizio.elapsed() >= Duration::from_millis(300),
            "la seconda richiesta deve attendere l'intervallo minimo"
        );
    }

    #[test]
    fn la_cache_sopravvive_al_riavvio() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("geo-cache.json");
        let (base, richieste, _server) = server_finto(
            r#"[{"display_name":"Milano","lat":"45.4642","lon":"9.1900","address":{}}]"#,
        );

        {
            let client = NominatimClient::with_base_url(&base, Some(cache.clone()));
            assert_eq!(client.search("Milano").unwrap().len(), 1);
        }
        assert_eq!(richieste.load(Ordering::SeqCst), 1);
        assert!(cache.is_file());

        // Un nuovo client con la stessa cache non interroga la rete.
        let client = NominatimClient::with_base_url("http://127.0.0.1:1", Some(cache));
        assert_eq!(client.search("Milano").unwrap().len(), 1);
        assert_eq!(richieste.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn le_risposte_non_valide_vengono_scartate() {
        assert!(parse_result(RawResult {
            display_name: "x".into(),
            lat: "non un numero".into(),
            lon: "1".into(),
            name: None,
            address: AddressDetails::default(),
        })
        .is_none());
        assert!(parse_result(RawResult {
            display_name: "x".into(),
            lat: "95".into(),
            lon: "1".into(),
            name: None,
            address: AddressDetails::default(),
        })
        .is_none());
    }
}
