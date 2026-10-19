# Changelog

Tutte le modifiche rilevanti di questo progetto sono documentate in questo file.

Il formato segue [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
e il progetto aderisce a [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] - 2026-10-19

### Added

- Fase 4 — selettore GPS su mappa OpenStreetMap con marker trascinabile, clic per posizionare, coordinate manuali e centratura; ricerca facoltativa dei luoghi su Nominatim solo su azione esplicita, con User-Agent e attribuzione.
- Fase 5 — editing non distruttivo: ricetta colore salvata nel catalogo (esposizione, luminosità, contrasto, alte luci, ombre, bianchi, neri, saturazione, vibrance, temperatura, tinta, gamma, nitidezza), ritaglio e rotazione a passi di 90°.
- Fase 5 — anteprima in tempo reale su copia ridotta con worker "latest wins", confronto "Prima", annulla/ripeti e ripristino per singolo campo.
- Fase 5 — preset colore salvati nel catalogo, applicabili alla foto corrente o all'intera selezione multipla.
- Fase 6 — nuovo crate `uberphoto-export`: profili Full metadata, Web optimized e Privacy, formati JPEG/PNG/WebP/TIFF, conversione ICC a sRGB con profilo incorporato, ridimensionamento Lanczos3 senza ingrandimento, denominazione con prefisso/suffisso/slug e conflitti senza sovrascrittura.
- Fase 6 — verifica dell'esportazione: i tag di sorgente e uscita vengono riletti insieme e classificati come preservati, adeguati, non supportati o persi, con avvisi espliciti (per esempio IPTC non memorizzabile in WebP).
- Fase 6 — scheda Esportazione con profilo, formato, qualità, ridimensionamento, destinazione, avanzamento, annullamento e report per foto; i file parziali vengono rimossi in caso di errore.
- Fase 7 — pacchetti installabili e autosufficienti: `.deb` Debian/Ubuntu con desktop entry, bundle `.app` macOS, installer Windows (Inno Setup) e ZIP portabile, tutti con ExifTool incluso; archivio portabile Linux con ExifTool.
- Fase 7 — smoke test di integrazione GPS (Fase 4): scrittura e rilettura del marker con altitudine, rimozione senza toccare i tag estranei, rifiuto delle coordinate fuori intervallo.
- Fase 7 — ExifTool viene cercato prima accanto all'eseguibile (copia inclusa nel pacchetto), poi in `UBERPHOTO_EXIFTOOL` e infine nel `PATH`.
- Fase 7 — benchmark manuali per la pipeline colore su 24 MP e per la scansione di 2.000 file; verifica dei pacchetti nella CI.
- Fase 7 — README aggiornato alle Fasi 1–7 con funzionalità, limiti noti e istruzioni di packaging.

### Fixed

- La scheda Colori applica i preset alla selezione multipla invece di limitarsi a dichiararla non supportata.
- `.gitignore` esclude segreti (`.env`, chiavi), fotografie, database locali, log e la specifica privata del prodotto; nel repository restano solo sorgenti, documentazione pubblica e l'icona necessaria alla compilazione.

## [0.2.1] - 2026-10-18

### Fixed

- Su Windows le operazioni su cartelle dell'indice (`load_dir`, `move_dir`, `remove_dir`) non trovavano le righe quando i percorsi mescolavano separatori `/` e `\`: il pattern LIKE ora è un prefisso indipendente dal separatore, con filtro esatto in Rust.
- La CI non pubblicava i log dei test falliti (ora il log finisce in un check run dedicato visibile senza autenticazione).
- Lo smoke test GUI Linux si interrompeva sotto Xvfb per il mancato caricamento di `libxkbcommon-x11` (pacchetto ora installato) e per lo schermo Xvfb a 8 bit (ora 24 bit).

## [0.2.0] - 2026-10-18

### Added

- Fase 2 — operazioni filesystem: spostamento, copia e rinomina con tre politiche sui conflitti (rinomina automatica, sostituzione, salto), annullamento dell'ultimo spostamento e fallback copia+elimina tra dispositivi diversi.
- Fase 2 — drag & drop delle foto dalla griglia alle cartelle dell'esploratore, con pastiglia di conteggio e prompt di conferma.
- Fase 2 — gestione dei sidecar XMP nelle operazioni di file: il sidecar segue la foto spostata o rinominata.
- Fase 2 — creazione e eliminazione di sottocartelle vuote e rinomina delle cartelle dall'esploratore.
- Fase 2 — indice SQLite locale (`index.db`) in modalità WAL: file, dimensioni immagine, valutazione, data di scatto, keyword, dati fotocamera e GPS; aggiornamenti incrementali e ricostruzione automatica.
- Fase 2 — rilevamento delle modifiche esterne con `notify` e riscansione calmierata; gli eventi di sola lettura non innescano più riscansioni.
- Fase 2 — ordinamento per data di acquisizione e per valutazione tramite l'indice.
- Fase 3 — nuovo crate `uberphoto-metadata`: adapter al processo ExifTool con argomenti strutturati, individuazione da `PATH` o `UBERPHOTO_EXIFTOOL`, lettura batch e scrittura con verifica post-scrittura.
- Fase 3 — modello canonico con 26 campi editabili (descrittivi, autore/diritti, data e orientamento), validazione di valutazione, orientamento, data e fuso orario.
- Fase 3 — scrittura embedded per JPEG, PNG, TIFF e WebP; IPTC nativo solo su JPEG/TIFF; sidecar XMP per RAW, HEIC e AVIF senza mai riscrivere l'originale.
- Fase 3 — editor metadati nell'inspector con multi-selezione, valori misti, indicatore dei campi modificati, salvataggio e ricarica.
- Fase 3 — vista di sola lettura dei dati tecnici della fotocamera e dei tag grezzi ExifTool con filtro e indicazione di scrivibilità.
- Fase 3 — badge GPS e stelle nella griglia, filtri per valutazione minima, GPS e keyword, valutazione rapida dal menu contestuale.
- Test di integrazione delle Fasi 2 e 3 (`phase2_smoke`, `phase3_smoke`) e smoke test E2E con indice e metadati popolati.
- CI: installazione di ExifTool su Linux e macOS (best effort su Windows), smoke test GUI sperimentali su macOS e Windows.

### Fixed

- Il watcher innescava un ciclo di riscansioni perché gli eventi di apertura/lettura dei file venivano trattati come modifiche.
- L'aggiornamento dell'indice applicava `move_path` anche alle copie, corrompendo il percorso della foto sorgente.

## [0.1.0] - 2026-10-18

### Added

- Fase 1 — fondazioni: workspace Cargo con i crate `uberphoto-{core,files,image,ui,app,metadata,geo,db,tests}`.
- Pannello sinistro: esploratore ad albero con caricamento lazy dei rami, cartelle recenti e apertura di una nuova radice tramite finestra di dialogo nativa.
- Pannello centrale: griglia di miniature virtualizzata con selezione singola e multipla (Ctrl/Cmd e Shift), filtro per nome, ordinamento e dimensione delle miniature regolabile.
- Pannello destro: anteprima grande con zoom, pan e adattamento alla finestra; schede predisposte, con dati reali nella scheda "Informazioni".
- Miniature e anteprime generate su thread di background con cache LRU, rilevamento modifiche su mtime e orientamento EXIF.
- Tema scuro professionale fotografico e schermata iniziale con logo.
- Persistenza di cartelle recenti, ultima cartella aperta e dimensione delle miniature.
- Icona dell'applicazione (finestra e schermata iniziale).
- CI GitHub Actions su Ubuntu, macOS e Windows (formato, clippy, test) e workflow di release con binari scaricabili per i tag `v*`.
- Smoke test GUI headless (Xvfb) nella CI Linux.

[Unreleased]: https://github.com/Ogalesha/UberPhoto/compare/v0.3.0...main
[0.3.0]: https://github.com/Ogalesha/UberPhoto/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/Ogalesha/UberPhoto/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/Ogalesha/UberPhoto/compare/v0.1.0...v0.2.0
