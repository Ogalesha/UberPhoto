//! Smoke test Fase 4: salvataggio e rimozione delle coordinate GPS.
//!
//! I test si saltano automaticamente se ExifTool non è disponibile.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use uberphoto_core::{FormatKind, GpsLocation};
use uberphoto_metadata::{
    document_from_tags, exiftool_disponibile, ExifTool, MetadataEdit, MetadataOutcome,
    MetadataRequest, MetadataService, WriteItem,
};

fn crea_immagine(path: &Path) {
    let image = image::RgbImage::from_fn(64, 48, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
    });
    image
        .save_with_format(path, image::ImageFormat::Jpeg)
        .unwrap();
}

fn attendi(service: &MetadataService) -> MetadataOutcome {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(outcome) = service.try_recv() {
            return outcome;
        }
        assert!(
            Instant::now() < deadline,
            "timeout in attesa del motore metadati"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn scrivi_gps(service: &MetadataService, path: &Path, location: Option<GpsLocation>) {
    service.request(MetadataRequest::Write {
        items: vec![WriteItem {
            path: path.to_path_buf(),
            format: FormatKind::Jpeg,
            edits: vec![MetadataEdit::Gps(location)],
        }],
    });
    match attendi(service) {
        MetadataOutcome::Written { report } => {
            assert!(report.failed.is_empty(), "{:?}", report.failed);
            assert_eq!(report.updated, vec![path.to_path_buf()]);
        }
        _ => panic!("esito inatteso: la scrittura non è stata confermata"),
    }
}

#[test]
fn il_marker_gps_sopravvive_a_scrittura_e_rilettura() {
    if !exiftool_disponibile() {
        eprintln!("exiftool assente: test saltato");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("marker.jpg");
    crea_immagine(&path);

    let posizione = GpsLocation::new(-33.8688, -70.6693, Some(512.5)).unwrap();
    let service = MetadataService::new(Arc::new(|| {}));
    scrivi_gps(&service, &path, Some(posizione));

    let tags = ExifTool::detect()
        .unwrap()
        .read(std::slice::from_ref(&path), &[])
        .unwrap();
    let gps = document_from_tags(&tags[0])
        .gps
        .expect("GPS assente dopo la scrittura");
    assert!((gps.latitude - posizione.latitude).abs() < 1e-4, "{gps:?}");
    assert!(
        (gps.longitude - posizione.longitude).abs() < 1e-4,
        "{gps:?}"
    );
    assert!(
        gps.altitude.is_some_and(|alt| (alt - 512.5).abs() < 0.1),
        "{gps:?}"
    );
}

#[test]
fn rimuovere_il_marker_lascia_intatti_gli_altri_tag() {
    if !exiftool_disponibile() {
        eprintln!("exiftool assente: test saltato");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("senza_gps.jpg");
    crea_immagine(&path);

    let tool = ExifTool::detect().unwrap();
    tool.write_embedded(&path, &["XMP-photoshop:City=Torino".to_owned()])
        .unwrap();
    let service = MetadataService::new(Arc::new(|| {}));
    scrivi_gps(
        &service,
        &path,
        Some(GpsLocation::new(45.0703, 7.6869, None).unwrap()),
    );
    scrivi_gps(&service, &path, None);

    let tags = tool.read(std::slice::from_ref(&path), &[]).unwrap();
    let document = document_from_tags(&tags[0]);
    assert!(
        document.gps.is_none(),
        "GPS non rimosso: {:?}",
        document.gps
    );
    assert_eq!(
        tags[0]
            .get("XMP-photoshop:City")
            .and_then(serde_json::Value::as_str),
        Some("Torino"),
        "un tag non gestito deve sopravvivere alla rimozione del GPS"
    );
}

#[test]
fn coordinate_fuori_intervallo_sono_rifiutate() {
    assert!(GpsLocation::new(91.0, 0.0, None).is_err());
    assert!(GpsLocation::new(0.0, -181.0, None).is_err());
    assert!(GpsLocation::new(f64::NAN, 0.0, None).is_err());
    assert!(GpsLocation::new(0.0, 0.0, Some(f64::INFINITY)).is_err());
}
