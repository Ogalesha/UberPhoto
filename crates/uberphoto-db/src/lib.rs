//! Indice locale SQLite (Fasi 2-3).
//!
//! Questo crate ospiterà l'indice e la cache persistente (rusqlite con
//! feature `bundled`): catalogo delle cartelle scansionate, cache metadati,
//! stato dell'EditRecipe, preset di esportazione.
//!
//! Principio architetturale: **SQLite riflette lo stato del filesystem, non
//! lo sostituisce**. Ogni scrittura deve restare verificabile rispetto ai
//! file reali; il database non è mai l'unica destinazione dei dati.
//!
//! Stato attuale: scaffolding, nessuna funzionalità implementata.
