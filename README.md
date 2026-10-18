# UberPhoto

UberPhoto is a **local-first desktop photo manager** written in Rust. It browses real filesystem folders, presents photographs in a fast virtualized grid, and — through incremental development phases — will edit photos **non-destructively**, edit the full EXIF/IPTC/XMP metadata stack through **ExifTool**, place GPS coordinates on an **OpenStreetMap** map, and export web-ready images with metadata intact.

**Your photographs never leave your disk.** There is no server and no upload: the network is only ever needed for map tiles (Phase 4).

> **Development status — Phase 1 of 7.**
> UberPhoto is built in incremental phases; each phase is compilable and usable on its own. Phase 1 (foundations: folder navigation, thumbnails, preview, inspector scaffold) is implemented and verified on Ubuntu. Metadata editing, GPS, image editing and export are **not implemented yet** — no feature is claimed until it has been verified.

---

## What works today (Phase 1)

### Left panel — Folder explorer
- Open any real filesystem folder as a root, through the native OS folder dialog.
- Lazy folder tree: subfolders are loaded only when a branch is expanded.
- Recent folders list, persisted across restarts.
- Loading a folder with thousands of photos stays responsive: directories are scanned on a dedicated worker thread (a local benchmark scans 2,000 entries in ~3 ms).

### Center panel — Photo browser
- Virtualized thumbnail grid: only the visible rows are drawn (`ScrollArea::show_rows`), so scrolling remains smooth with thousands of photos.
- Adjustable thumbnail size (96–320 px), sort by name / modified date / size, live name filter.
- Single click to select, `Ctrl`/`Cmd`+click to toggle, `Shift`+click for a range; double-click opens the large preview.
- Thumbnails are decoded on a background rayon pool, cached in an LRU (2,000 entries) with mtime-based freshness; decode failures are cached and shown as "n/d".
- EXIF orientation is applied consistently to thumbnails and previews.

### Preview window
- Opens on double-click; zoom with the mouse wheel or the `−` / `+` buttons (2 %–3200 %), drag to pan, `Adatta` to fit, `1:1` for one image pixel per physical screen pixel, double-click to re-fit, `Esc` to close.
- Decoded by a single dedicated background worker with a latest-wins policy; the oversize buffer is capped at 2560 px on the long edge.
- Purely visual: no pixels are modified.

### Right panel — Inspector (scaffold)
- Five tabs (`Colori`, `Informazioni`, `Metadati`, `Posizione GPS`, `Esportazione`) wired for the upcoming phases.
- Today the `Informazioni` tab shows real data about the selection; the other tabs explicitly state which phase implements them.

### Application
- Dark "photographic" theme: neutral chrome so photographs dominate the scene and colours are not biased by the surroundings.
- Welcome screen with the app logo; the same icon is used by the window / taskbar.
- Session persistence (recent folders, last folder, thumbnail size), logging via `tracing` with `RUST_LOG` control.

### Recognized formats

| Format | Browse/decode today | Notes |
|---|---|---|
| JPEG, PNG, TIFF, WebP | ✅ | Full pipeline; metadata write planned for Phase 3 |
| GIF, BMP | ✅ | Browsing only |
| HEIC, AVIF | ⛔ | Recognized and listed; decoding enabled in a later phase after per-platform verification |
| DNG, CR2, CR3, NEF, ARW, RAF (RAW) | ⛔ | Recognized and listed; originals are never rewritten — sidecar XMP will be used |

Capabilities per format are declared explicitly in code (`FormatKind::capabilities`) and updated phase by phase.

---

## Requirements

- **Rust** stable, 1.85 or newer (developed with 1.98).
- **Ubuntu / Linux**: any X11 or Wayland session (eframe uses winit + wgpu).
- **macOS**: recent macOS versions on Intel or Apple Silicon.
- **Windows**: Windows 10 or newer (release builds do not open a console window).
- **ExifTool**: not needed for Phase 1. From Phase 3 it will be required for metadata read/write and resolved from `PATH`.

## Getting started

```bash
git clone git@github.com:Ogalesha/UberPhoto.git
cd UberPhoto

# Run the app (debug build, first compile takes a few minutes)
cargo run -p uberphoto-app

# Optimized binary
cargo build --release -p uberphoto-app
# Linux/macOS:  ./target/release/uberphoto-app
# Windows:      .\target\release\uberphoto-app.exe
```

### Quality gates

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Manual benchmark: scan a 2,000-file folder (ignored by default)
cargo test -p uberphoto-files -- --ignored --nocapture

