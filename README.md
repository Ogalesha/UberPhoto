<p align="center"><img src="assets/UberPhoto.png" alt="UberPhoto" width="96"></p>

# UberPhoto

Browse, edit metadata, geotag and export photos directly from your folders. No library import, no account, no photo uploads.

**Use v0.4.1 or newer for the recovery protections described below.**

[Build the current version](#build-from-source) · [Published packages](https://github.com/Ogalesha/UberPhoto/releases/latest)

Linux · macOS · Windows. Release packages include ExifTool.

## What you can do

- **Organize:** browse real folders, rate and select photos, move or copy them with their XMP sidecars.
- **Describe:** edit EXIF, IPTC and XMP metadata; place photos on an OpenStreetMap map.
- **Adjust:** exposure, color, crop and rotation stay in a draft until you save or export.
- **Export:** JPEG, PNG, WebP or TIFF, with resize, metadata checks and privacy options.

## Keep your originals recoverable

**Save as a new photo is the default.** Replacing an existing photo requires confirmation and keeps a verified recovery copy in `.uberphoto-backups`. Metadata writes are checked before the original is replaced; new exports do not overwrite existing files.

Transfers and saves prepare the whole batch first. Interrupted batches recover from a persistent journal when the folder is reopened. In **Backup**, choose a physically separate disk for a second verified copy; a configured disk must be connected before files can be changed. [Recovery details](docs/RECOVERY.md).

## Know the limits

- JPEG, PNG, TIFF and WebP support the editing workflow.
- RAW, HEIC and AVIF use XMP sidecars for metadata; preview, color editing and export are not available yet.
- Image processing uses 8-bit color. Check the result before replacing a high-bit-depth original.
- Photos are processed locally. Maps and place search contact OpenStreetMap services.
- Packages are currently unsigned. Linux is the primary platform; macOS and Windows GUI smoke tests are experimental.

## Start

Open a folder, select a photo, then choose **Colors**, **Metadata** or **GPS**. Use **Save** to create a new photo, or **Export** to choose output settings.

[Installation, recovery and full guide](docs/GUIDE.md) · [Changelog](CHANGELOG.md) · [Report an issue](https://github.com/Ogalesha/UberPhoto/issues)

## Build from source

Rust 1.95+ and ExifTool are required for development. See the [guide](docs/GUIDE.md#requirements) for platform dependencies.

```sh
cargo run --locked -p uberphoto-app
```

[MIT License](LICENSE). Release packages also include [ExifTool](https://exiftool.org/) under its own license.
