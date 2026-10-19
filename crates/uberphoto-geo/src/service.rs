//! Servizio geografico su thread dedicato.
//!
//! Stesso schema degli altri servizi (scansione, miniature, metadati):
//! canale mpsc, nessun blocco della UI, repaint al termine. Le richieste
//! sono sequenziali, così il rate limiting di Nominatim resta effettivo.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use uberphoto_core::GpsLocation;

use crate::nominatim::{GeocodeResult, NominatimClient};

/// Richiesta inviata dal thread UI.
pub enum GeoRequest {
    /// Ricerca per testo (solo su azione esplicita dell'utente).
    Search { query: String },
    /// Reverse geocoding della posizione indicata.
    Reverse { location: GpsLocation },
}

/// Esito di una richiesta, con il contesto per aggiornare la UI.
pub enum GeoOutcome {
    Search {
        query: String,
        result: Result<Vec<GeocodeResult>, String>,
    },
    Reverse {
        location: GpsLocation,
        result: Result<Option<Box<GeocodeResult>>, String>,
    },
}

pub struct GeoService {
    tx: Sender<GeoRequest>,
    rx: Receiver<GeoOutcome>,
}

impl GeoService {
    pub fn new(client: NominatimClient, repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, requests) = channel::<GeoRequest>();
        let (outcomes_tx, rx) = channel::<GeoOutcome>();

        std::thread::Builder::new()
            .name("geo".to_owned())
            .spawn(move || {
                while let Ok(request) = requests.recv() {
                    let outcome = match request {
                        GeoRequest::Search { query } => GeoOutcome::Search {
                            result: client.search(&query).map_err(|err| err.to_string()),
                            query,
                        },
                        GeoRequest::Reverse { location } => GeoOutcome::Reverse {
                            result: client
                                .reverse(location.latitude, location.longitude)
                                .map(|result| result.map(Box::new))
                                .map_err(|err| err.to_string()),
                            location,
                        },
                    };
                    if outcomes_tx.send(outcome).is_err() {
                        break;
                    }
                    repaint();
                }
            })
            .expect("avvio del thread geo");

        Self { tx, rx }
    }

    pub fn request(&self, request: GeoRequest) {
        if self.tx.send(request).is_err() {
            tracing::error!("servizio geografico non disponibile");
        }
    }

    pub fn try_recv(&self) -> Option<GeoOutcome> {
        self.rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    #[test]
    fn il_servizio_recapita_la_ricerca() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let porta = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                let body =
                    r#"[{"display_name":"Roma","lat":"41.8933","lon":"12.4829","address":{}}]"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let client = NominatimClient::with_base_url(&format!("http://127.0.0.1:{porta}"), None);
        let service = GeoService::new(client, Arc::new(|| {}));
        service.request(GeoRequest::Search {
            query: "Roma".to_owned(),
        });

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut outcome = None;
        while Instant::now() < deadline && outcome.is_none() {
            outcome = service.try_recv();
            if outcome.is_none() {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        match outcome.expect("esito entro il timeout") {
            GeoOutcome::Search { query, result } => {
                assert_eq!(query, "Roma");
                let results = result.unwrap();
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].display_name, "Roma");
            }
            _ => panic!("esito inatteso"),
        }
    }
}
