//! Smoke test Fase 6: Export for Web con verifica ExifTool.
//!
//! I test si saltano automaticamente se ExifTool non è disponibile.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use uberphoto_core::FormatKind;
use uberphoto_export::{
    ExportEvent, ExportItem, ExportJob, ExportPreset, ExportService, ExportSettings, NameMode,
    Naming, PrivacyOptions, ResizeRules,
};
use uberphoto_image::EditRecipe;
use uberphoto_metadata::{exiftool_disponibile, ExifTool};

fn crea_immagine(path: &Path, width: u32, height: u32) {
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
    });
    image
        .save_with_format(path, image::ImageFormat::Jpeg)
        .unwrap();
}

fn prepara_sorgente(path: &Path) {
    crea_immagine(path, 400, 300);
    let tool = ExifTool::detect().unwrap();
    tool.write_embedded(
        path,
        &[
            "XMP-dc:Title=Mare al tramonto".to_owned(),
            "XMP-dc:Creator=Mario Rossi".to_owned(),
            "XMP-dc:Rights=© Mario Rossi".to_owned(),
            "XMP-dc:Subject=mare,tramonto".to_owned(),
            "EXIF:ImageDescription=Descrizione".to_owned(),
            "EXIF:Copyright=© Mario Rossi".to_owned(),
            "IPTC:Keywords=mare".to_owned(),
            "IPTC:Caption-Abstract=Descrizione IPTC".to_owned(),
            "GPSLatitude=45.1".to_owned(),
            "GPSLatitudeRef=N".to_owned(),
            "GPSLongitude=9.2".to_owned(),
            "GPSLongitudeRef=E".to_owned(),
            "XMP-photoshop:City=Genova".to_owned(),
        ],
    )
    .unwrap();
}

