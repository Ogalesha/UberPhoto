//! Smoke test di Fase 1: scansione di una cartella reale, ordinamento delle
//! voci e decodifica con scalatura di un'immagine generata.

use std::fs;

use uberphoto_files::{scan_directory, sort_assets, SortKey};

#[test]
fn scansione_ordinamento_e_filtro_di_una_cartella_reale() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();

    fs::write(root.join("IMG_10.jpg"), b"xxx").unwrap();
    fs::write(root.join("img_2.jpg"), b"xx").unwrap();
    fs::write(root.join("foto.png"), b"x").unwrap();
    fs::write(root.join("note.txt"), b"contenuto non immagine").unwrap();
    fs::write(root.join(".nascosta.jpg"), b"x").unwrap();

    let mut entries = scan_directory(root, false).expect("scansione");

    let names: Vec<String> = entries.iter().map(|e| e.file_name()).collect();
    assert_eq!(names, vec!["foto.png", "img_2.jpg", "IMG_10.jpg"]);

    sort_assets(&mut entries, SortKey::Size, false);
    let names: Vec<String> = entries.iter().map(|e| e.file_name()).collect();
    assert_eq!(names, vec!["IMG_10.jpg", "img_2.jpg", "foto.png"]);
}

#[test]
fn decodifica_con_scalatura_di_un_png_generato() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("generata.png");

    let image = image::RgbaImage::from_fn(400, 200, |x, _y| image::Rgba([x as u8, 40, 60, 255]));
    image.save(&path).expect("salvataggio png");

    let loaded = uberphoto_image::load_image_rgba(&path, Some(100)).expect("decodifica");
    assert_eq!((loaded.width, loaded.height), (100, 50));
    assert_eq!((loaded.source_width, loaded.source_height), (400, 200));

    let full = uberphoto_image::load_image_rgba(&path, None).expect("decodifica completa");
    assert_eq!((full.width, full.height), (400, 200));
    assert_eq!(full.rgba.len(), 400 * 200 * 4);
}
