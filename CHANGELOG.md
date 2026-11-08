# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.1] - 2026-11-08

### Safety and recovery

- Prepare complete photo/sidecar transfers and saves before changing any user file. Durable intent records allow interrupted batches to roll back when their folder is reopened. Outputs and displaced originals are retained rather than deleted.
- Add **Backup** in the top bar to choose a separate backup volume. Each destructive operation requires a verified second copy when configured; an unavailable disk blocks the operation, including after restart.
- Keep verified recovery copies for replacements, refuse self-copy and unsafe symbolic links, and verify metadata on private work files before publication.
- Read and merge existing XMP sidecars, preserve unknown tags, and retain drafts until explicit save. Block export with unsaved metadata/GPS and fix author privacy fields.
- Make scan, preview, geographic lookup, save, export, recovery and close transitions explicit; discard stale asynchronous results and retain filesystem changes received during writes.
- Update TLS dependencies and add dependency auditing to CI. Require successful package builds before publishing a release.

### Usability

- Add recovery copies to the Information tab, improve narrow save dialogs and filters, and simplify the GitHub README.
- Document recovery behavior and the difference between recoverable batches and instantaneous filesystem atomicity.

## [0.4.0] - 2026-11-07

### Added

- Complete uninstall on all platforms: `uberphoto --uninstall` deletes the index, settings, presets, recent folders and map cache from the user data directory; on Windows it removes the whole `%APPDATA%\UberPhoto` root, and the Windows uninstaller removes `%APPDATA%\UberPhoto` and `%LOCALAPPDATA%\UberPhoto`. The guide documents full removal on Linux, macOS and Windows.
- Aggregate folder view: opening a folder shows photos from that folder and all its subfolders, with a bar of direct subfolders above the grid to narrow the view to a branch.
- Unified manual save with **Save** in the top bar and `Ctrl+S`: a dialog lets you choose between a new copy (`name-edited.ext`, progressive collision handling) and overwriting the original with explicit confirmation.
- New save service in `uberphoto-export`: rendering to a temporary file in the same folder, verified metadata application, atomic replacement, rollback on failure and batch handling with partial errors.
- In-memory drafts for color, metadata and location, kept per photo even when the selection changes; saving includes only selected photos that have drafts.
- Save tests: automatic naming, overwrite with pixels and metadata, copy with metadata, rollback on error and collisions.
- Confirmation on close when unsaved drafts remain, and close blocked while a save is in progress.
- UI localized in Italian, English, German, French, Spanish, Polish, Turkish, Russian and Albanian, with a selector in the top bar, persisted choice and system language detection (environment variables with `sys-locale` fallback).
- Plural forms for count messages, with dedicated rules for Russian and Polish (`one|few|many`) and automated tests; summary messages without a count use number-neutral labels.
- Monochrome emoji fonts bundled into the binary so interface icons look identical on every platform; a test verifies coverage of every catalog and language-name character.
- Tests for the persisted language choice and placeholder coverage across all variants.

### Changed

- Color is no longer automatically saved to the catalog when changing photo or applying a preset: it stays a draft until the user presses **Save**.
- Metadata and GPS fields are no longer written immediately: they stay as drafts and are applied together by the manual save, with post-write verification.
- Export uses the current recipe drafts, so the color and the exported file stay consistent.
- Manual coordinate errors block saving until they are corrected.
- Color, metadata and location drafts follow file and folder renames and moves.
- Watching for external changes is recursive: the aggregate view also refreshes when a subfolder changes.
- UI strings now come from a translation catalog with Italian fallback; dynamic messages use placeholders and the catalog is covered by completeness and ordering tests.
- Save and export outcome messages no longer inflect the participle for a variable count: they use labels such as "succeeded: N" and "failed: N".

### Fixed

- Quick rating no longer clears metadata or GPS drafts after a re-read.
- Replacement on Windows no longer removes the original before the atomic rename.
- Metadata and GPS forms rebuild when documents become available again, instead of staying empty after a save or re-read.
- Metadata and GPS drafts are no longer lost when changing selection: they stay associated with the individual photo.
- After a save or re-read, the metadata cache is invalidated for files with XMP sidecars too, so verified values appear in the forms immediately.
- Fixed some translations: Polish "Location" is no longer "Stanowisko" (workplace), capitalization after semicolons in Turkish and Spanish, and singular/plural forms of photo and file across all languages.

### Documentation

- Repository documentation, CI workflows, packaging scripts and uninstall CLI messages are in English; the interface translation catalog remains localized.
- README updated with the manual save flow, rollback guarantees, the new meaning of draft recipes and the interface languages.
- README links to the latest release from the top of the page and documents the correct per-user data paths on macOS and Windows.
- Release tag example updated to `v0.4.0`.

## [0.3.0] - 2026-10-19

### Added