fn attendi_lavoro(service: &ExportService) -> (Vec<ExportEvent>, bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut eventi = Vec::new();
    loop {
        for evento in service.drain() {
            let finito = matches!(evento, ExportEvent::Finished { .. });
            eventi.push(evento);
            if finito {
                return (eventi, true);
            }
        }
        assert!(
            Instant::now() < deadline,
            "timeout in attesa dell'esportazione"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn export_full_preserva_metadati_verificati_e_originale() {
    if !exiftool_disponibile() {
        eprintln!("exiftool assente: test saltato");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("scatto.jpg");
    prepara_sorgente(&source);
    let originale = std::fs::read(&source).unwrap();

    let settings = ExportSettings {
        destination: dir.path().join("web"),
        preset: ExportPreset::FullMetadata,
        resize: ResizeRules {
            enabled: true,
            max_width: 300,
            max_height: 300,
            keep_aspect: true,
        },
        naming: Naming {
            mode: NameMode::KeepName,
            ..Naming::default()
        },
        ..ExportSettings::default()
    };
    let service = ExportService::new(Arc::new(|| {}));
    let avviato = service.start(ExportJob {
        settings,
        items: vec![ExportItem {
            path: source.clone(),
            kind: FormatKind::Jpeg,
            recipe: EditRecipe {
                exposure: 0.5,
                rotation: 90,
                ..EditRecipe::default()
            },
        }],
    });
    assert!(avviato, "lavoro non avviato");

    let (eventi, _) = attendi_lavoro(&service);
    let report = eventi
        .iter()
        .find_map(|evento| match evento {
            ExportEvent::Item(report) => Some(report.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("nessun report: {eventi:?}"));

    assert!(report.verified(), "campi persi: {:?}", report.lost);
    assert!(
        report.preserved.contains(&"Titolo".to_owned()),
        "{:?}",
        report.preserved
    );
    assert!(
        report.preserved.contains(&"Autore".to_owned()),
        "{:?}",
        report.preserved
    );
    assert!(report.preserved.contains(&"Copyright".to_owned()));
    assert!(
        report.preserved.contains(&"Keywords".to_owned())
            || report.preserved.contains(&"Descrizione".to_owned())
    );
    assert!(report.preserved.contains(&"Posizione GPS".to_owned()));
    assert_eq!((report.width, report.height), (225, 300));

    let output = &report.output;
    assert!(output.is_file());

    let tags = ExifTool::detect()
        .unwrap()
        .read(std::slice::from_ref(output), &[])
        .unwrap();
    let document = uberphoto_metadata::document_from_tags(&tags[0]);
    assert_eq!(
        document.value(uberphoto_metadata::MetadataField::Title),
        Some("Mare al tramonto")
    );
    assert_eq!(
        document.value(uberphoto_metadata::MetadataField::Creator),
        Some("Mario Rossi")
    );
    assert!(document.gps.is_some(), "GPS assente nel file esportato");
    assert_eq!(
        document.value(uberphoto_metadata::MetadataField::Orientation),
        Some("1")
    );

    let citta = ExifTool::detect()
        .unwrap()
        .read(std::slice::from_ref(output), &["XMP-photoshop:City"])
        .unwrap();
    assert_eq!(
        citta[0]
            .get("XMP-photoshop:City")
            .and_then(serde_json::Value::as_str),
        Some("Genova"),
        "un tag non gestito deve sopravvivere all'esportazione"
    );

    assert_eq!(
        std::fs::read(&source).unwrap(),
        originale,
        "l'originale non deve essere toccato"
    );
}

#[test]
fn export_privacy_rimuove_il_gps_e_conserva_il_resto() {
    if !exiftool_disponibile() {
        eprintln!("exiftool assente: test saltato");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("privata.jpg");
    prepara_sorgente(&source);

    let settings = ExportSettings {
        destination: dir.path().join("web"),
        preset: ExportPreset::Privacy,
        privacy: PrivacyOptions::default(),
        resize: ResizeRules {
            enabled: false,
            ..ResizeRules::default()
        },
        ..ExportSettings::default()
    };
    let service = ExportService::new(Arc::new(|| {}));
    assert!(service.start(ExportJob {
        settings,
        items: vec![ExportItem {
            path: source.clone(),
            kind: FormatKind::Jpeg,
            recipe: EditRecipe::default(),
        }],
    }));
    let (eventi, _) = attendi_lavoro(&service);
    let report = eventi
        .iter()
        .find_map(|evento| match evento {
            ExportEvent::Item(report) => Some(report.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("nessun report: {eventi:?}"));

    assert!(report.verified(), "campi persi: {:?}", report.lost);
    assert!(report.adjusted.contains(&"GPS rimosso".to_owned()));

    let tags = ExifTool::detect()
        .unwrap()
        .read(&[report.output], &[])
        .unwrap();
    let document = uberphoto_metadata::document_from_tags(&tags[0]);
    assert!(document.gps.is_none(), "GPS ancora presente");
    assert_eq!(
        document.value(uberphoto_metadata::MetadataField::Title),
        Some("Mare al tramonto")
    );
}

#[test]
fn export_web_optimized_copia_solo_i_campi_editoriali() {
    if !exiftool_disponibile() {
        eprintln!("exiftool assente: test saltato");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("web.jpg");
    prepara_sorgente(&source);

    let settings = ExportSettings {
        destination: dir.path().join("web"),
        preset: ExportPreset::WebOptimized,
        format: uberphoto_export::ExportFormat::Webp,
        resize: ResizeRules {
            enabled: false,
            ..ResizeRules::default()
        },
        ..ExportSettings::default()
    };
    let service = ExportService::new(Arc::new(|| {}));
    assert!(service.start(ExportJob {
        settings,
        items: vec![ExportItem {
            path: source.clone(),
            kind: FormatKind::Jpeg,
            recipe: EditRecipe::default(),
        }],
    }));
    let (eventi, _) = attendi_lavoro(&service);
    let report = eventi
        .iter()
        .find_map(|evento| match evento {
            ExportEvent::Item(report) => Some(report.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("nessun report: {eventi:?}"));

    assert!(report.output.extension().is_some_and(|ext| ext == "webp"));
    assert!(report.preserved.contains(&"Titolo".to_owned()));
    assert!(report.preserved.contains(&"Autore".to_owned()));
    assert!(
        report.unsupported.iter().any(|riga| riga.contains("IPTC")),
        "WebP non dichiara IPTC non supportato: {:?}",
        report.unsupported
    );
    assert!(report.verified(), "campi persi: {:?}", report.lost);
}

#[test]
fn un_formato_non_decodificabile_viene_rifiutato() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("raw.cr2");
    std::fs::write(&source, b"finto raw").unwrap();
    let service = ExportService::new(Arc::new(|| {}));
    assert!(service.start(ExportJob {
        settings: ExportSettings::default(),
        items: vec![ExportItem {
            path: source.clone(),
            kind: FormatKind::Cr2,
            recipe: EditRecipe::default(),
        }],
    }));
    let (eventi, _) = attendi_lavoro(&service);
    assert!(eventi.iter().any(|evento| matches!(
        evento,
        ExportEvent::Failed { source: percorso, .. } if percorso == &source
    )));
}
