# Changelog

Tutte le modifiche rilevanti di questo progetto sono documentate in questo file.

Il formato segue [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
e il progetto aderisce a [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/Ogalesha/UberPhoto/commits/main
