//! Smoke test Fase 3: scrittura metadati, riassunto e indice SQLite.
//!
//! I test che richiedono ExifTool si saltano automaticamente se il binario
//! non è disponibile, così la suite resta eseguibile anche senza il tool.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use uberphoto_core::FormatKind;
use uberphoto_db::{Index, MetadataIndex};
use uberphoto_metadata::{
    document_from_tags, exiftool_disponibile, ExifTool, MetadataEdit, MetadataField,
    MetadataOutcome, MetadataRequest, MetadataService, WriteItem,
};

fn crea_immagine(path: &Path, format: image::ImageFormat) {
    let image = image::RgbImage::from_fn(64, 48, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
    });
    image.save_with_format(path, format).unwrap();
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

#[test]
fn scrittura_riassunto_e_indice_in_sincronia() {
    if !exiftool_disponibile() {
        eprintln!("exiftool assente: test saltato");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("foto.jpg");
    crea_immagine(&path, image::ImageFormat::Jpeg);

    let tool = ExifTool::detect().unwrap();
    tool.write_embedded(
        &path,
        &[
            "XMP-photoshop:Credit=Studio Originale".to_owned(),
            "EXIF:Artist=Mario Rossi".to_owned(),
        ],
    )
    .unwrap();

    let service = MetadataService::new(Arc::new(|| {}));
    service.request(MetadataRequest::Write {
        items: vec![WriteItem {
            path: path.clone(),
            format: FormatKind::Jpeg,
            edits: vec![
                MetadataEdit::Field(MetadataField::Title, Some("Titolo test".to_owned())),
                MetadataEdit::Field(MetadataField::Keywords, Some("mare, blu".to_owned())),
                MetadataEdit::Field(MetadataField::Rating, Some("4".to_owned())),
            ],
        }],
    });
    match attendi(&service) {
        MetadataOutcome::Written { report } => {
            assert!(report.failed.is_empty(), "{:?}", report.failed);
            assert_eq!(report.updated, vec![path.clone()]);
        }
        _ => panic!("esito inatteso"),
    }

    // I tag non gestiti dall'editor restano intatti.
    let tags = tool.read(std::slice::from_ref(&path), &[]).unwrap();
    let valore = |suffisso: &str| {
        tags[0]
            .iter()
            .find(|(key, _)| key.ends_with(suffisso))
            .map(|(_, value)| value.as_str().unwrap_or_default().to_owned())
    };
    assert_eq!(valore(":Credit").as_deref(), Some("Studio Originale"));
    assert_eq!(valore(":Artist").as_deref(), Some("Mario Rossi"));

    // Il riassunto alimenta l'indice SQLite come fa la UI dopo la lettura.
    service.request(MetadataRequest::ReadSummary {
        paths: vec![path.clone()],
    });
    let summary = match attendi(&service) {
        MetadataOutcome::Summary { mut items } => {
            assert_eq!(items.len(), 1);
            items.remove(0).1
        }
        _ => panic!("esito inatteso"),
    };
    assert_eq!(summary.rating, Some(4));
    assert_eq!(summary.keywords.as_deref(), Some("mare, blu"));
    assert!(!summary.has_gps);

    let index = Index::open(&dir.path().join("index.db")).unwrap();
    index
        .upsert_metadata(
            &path,
            Some(1),
            &MetadataIndex {
                has_gps: summary.has_gps,
                rating: summary.rating,
                date_original: summary.date_original,
                keywords: summary.keywords,
                camera_make: summary.camera_make,
                camera_model: summary.camera_model,
                lens: summary.lens,
                iso: summary.iso,
                f_number: summary.f_number,
                exposure_time: summary.exposure_time,
                focal_length: summary.focal_length,
                gps_lat: summary.gps_lat,
                gps_lon: summary.gps_lon,
            },
        )
        .unwrap();
    let rows = index.load_dir(dir.path()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].rating, Some(4));
    assert_eq!(rows[0].keywords.as_deref(), Some("mare, blu"));
}

#[test]
fn i_formati_senza_metadati_embedded_usano_il_sidecar() {
    if !exiftool_disponibile() {
        eprintln!("exiftool assente: test saltato");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // PNG rinominato: simula un RAW il cui formato non supporta i metadati
    // incorporati e richiede quindi il sidecar XMP.
    let path = dir.path().join("scatto.dng");
    crea_immagine(&path, image::ImageFormat::Png);
    let prima = std::fs::read(&path).unwrap();

    let service = MetadataService::new(Arc::new(|| {}));
    service.request(MetadataRequest::Write {
        items: vec![WriteItem {
            path: path.clone(),
            format: FormatKind::Dng,
            edits: vec![
                MetadataEdit::Field(MetadataField::Title, Some("Titolo RAW".to_owned())),
                MetadataEdit::Field(MetadataField::Orientation, Some("6".to_owned())),
            ],
        }],
    });
    let report = match attendi(&service) {
        MetadataOutcome::Written { report } => report,
        _ => panic!("esito inatteso"),
    };
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.updated, vec![path.clone()]);
    assert!(
        !report.skipped.is_empty(),
        "l'orientamento non ha equivalente nel sidecar"
    );

    let sidecar = uberphoto_metadata::sidecar_path(&path);
    assert!(sidecar.is_file(), "sidecar XMP non creato");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        prima,
        "il RAW non deve essere modificato"
    );

    let tags = ExifTool::detect().unwrap().read(&[sidecar], &[]).unwrap();
    let document = document_from_tags(&tags[0]);
    assert_eq!(document.value(MetadataField::Title), Some("Titolo RAW"));
}
