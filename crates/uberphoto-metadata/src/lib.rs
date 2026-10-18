//! Motore metadati (Fase 3).
//!
//! Questo crate ospiterà:
//! - l'adapter al processo esterno ExifTool (argomenti strutturati, mai
//!   interpolazione di shell);
//! - il modello canonico `MetadataDocument` e la mappatura esplicita verso
//!   i tag EXIF / IPTC / XMP di destinazione;
//! - la gestione dei sidecar XMP per RAW e formati non riscrivibili;
//! - la verifica post-scrittura (rilettura del file con ExifTool).
//!
//! Stato attuale: scaffolding, nessuna funzionalità implementata.