- Phase 4 — GPS picker on an OpenStreetMap map with draggable marker, click to place, manual coordinates and centering; optional place search on Nominatim only on explicit action, with User-Agent and attribution.
- Phase 5 — non-destructive editing: color recipe saved in the catalog (exposure, brightness, contrast, highlights, shadows, whites, blacks, saturation, vibrance, temperature, tint, gamma, sharpness), crop and rotation in 90° steps.
- Phase 5 — live preview on a reduced copy with a "latest wins" worker, "Before" comparison, undo/redo and per-field reset.
- Phase 5 — color presets saved in the catalog, applicable to the current photo or the whole multi-selection.
- Phase 6 — new `uberphoto-export` crate: Full metadata, Web optimized and Privacy profiles, JPEG/PNG/WebP/TIFF formats, ICC to sRGB conversion with embedded profile, Lanczos3 resizing without upscaling, naming with prefix/suffix/slug and conflict handling without overwriting.
- Phase 6 — export verification: source and output tags are read back together and classified as preserved, adjusted, unsupported or lost, with explicit warnings (for example IPTC cannot be stored in WebP).
- Phase 6 — Export tab with profile, format, quality, resizing, destination, progress, cancellation and per-photo report; partial files are removed on error.
- Phase 7 — installable, self-contained packages: Debian/Ubuntu `.deb` with desktop entry, macOS `.app` bundle, Windows installer (Inno Setup) and portable ZIP, all with ExifTool included; portable Linux archive with ExifTool.
- Phase 7 — GPS integration smoke test (Phase 4): write and read back the marker with altitude, removal without touching unrelated tags, rejection of out-of-range coordinates.
- Phase 7 — ExifTool is looked up first next to the executable (copy bundled in the package), then in `UBERPHOTO_EXIFTOOL`, and finally in `PATH`.
- Phase 7 — manual benchmarks for the color pipeline on 24 MP and for scanning 2,000 files; package verification in CI.
- Phase 7 — README updated for Phases 1–7 with features, known limitations and packaging instructions.

### Fixed

- The Colors tab applies presets to a multi-selection instead of merely declaring it unsupported.
- `.gitignore` excludes secrets (`.env`, keys), photographs, local databases, logs and the private product spec; only sources, public documentation and the icon required for the build remain in the repository.

## [0.2.1] - 2026-10-18

### Fixed

- On Windows, index folder operations (`load_dir`, `move_dir`, `remove_dir`) did not find rows when paths mixed `/` and `\` separators: the LIKE pattern is now a separator-independent prefix, with exact filtering in Rust.
- CI did not publish logs of failed tests (the log now goes to a dedicated check run visible without authentication).
- The Linux GUI smoke test stopped under Xvfb because `libxkbcommon-x11` failed to load (package now installed) and because the Xvfb screen was 8-bit (now 24-bit).

## [0.2.0] - 2026-10-18

### Added

- Phase 2 — filesystem operations: move, copy and rename with three conflict policies (automatic rename, replace, skip), undo of the last move and copy+delete fallback across different devices.
- Phase 2 — drag & drop of photos from the grid to explorer folders, with a count pill and confirmation prompt.
- Phase 2 — XMP sidecar handling in file operations: the sidecar follows the moved or renamed photo.
- Phase 2 — creation and deletion of empty subfolders and folder rename from the explorer.
- Phase 2 — local SQLite index (`index.db`) in WAL mode: files, image dimensions, rating, capture date, keywords, camera data and GPS; incremental updates and automatic rebuild.
- Phase 2 — external change detection with `notify` and throttled rescanning; read-only events no longer trigger rescans.
- Phase 2 — sorting by acquisition date and rating through the index.
- Phase 3 — new `uberphoto-metadata` crate: adapter for the ExifTool process with structured arguments, discovery from `PATH` or `UBERPHOTO_EXIFTOOL`, batch reading and writing with post-write verification.
- Phase 3 — canonical model with 26 editable fields (descriptive, author/copyright, date and orientation), validation of rating, orientation, date and time zone.
- Phase 3 — embedded writing for JPEG, PNG, TIFF and WebP; native IPTC only on JPEG/TIFF; XMP sidecars for RAW, HEIC and AVIF without ever rewriting the original.
- Phase 3 — metadata editor in the inspector with multi-selection, mixed values, modified-field indicator, saving and reloading.
- Phase 3 — read-only view of technical camera data and raw ExifTool tags with filter and writability indicator.
- Phase 3 — GPS badge and stars in the grid, filters by minimum rating, GPS and keywords, quick rating from the context menu.
- Integration tests for Phases 2 and 3 (`phase2_smoke`, `phase3_smoke`) and E2E smoke tests with a populated index and metadata.
- CI: ExifTool installation on Linux and macOS (best effort on Windows), experimental GUI smoke tests on macOS and Windows.

### Fixed

- The watcher triggered a rescan loop because file open/read events were treated as modifications.
- Index updates applied `move_path` to copies as well, corrupting the source photo path.

## [0.1.0] - 2026-10-18

### Added

- Phase 1 — foundations: Cargo workspace with the `uberphoto-{core,files,image,ui,app,metadata,geo,db,tests}` crates.
- Left panel: tree explorer with lazy branch loading, recent folders and opening a new root through the native dialog.
- Center panel: virtualized thumbnail grid with single and multiple selection (Ctrl/Cmd and Shift), name filter, sorting and adjustable thumbnail size.
- Right panel: large preview with zoom, pan and fit-to-window; placeholder tabs, with real data in the "Information" tab.
- Thumbnails and previews generated on background threads with LRU cache, mtime change detection and EXIF orientation.
- Professional dark photography theme and welcome screen with logo.
- Persistence of recent folders, last opened folder and thumbnail size.
- Application icon (window and welcome screen).
- GitHub Actions CI on Ubuntu, macOS and Windows (format, clippy, tests) and release workflow with downloadable binaries for `v*` tags.
- Headless GUI smoke test (Xvfb) in Linux CI.

[Unreleased]: https://github.com/Ogalesha/UberPhoto/compare/v0.4.0...main
[0.4.0]: https://github.com/Ogalesha/UberPhoto/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Ogalesha/UberPhoto/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/Ogalesha/UberPhoto/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/Ogalesha/UberPhoto/compare/v0.1.0...v0.2.0
