# UberPhoto

UberPhoto is a **local-first desktop photo manager** for real folders, built in Rust. It opens folders directly from disk and lets you cull, rate, edit metadata, geotag, adjust color non-destructively and export web-ready files — without importing anything into a proprietary library.

**Your photographs never leave your disk.** There is no account, no server and no upload. The network is used only for OpenStreetMap tiles and, on explicit request, Nominatim place search.

## Why UberPhoto

- **Your folders, your files.** No library import and no hidden copy of your originals: the filesystem stays the source of truth and the SQLite index can be rebuilt at any time.
- **Fast culling.** Cached thumbnails, keyboard and multi-selection, star ratings, filters and a large preview keep big shoots moving.
- **Complete metadata editing.** EXIF, IPTC and XMP through the bundled ExifTool, with read-back verification and XMP sidecars for formats that must not be rewritten.
- **Geotagging on a map.** Search a place or click the OpenStreetMap map, then save coordinates and IPTC location fields to one photo or a whole selection.
- **Non-destructive color work.** Exposure, tone, color, crop and rotation stay in a per-photo draft until you explicitly save a new copy or overwrite the original.
- **Save and export with guarantees.** Atomic replacement, rollback on failure, per-field verification, ICC-aware rendering and metadata-aware export profiles.

## Features

### Left panel — Folder explorer
- Open any real filesystem folder as a root, through the native OS folder dialog.
- Lazy folder tree: subfolders are loaded only when a branch is expanded.
- Recent folders list, persisted across restarts.
- Loading a folder with thousands of photos stays responsive: directories are scanned on a dedicated worker thread (a local benchmark scans 2,000 entries in ~3 ms).

### Center panel — Photo browser
- Virtualized thumbnail grid: only the visible rows are drawn (`ScrollArea::show_rows`), so scrolling remains smooth with thousands of photos.
- Adjustable thumbnail size (96–320 px), sort by name / modified date / size / capture date / rating, live name filter.
- Single click to select, `Ctrl`/`Cmd`+click to toggle, `Shift`+click for a range; double-click opens the large preview.
- Thumbnails are decoded on a background rayon pool, cached in an LRU (2,000 entries) with mtime-based freshness; decode failures are cached and shown as "n/d".
- EXIF orientation is applied consistently to thumbnails and previews.
- Grid overlays for GPS presence and star rating; toolbar filters by minimum rating, GPS and keyword.
- Quick rating (0–5 stars) from the photo context menu.

### Preview window
- Opens on double-click; zoom with the mouse wheel or the `−` / `+` buttons (2 %–3200 %), drag to pan, `Adatta` to fit, `1:1` for one image pixel per physical screen pixel, double-click to re-fit, `Esc` to close.
- Decoded by a single dedicated background worker with a latest-wins policy; the oversize buffer is capped at 2560 px on the long edge.

### Color editor — non-destructive by default
- Adjustments are kept as an in-memory **recipe draft**, so changing photo or tab never rewrites the original. A color draft can still be saved explicitly to disk with **Salva**.
- Exposure (−5…+5 EV), brightness, contrast, highlights, shadows, whites, blacks, saturation, vibrance, temperature, tint, gamma, sharpness.
- Straighten-free geometry: crop with adjustable region and 90° rotation steps.
- Live preview computed on a reduced copy in a background worker; dragging a slider updates it in real time, undo is recorded on release rather than on every frame.
- Undo/redo (`Ctrl+Z`, `Ctrl+Shift+Z` / `Ctrl+Y`), "Prima" press-and-hold comparison, per-field reset and full reset.
- Named presets stored in the catalog; a preset can be applied to the current photo or to the whole multi-selection.
- Full-resolution pixels are produced only by export or by the explicit save flow; the editor is single-photo by design, while presets can be batch-applied.

