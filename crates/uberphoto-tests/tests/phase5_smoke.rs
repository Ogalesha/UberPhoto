//! Smoke test Fase 5: ricette di editing, pipeline colore, catalogo e servizio
//! di elaborazione in background.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use uberphoto_db::Index;
use uberphoto_image::{
    apply_recipe, load_image_rgba, CropRect, EditRecipe, EditService, EditSource, EditedPixels,
};

fn crea_immagine(path: &Path) {
    let image = image::RgbImage::from_fn(80, 60, |x, y| {
        image::Rgb([(x * 3) as u8, (y * 4) as u8, 120])
    });
    image
        .save_with_format(path, image::ImageFormat::Jpeg)
        .unwrap();
}

fn attendi(service: &EditService) -> Vec<EditedPixels> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let risultati: Vec<_> = service
            .drain()
            .into_iter()
            .map(|r| (*r.pixels).clone())
            .collect();
        if !risultati.is_empty() {
            return risultati;
        }
        assert!(
            Instant::now() < deadline,
            "timeout in attesa del servizio di editing"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn ricetta_identica_preserva_i_pixel_e_geometria_ruota_e_ritaglia() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("foto.jpg");
    crea_immagine(&path);

    let loaded = load_image_rgba(&path, None).unwrap();
    let originale = loaded.rgba.clone();

    let identica = apply_recipe(&loaded, &EditRecipe::default());
    assert_eq!(
        identica.rgba, originale,
        "ricetta neutra non altera i pixel"
    );

    let ruotata = apply_recipe(
        &loaded,
        &EditRecipe {
            rotation: 90,
            ..EditRecipe::default()
        },
    );
    assert_eq!(
        (ruotata.width, ruotata.height),
        (loaded.height, loaded.width),
        "la rotazione scambia le dimensioni"
    );

    let ritagliata = apply_recipe(
        &loaded,
        &EditRecipe {
            crop: Some(CropRect {
                left: 0.25,
                top: 0.25,
                right: 0.75,
                bottom: 0.75,
            }),
            ..EditRecipe::default()
        },
    );
    assert_eq!(ritagliata.width, loaded.width / 2);
    assert_eq!(ritagliata.height, loaded.height / 2);

    let regolata = apply_recipe(
        &loaded,
        &EditRecipe {
            exposure: 1.0,
            contrast: 30.0,
            saturation: 40.0,
            ..EditRecipe::default()
        },
    );
    assert_eq!(regolata.rgba.len(), originale.len());
    assert_ne!(regolata.rgba, originale, "le regolazioni cambiano i pixel");
}

#[test]
fn ricetta_e_preset_sopravvivono_a_rinomina_e_spostamento() {
    let dir = tempfile::tempdir().unwrap();
    let foto = dir.path().join("a.jpg");
    crea_immagine(&foto);
    let rinominata = dir.path().join("b.jpg");

    let index = Index::open(&dir.path().join("catalogo.db")).unwrap();
    let recipe = EditRecipe {
        exposure: 0.5,
        rotation: 270,
        ..EditRecipe::default()
    };
    index.save_recipe(&foto, &recipe.to_json()).unwrap();
    index.save_preset("Calda", &recipe.to_json()).unwrap();
    assert_eq!(index.count_recipes().unwrap(), 1);

    std::fs::rename(&foto, &rinominata).unwrap();
    index.move_path(&foto, &rinominata).unwrap();
    let riletta = index.load_recipe(&rinominata).unwrap().unwrap();
    assert_eq!(EditRecipe::from_json(&riletta).unwrap(), recipe);
    assert!(index.load_recipe(&foto).unwrap().is_none());

    index.remove(&rinominata).unwrap();
    assert_eq!(index.count_recipes().unwrap(), 0);
    let presets = index.load_presets().unwrap();
    assert_eq!(presets.len(), 1);
    assert_eq!(EditRecipe::from_json(&presets[0].1).unwrap(), recipe);
}

#[test]
fn il_servizio_elabora_l_ultima_richiesta_e_riusa_la_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("foto.jpg");
    crea_immagine(&path);

    let image = load_image_rgba(&path, None).unwrap();
    let larghezza = image.width;
    let source = Arc::new(EditSource {
        path: path.clone(),
        mtime: None,
        image,
    });

    let service = EditService::new(Arc::new(|| {}));
    let prima = EditRecipe {
        exposure: 1.0,
        ..EditRecipe::default()
    };
    service.request(1, source.clone(), prima);
    let risultati = attendi(&service);
    assert_eq!(risultati.last().unwrap().width, larghezza);

    // La cache restituisce subito il risultato per la stessa ricetta.
    service.request(2, source.clone(), prima);
    let risultati = attendi(&service);
    assert_eq!(risultati.len(), 1);
    assert_eq!(risultati[0].width, larghezza);
}