# Headless GUI smoke test (what CI runs on Linux via Xvfb)
UBERPHOTO_SMOKE_EXIT_SECS=6 xvfb-run -a ./target/release/uberphoto-app
```

`UBERPHOTO_SMOKE_EXIT_SECS` makes the app close itself gracefully after N seconds; it exists so CI can verify that the GUI starts, renders and shuts down cleanly without a human.

## Repository layout

```
uberphoto/
├── assets/UberPhoto.png        # application icon (window + welcome screen)
├── crates/
│   ├── uberphoto-app/          # binary entry point (window, icon, logging)
│   ├── uberphoto-ui/           # egui interface: panels, theme, caches wiring
│   ├── uberphoto-core/         # domain types: formats, capabilities, photo assets
│   ├── uberphoto-files/        # scanning, natural sort, folder tree
│   ├── uberphoto-image/        # decode, EXIF orientation, background services
│   ├── uberphoto-metadata/     # (Phase 3) ExifTool engine
│   ├── uberphoto-geo/          # (Phase 4) OpenStreetMap integration
│   ├── uberphoto-db/           # (Phase 2) SQLite index mirroring the filesystem
│   └── uberphoto-tests/        # cross-crate integration tests
├── .github/workflows/          # CI and release pipelines
├── CHANGELOG.md                # Keep a Changelog 1.1.0
└── PRODUCT.md                  # full product specification (Italian)
```

## Architecture & design decisions

- **The filesystem is the source of truth.** The future SQLite index will mirror it for speed (search, ratings, keywords) but never replace it.
- **Immediate-mode UI, background pipelines.** egui renders on the main thread; directory scans, thumbnail decoding (rayon pool sized to the machine) and preview decoding (one dedicated worker) run off-thread and deliver results through channels with **latest-wins** semantics, so stale work is discarded instead of blocking.
- **Only visible work is done.** Thumbnails are requested for cells currently on screen; the LRU cache (2,000 textures, quantized size buckets) keeps memory bounded.
- **EXIF orientation now, ExifTool later.** Phase 1 reads only the orientation tag with `kamadak-exif` (a pure-Rust, cross-platform crate) and documents it as the single internal EXIF reader. The complete metadata engine — including writing — goes through ExifTool as an external process from Phase 3, because metadata reliability is the priority.
- **Non-destructive by principle.** No phase will ever rewrite original pixels of photographs; RAW formats are never rewritten at all (sidecar XMP).
- **Cross-platform by construction.** Pure-Rust dependencies everywhere the code touches files, images or the window (`eframe`/wgpu, `image`, `kamadak-exif`, `rfd` native dialogs); no platform-specific code except the Windows console-hiding attribute and test-only Unix symlink helpers.

## Testing and CI

- Unit and integration tests live next to the code plus a cross-crate smoke suite in `uberphoto-tests` (fixtures generated at runtime; no binary test assets in the repository).
- CI (`.github/workflows/ci.yml`) runs on **Ubuntu, macOS and Windows**: `rustfmt --check`, `clippy -D warnings`, `cargo test --workspace`, a release build, and — on Linux only — a headless GUI smoke test under Xvfb.

## Releases and versioning

- The project follows **[Semantic Versioning](https://semver.org/)** and keeps a **[Keep a Changelog](https://keepachangelog.com/en/1.1.0/)** file.
- The repository tracks **source code only**; compiled artifacts are never committed (see `.gitignore`).
- Pushing a tag like `v0.1.0` triggers the release workflow, which builds on all three platforms and attaches downloadable binaries to the GitHub Release:
  - `uberphoto-linux-x86_64.tar.gz`
  - `uberphoto-macos-aarch64.tar.gz`
  - `uberphoto-windows-x86_64.zip`
- Every workflow run also publishes build artifacts on GitHub, so compiled files are always obtainable without a local toolchain.

## Configuration and data

- UI preferences (recent folders, last folder, thumbnail size) are stored by eframe in RON format, typically at:
  - Linux: `~/.local/share/uberphoto/app.ron`
  - macOS: `~/Library/Application Support/uberphoto/app.ron`
  - Windows: `%APPDATA%\uberphoto\app.ron`
- Log verbosity follows `RUST_LOG` (default `info`).

## Roadmap

| Phase | Scope |
|---|---|
| 1 ✅ | Foundations, folder explorer, virtualized browser, preview, inspector scaffold |
| 2 | Filesystem operations and drag & drop (move/copy/rename with conflict handling, sidecar moves, SQLite index, external-change detection) |
| 3 | Metadata engine via ExifTool: read/write EXIF, IPTC, XMP; descriptive, author, time and technical fields; GPS values; sidecars |
| 4 | OpenStreetMap GPS picker (`walkers`) with marker placement |
| 5 | Non-destructive color editing (exposure, contrast, saturation, …), crop/rotate, presets, undo/redo, before/after |
| 6 | Export for web: profiles, metadata embedding, validation, reports |
| 7 | Quality and distribution: realistic metadata fixtures, regression tests against metadata loss, installable packages for all platforms |

`PRODUCT.md` (Italian) holds the complete specification, acceptance criteria and development rules.

## License

Licensed under the [MIT License](LICENSE). Copyright (c) 2026 Mirgen.

UberPhoto is a personal project; the source is public and contributions are welcome via issues and pull requests.