### Metadata editor
- ExifTool runs as an external process on a dedicated thread; packages include their own copy, otherwise it is resolved from `UBERPHOTO_EXIFTOOL` or `PATH`.
- Descriptive fields: title, headline, description, caption, alt text, keywords, category, event, people, rating, label, notes, identifier.
- Author and rights fields: creator, copyright notice, credit line, source, publisher, license, usage terms, license URL, portfolio URL, e-mail, instructions.
- Temporal fields: capture date, time zone offset and EXIF orientation.
- Multi-selection editing: common values are shown, fields with different values are marked as mixed and are left untouched unless edited; clearing a field removes the tag.
- Embedded metadata for JPEG, PNG, TIFF and WebP; native IPTC only where the format supports it (JPEG/TIFF), with matching XMP tags everywhere else.
- RAW, HEIC and AVIF files are never rewritten: their XMP fields go to a `<file>.xmp` sidecar, created from the image's existing XMP or updated in place.
- Edited fields stay in a per-photo draft that survives selection changes until **Salva** is pressed. Post-write verification rereads every file and reports values that could not be confirmed; tags not managed by the editor are preserved.
- Advanced read-only view of all ExifTool tags with group, name, value, writability and a live filter.

### GPS picker
- OpenStreetMap map with a draggable marker; click to place, drag to refine.
- Manual latitude/longitude/altitude entry, decimal or DMS formatting, and "Centra" to recenter the map on the photo's coordinates.
- Multi-selection aware: mixed coordinates are flagged; the marker and place fields are kept as per-photo drafts and written to every selected photo by **Salva**.
- Optional place search through Nominatim, only on explicit user action, with the required User-Agent and attribution.
- Tiles are created lazily on first map open; no network access otherwise.

### Manual save
- **Salva** in the top bar (or `Ctrl+S`) opens a confirmation dialog for the selected photos that have drafts. The save bar also reports pending photos outside the selection, so they are never lost silently.
- **Nuova foto** creates `nome-modificata.ext` next to the original, with automatic `-2`, `-3`, … collision handling. The source file is never touched.
- **Sovrascrivi originale** rewrites color and metadata in place and is irreversible; it requires an explicit confirmation checkbox. For batches, the dialog reports partial failures and keeps only the failed photos for retry.
- Color changes are rendered to a temporary file in the same folder and atomically renamed over the original only after metadata application succeeds. Metadata-only overwrites are written by ExifTool and verified by reading the file back.
- JPEG, PNG, TIFF and WebP support new copies. RAW, HEIC and AVIF are never rewritten: their XMP metadata goes to the sidecar, and color edits cannot be applied.
- Metadata and GPS fields are verified after writing; the save is reported as successful only when the read-back matches.
- Closing the app with unsaved drafts asks for confirmation; closing while a save is running is refused with a status message until the batch finishes.

### Export
- Three profiles: **Full metadata** (default), **Web optimized** (editorial EXIF/IPTC/XMP whitelist), **Privacy** (removes GPS, serial number and author/creator fields).
- Formats: JPEG (quality 1–100), PNG, WebP (lossless) and TIFF.
- Non-sRGB images are converted to sRGB with LittleCMS and the generated sRGB profile is embedded in the export.
- Optional resize with a maximum long edge (never upscales); Lanczos3 resampling.
- Naming modes: keep name, add prefix, add suffix, web-safe slug; collisions get `-2`, `-3`, … and existing files are never overwritten.
- The color recipe is applied to full-resolution pixels, after ICC conversion and before resizing.
- Every export is verified: source and output tags are read back in one ExifTool call and the report classifies metadata as preserved, adjusted, unsupported or lost, with explicit warnings (for example WebP cannot store IPTC).
- Per-item progress, cancellation and a per-photo result list; failures never leave a partial file behind.
- Supported sources are JPEG, PNG, TIFF and WebP; RAW/HEIC/AVIF are listed in the browser but cannot be decoded, edited or exported yet.

### Filesystem operations
- Move or copy photos with drag & drop onto a folder in the explorer, or through the native destination dialog; rename photos and folders; undo the last move.
- Conflict handling with three policies: automatic rename (`photo (1).jpg`), replace, skip.
- XMP sidecars travel with their photo; cross-device moves fall back to copy + delete.
- Create and delete empty subfolders, rename folders from the explorer or context menu.
- Local SQLite index (`index.db` in the data directory) mirrors file info, image dimensions, ratings, keywords and camera data; WAL mode, incremental updates and upserts. Edit recipes and presets live in the same catalog.
- External changes are detected with `notify` (debounced); read-only access events are ignored, so browsing never triggers spurious rescans.

### Application
- Dark "photographic" theme: neutral chrome so photographs dominate the scene and colours are not biased by the surroundings.
- Welcome screen with the app logo; the same icon is used by the window / taskbar and by packaged desktop entries.
- Session persistence (recent folders, last folder, thumbnail size), logging via `tracing` with `RUST_LOG` control.
- Headless-friendly: `UBERPHOTO_SMOKE_EXIT_SECS` closes the app gracefully after N seconds for CI.

### Recognized formats

| Format | Browse/decode | Metadata write | Edit & export | Notes |
|---|---|---|---|---|
| JPEG, TIFF | ✅ | Embedded EXIF/XMP + native IPTC | ✅ | Full pipeline |
| PNG, WebP | ✅ | Embedded XMP | ✅ | IPTC has no native block; XMP tags are used. WebP export is lossless and the report flags IPTC as unsupported |
| GIF, BMP | ✅ | ⛔ | ⛔ | Browsing only |
| HEIC, AVIF | ⛔ | Sidecar XMP | ⛔ | Recognized and listed; decoding not enabled yet |
| DNG, CR2, CR3, NEF, ARW, RAF (RAW) | ⛔ | Sidecar XMP | ⛔ | Recognized and listed; originals are never rewritten — metadata goes to `<file>.xmp` |

Capabilities per format are declared explicitly in code (`FormatKind::capabilities`).

### Known limitations
- RAW/HEIC/AVIF decoding is not implemented (browse-only). Their XMP is edited through sidecars, never embedded.
- Edit recipes are not written into XMP sidecars (`XMP-crs`) yet; they live in the local catalog.
- WebP cannot store IPTC; export reports that explicitly instead of pretending success.
- The color editor is single-photo; batch color operations use presets.
- Interactive GUI behaviour is covered by headless egui tests and boot smoke tests, not by automated clicking.

---

## Requirements

- **Rust** stable, 1.85 or newer (developed with 1.98).
- **Ubuntu / Linux**: any X11 or Wayland desktop session (eframe uses winit + wgpu), plus a C toolchain (`gcc` / `build-essential`) to build the statically linked LittleCMS dependency.
- **macOS**: recent macOS versions on Intel or Apple Silicon (11+). Building requires Xcode Command Line Tools.
- **Windows**: Windows 10 or newer (release builds do not open a console window). Building requires the MSVC toolchain.
- **ExifTool 12+**: required for metadata read/write and export verification.
  - Release packages (`.deb`, macOS `.app`, Windows ZIP) **bundle ExifTool**, so no separate ExifTool installation is required. Linux and macOS run the bundled Perl library with the system `perl`; Windows ships the standalone ExifTool executable.
  - Development builds resolve it from `UBERPHOTO_EXIFTOOL`, then from a bundled copy, then from `PATH`.
    - Ubuntu/Debian: `sudo apt install libimage-exiftool-perl`
    - macOS: `brew install exiftool`
    - Windows: `choco install exiftool` or download from https://exiftool.org
  - Metadata tests skip automatically when ExifTool is absent.

## Installing release packages

Download the files from the [Releases](https://github.com/Ogalesha/UberPhoto/releases) page; no GitHub account is required.

### Linux — Debian/Ubuntu package

```bash
sudo apt install ./uberphoto_<version>_amd64.deb
```

`apt` resolves the dependencies and installs the desktop entry and icons. Remove it with `sudo apt remove uberphoto`.

### Linux — portable archive

```bash
tar -xzf uberphoto-linux-x86_64.tar.gz
./uberphoto
```

The archive contains ExifTool next to the executable, so nothing else is needed.

### Windows

Run `uberphoto-windows-x86_64-setup.exe` (per-user install, no administrator rights). If SmartScreen appears, choose **More info → Run anyway**. The portable `uberphoto-windows-x86_64.zip` can be extracted anywhere and started with `uberphoto.exe`; ExifTool is included.

### macOS

Extract `uberphoto-macos-<arch>.app.tar.gz`, then drag `UberPhoto.app` into **Applications**. On first launch, right-click the app and choose **Open → Open** (or use **System Settings → Privacy & Security → Open Anyway**). If macOS still blocks it, remove the quarantine flag:

```bash
xattr -dr com.apple.quarantine /Applications/UberPhoto.app
```

### Verify downloads

Every current release attaches a `SHA256SUMS` file:

```bash
sha256sum -c SHA256SUMS          # Linux
shasum -a 256 -c SHA256SUMS      # macOS
```

On Windows compare `Get-FileHash <file> -Algorithm SHA256` with the matching line.

Packages are complete and self-contained, but they are **not code-signed** (the project has no Apple Developer ID or Windows code-signing certificate). The warnings above are the only consequence: downloads and installation work normally.

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

# Manual benchmarks (ignored by default)
cargo test -p uberphoto-files -- --ignored --nocapture
cargo test -p uberphoto-image --release -- --ignored --nocapture

# Headless GUI smoke test (what CI runs on Linux via Xvfb)
UBERPHOTO_SMOKE_EXIT_SECS=6 xvfb-run -a ./target/release/uberphoto-app

# Build the Linux package locally (downloads the pinned ExifTool and bundles it)
bash packaging/linux/build-deb.sh
```

`UBERPHOTO_SMOKE_EXIT_SECS` makes the app close itself gracefully after N seconds; it exists so CI can verify that the GUI starts, renders and shuts down cleanly without a human.

## Repository layout

```
uberphoto/
├── assets/UberPhoto.png        # application icon (window, welcome screen, packages)
├── crates/
│   ├── uberphoto-app/          # binary entry point (window, icon, logging)
│   ├── uberphoto-ui/           # egui interface: panels, theme, caches wiring
│   ├── uberphoto-core/         # domain types: formats, capabilities, photos, GPS
│   ├── uberphoto-files/        # scanning, natural sort, folder tree
│   ├── uberphoto-image/        # decode, orientation, editing pipeline, background services
│   ├── uberphoto-metadata/     # ExifTool engine, canonical model, sidecars
│   ├── uberphoto-geo/          # OpenStreetMap/Nominatim integration
│   ├── uberphoto-db/           # SQLite catalog (index, recipes, presets)
│   ├── uberphoto-export/       # render, ICC conversion, metadata copy and verification
│   └── uberphoto-tests/        # cross-crate integration tests
├── packaging/                  # Linux .deb, macOS .app, Windows installer + portable ZIP
├── .github/workflows/          # CI and release pipelines
└── CHANGELOG.md                # Keep a Changelog 1.1.0
```

## Architecture & design decisions

- **The filesystem is the source of truth.** The SQLite index mirrors it for speed (sorting by capture date or rating, keyword/GPS filters) but never replaces it: deleting `index.db` simply rebuilds it on the next scan. Presets remain catalog data; color, metadata and GPS edits stay as drafts in memory until **Salva** writes them to disk.
- **Non-destructive by default.** Color edits are recipes and the recommended save mode always creates a new copy. The original is rewritten only after the user explicitly chooses **Sovrascrivi originale** and confirms it. RAW, HEIC and AVIF files are never rewritten; their metadata uses sidecar XMP.
- **Immediate-mode UI, background pipelines.** egui renders on the main thread; directory scans, thumbnail decoding (rayon pool sized to the machine), preview decoding, editing previews, metadata operations and export all run off-thread and deliver results through channels with **latest-wins** semantics, so stale work is discarded instead of blocking.
- **Only visible work is done.** Thumbnails are requested for cells currently on screen; the LRU cache (2,000 textures, quantized size buckets) keeps memory bounded.
- **Metadata reliability first.** All metadata read/write and export verification go through ExifTool as an external process with structured arguments (never a shell), so the same tag semantics apply on every platform.
- **Export is verified, not assumed.** After copying tags, UberPhoto rereads source and output together and reports what was preserved, adjusted, unsupported or lost. Metadata loss is surfaced, not hidden.
- **Self-contained packages.** Release artifacts bundle ExifTool (Perl library + wrapper on Linux/macOS, standalone executable on Windows), so installing UberPhoto does not require installing ExifTool. Only standard OS components remain: the system Perl on Linux/macOS and the desktop graphics stack.
- **Cross-platform by construction.** Pure-Rust dependencies everywhere the code touches files, images or the window (`eframe`/wgpu, `image`, `kamadak-exif`, `rfd` native dialogs); no platform-specific code except the Windows console-hiding attribute, the macOS bundle layout and test-only Unix symlink helpers.

## Testing and CI

- Unit and integration tests live next to the code plus a cross-crate smoke suite in `uberphoto-tests` (fixtures generated at runtime; no binary test assets in the repository).
- Integration smoke tests cover filesystem operations, metadata round-trip, GPS writing/removal, editing recipes and export with metadata verification. Save tests cover new-copy naming, atomic overwrite, metadata round-trip and rollback on error.
- CI (`.github/workflows/ci.yml`) runs on **Ubuntu, macOS and Windows**: `rustfmt --check`, `clippy -D warnings`, `cargo test --workspace`, a release build, packaging checks that bundle ExifTool, and — on Linux only — a headless GUI smoke test under Xvfb.
- Benchmark tests are `#[ignore]`d so they never slow the normal suite.

## Releases and versioning

- The project follows **[Semantic Versioning](https://semver.org/)** and keeps a **[Keep a Changelog](https://keepachangelog.com/en/1.1.0/)** file.
- The repository tracks **source code only**; compiled artifacts, local databases, logs and photographs are never committed (see `.gitignore`).
- Pushing a tag like `v0.3.0` triggers the release workflow, which builds and attaches **self-contained** packages:
  - `uberphoto_<version>_amd64.deb` — Debian/Ubuntu package with desktop entry and bundled ExifTool
  - `uberphoto-linux-x86_64.tar.gz` — portable Linux archive with bundled ExifTool
  - `uberphoto-macos-aarch64.app.tar.gz` — macOS application bundle with bundled ExifTool
  - `uberphoto-macos-x86_64.app.tar.gz` — Intel build (experimental)
  - `uberphoto-windows-x86_64-setup.exe` — Windows installer (Inno Setup) with Start Menu entry and standalone ExifTool
  - `uberphoto-windows-x86_64.zip` — portable Windows archive with standalone ExifTool
- Release artifacts are not code-signed: Windows SmartScreen and macOS Gatekeeper may show a warning; see [Installing release packages](#installing-release-packages) for the first-launch steps. Packages are otherwise complete and self-contained.
- Every workflow run also publishes build artifacts on GitHub, so compiled files are always obtainable without a local toolchain.

## Configuration and data

- UI preferences (recent folders, last folder, thumbnail size) are stored by eframe in RON format, typically at:
  - Linux: `~/.local/share/uberphoto/app.ron`
  - macOS: `~/Library/Application Support/uberphoto/app.ron`
  - Windows: `%APPDATA%\uberphoto\app.ron`
- The SQLite catalog (`index.db`, WAL mode) lives in the same data directory and holds the folder index plus edit recipes and presets. It is disposable: photos and their embedded metadata remain authoritative.
- Log verbosity follows `RUST_LOG` (default `info`).

## Availability

UberPhoto runs on Linux, macOS and Windows. Linux is the primary platform and its packages are tested end to end. macOS and Windows packages are published; their GUI smoke tests remain experimental in CI. Every release package bundles ExifTool, so no separate metadata tool is required.

## License

Licensed under the [MIT License](LICENSE). Copyright (c) 2026 Mirgen.

Release packages bundle [ExifTool](https://exiftool.org/) by Phil Harvey, distributed under the same terms as Perl itself (Artistic License or GPL). Its README is included in every package.

UberPhoto is a personal project; the source is public and contributions are welcome via issues and pull requests.
