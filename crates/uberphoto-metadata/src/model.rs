//! Modello canonico dei metadati e mappatura esplicita verso i tag
//! EXIF / IPTC / XMP.
//!
//! La lettura usa come chiavi i gruppi di famiglia 1 di ExifTool
//! (`XMP-dc:Title`, `IFD0:Artist`, ...) con priorità documentate; la
//! scrittura produce assegnazioni `Gruppo:Tag=valore` filtrate per formato
//! (l'IPTC esiste solo in JPEG/TIFF) e per destinazione (i sidecar XMP
//! ricevono soltanto tag XMP).

use std::collections::BTreeMap;

use serde_json::Value;
use uberphoto_core::FormatKind;

use crate::exiftool::TagMap;

/// Campi canonici editabili dell'editor metadati.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MetadataField {
    Title,
    Headline,
    Description,
    Caption,
    AltText,
    Keywords,
    Category,
    Event,
    People,
    Rating,
    Label,
    Notes,
    Identifier,
    Creator,
    Copyright,
    Credit,
    Source,
    Publisher,
    License,
    UsageTerms,
    LicenseUrl,
    PortfolioUrl,
    Email,
    Instructions,
    DateOriginal,
    OffsetTime,
    Orientation,
}

/// Sezione dell'editor in cui appare il campo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Descriptive,
    Author,
    Temporal,
}

impl MetadataField {
    pub const ALL: [Self; 27] = [
        Self::Title,
        Self::Headline,
        Self::Description,
        Self::Caption,
        Self::AltText,
        Self::Keywords,
        Self::Category,
        Self::Event,
        Self::People,
        Self::Rating,
        Self::Label,
        Self::Notes,
        Self::Identifier,
        Self::Creator,
        Self::Copyright,
        Self::Credit,
        Self::Source,
        Self::Publisher,
        Self::License,
        Self::UsageTerms,
        Self::LicenseUrl,
        Self::PortfolioUrl,
        Self::Email,
        Self::Instructions,
        Self::DateOriginal,
        Self::OffsetTime,
        Self::Orientation,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Title => "Titolo",
            Self::Headline => "Headline",
            Self::Description => "Descrizione",
            Self::Caption => "Didascalia",
            Self::AltText => "Testo alternativo",
            Self::Keywords => "Parole chiave",
            Self::Category => "Categoria",
            Self::Event => "Evento",
            Self::People => "Persone raffigurate",
            Self::Rating => "Valutazione",
            Self::Label => "Etichetta colore",
            Self::Notes => "Note",
            Self::Identifier => "Identificativo",
            Self::Creator => "Autore",
            Self::Copyright => "Copyright",
            Self::Credit => "Credit line",
            Self::Source => "Fonte",
            Self::Publisher => "Editore",
            Self::License => "Licenza",
            Self::UsageTerms => "Termini d'uso",
            Self::LicenseUrl => "URL licenza",
            Self::PortfolioUrl => "URL portfolio / sito",
            Self::Email => "Email di contatto",
            Self::Instructions => "Istruzioni d'uso",
            Self::DateOriginal => "Data di scatto",
            Self::OffsetTime => "Fuso orario",
            Self::Orientation => "Orientamento",
        }
    }

    pub fn section(self) -> Section {
        match self {
            Self::Title
            | Self::Headline
            | Self::Description
            | Self::Caption
            | Self::AltText
            | Self::Keywords
            | Self::Category
            | Self::Event
            | Self::People
            | Self::Rating
            | Self::Label
            | Self::Notes
            | Self::Identifier => Section::Descriptive,
            Self::Creator
            | Self::Copyright
            | Self::Credit
            | Self::Source
            | Self::Publisher
            | Self::License
            | Self::UsageTerms
            | Self::LicenseUrl
            | Self::PortfolioUrl
            | Self::Email
            | Self::Instructions => Section::Author,
            Self::DateOriginal | Self::OffsetTime | Self::Orientation => Section::Temporal,
        }
    }

    /// True per i campi con più valori (lista separata da virgole nella UI).
    pub fn is_list(self) -> bool {
        matches!(self, Self::Keywords | Self::People)
    }

    /// Suggerimento di compilazione mostrato nella UI.
    pub fn hint(self) -> &'static str {
        match self {
            Self::Keywords => "valori separati da virgole",
            Self::Rating => "0–5 stelle",
            Self::DateOriginal => "AAAA:MM:GG hh:mm:ss",
            Self::OffsetTime => "+HH:MM",
            Self::Orientation => "1–8 (EXIF)",
            Self::People => "nomi separati da virgole",
            _ => "",
        }
    }
}

/// Posizione GPS in gradi decimali (latitudine a sud e longitudine a ovest
/// sono negative).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpsPosition {
    pub latitude: f64,
    pub longitude: f64,
}

/// Informazioni tecniche prodotte dalla fotocamera: sola lettura, per non
/// falsificare accidentalmente la provenienza del file.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TechnicalInfo {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub focal_length: Option<String>,
    pub f_number: Option<String>,
    pub exposure_time: Option<String>,
    pub iso: Option<i64>,
    pub color_profile: Option<String>,
    pub date_created: Option<String>,
    pub date_modified: Option<String>,
}

/// Documento canonico: valori scalari, liste e dati tecnici.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MetadataDocument {
    values: BTreeMap<MetadataField, String>,
    lists: BTreeMap<MetadataField, Vec<String>>,
    pub technical: TechnicalInfo,
    pub gps: Option<GpsPosition>,
}

impl MetadataDocument {
    pub fn value(&self, field: MetadataField) -> Option<&str> {
        self.values.get(&field).map(String::as_str)
    }

    pub fn list(&self, field: MetadataField) -> &[String] {
        self.lists.get(&field).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Valore pronto per la UI: liste unite da virgole.
    pub fn display(&self, field: MetadataField) -> String {
        if field.is_list() {
            self.list(field).join(", ")
        } else {
            self.value(field).unwrap_or_default().to_owned()
        }
    }

    pub fn rating(&self) -> Option<u8> {
        self.value(MetadataField::Rating)
            .and_then(|v| v.parse::<i64>().ok())
            .and_then(|v| u8::try_from(v).ok())
            .filter(|v| *v <= 5)
    }

    fn set_value(&mut self, field: MetadataField, value: Option<String>) {
        match value {
            Some(value) if !value.is_empty() => {
                self.values.insert(field, value);
            }
            _ => {
                self.values.remove(&field);
            }
        }
    }

    fn set_list(&mut self, field: MetadataField, items: Vec<String>) {
        if items.is_empty() {
            self.lists.remove(&field);
        } else {
            self.lists.insert(field, items);
        }
    }
}

// ---------------------------------------------------------------------------
// Lettura
// ---------------------------------------------------------------------------

fn string_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        }
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(if *flag { "sì" } else { "no" }.to_owned()),
        Value::Array(items) => {
            let joined: Vec<String> = items.iter().filter_map(string_value).collect();
            (!joined.is_empty()).then(|| joined.join(", "))
        }
        _ => None,
    }
}

fn number_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn list_value(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items.iter().filter_map(string_value).collect(),
        other => string_value(other).into_iter().collect(),
    }
}

fn lookup<'a>(tags: &'a TagMap, keys: &[&str]) -> Option<&'a Value> {
    keys.iter()
        .find_map(|key| tags.get(*key))
        .filter(|value| !value.is_null())
}

fn text(tags: &TagMap, keys: &[&str]) -> Option<String> {
    lookup(tags, keys).and_then(string_value)
}

fn list(tags: &TagMap, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .find_map(|key| tags.get(*key))
        .map(list_value)
        .unwrap_or_default()
}

fn number(tags: &TagMap, keys: &[&str]) -> Option<f64> {
    lookup(tags, keys).and_then(number_value)
}

/// Valore numerico con segno dedotto dal riferimento EXIF (`S`/`W`).
fn gps_axis(tags: &TagMap, value_keys: &[&str], ref_keys: &[&str]) -> Option<f64> {
    let value = number(tags, value_keys)?;
    let reference = text(tags, ref_keys).unwrap_or_default();
    let negative = reference.starts_with('S') || reference.starts_with('W');
    Some(if negative { -value } else { value })
}

fn contact_value(tags: &TagMap, key: &str, flat_keys: &[&str]) -> Option<String> {
    if let Some(Value::Object(contact)) = tags.get("XMP-iptcCore:CreatorContactInfo") {
        if let Some(value) = contact.get(key).and_then(string_value) {
            return Some(value);
        }
    }
    text(tags, flat_keys)
}

fn mm(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{} mm", value.round() as i64)
    } else {
        format!("{value:.1} mm")
    }
}

fn aperture(value: f64) -> String {
    let mut text = format!("{value:.1}");
    if let Some(stripped) = text.strip_suffix(".0") {
        text = stripped.to_owned();
    }
    format!("f/{text}")
}

fn seconds(value: f64) -> String {
    if value > 0.0 && value < 1.0 {
        format!("1/{} s", (1.0 / value).round() as i64)
    } else if (value - value.round()).abs() < 0.001 {
        format!("{} s", value.round() as i64)
    } else {
        format!("{value:.1} s")
    }
}

/// Costruisce il documento canonico da una mappa di tag ExifTool.
pub fn document_from_tags(tags: &TagMap) -> MetadataDocument {
    let mut document = MetadataDocument::default();

    document.set_value(
        MetadataField::Title,
        text(tags, &["XMP-dc:Title", "IPTC:ObjectName"]),
    );
    document.set_value(
        MetadataField::Headline,
        text(tags, &["XMP-photoshop:Headline", "IPTC:Headline"]),
    );
    document.set_value(
        MetadataField::Description,
        text(
            tags,
            &[
                "XMP-dc:Description",
                "IFD0:ImageDescription",
                "EXIF:ImageDescription",
                "IPTC:Caption-Abstract",
            ],
        ),
    );
    document.set_value(
        MetadataField::Caption,
        text(tags, &["IPTC:Caption-Abstract"]),
    );
    document.set_value(
        MetadataField::AltText,
        text(tags, &["XMP-iptcCore:AltTextAccessibility"]),
    );
    document.set_list(
        MetadataField::Keywords,
        list(tags, &["XMP-dc:Subject", "IPTC:Keywords"]),
    );
    document.set_value(
        MetadataField::Category,
        text(tags, &["XMP-photoshop:Category", "IPTC:Category"]),
    );
    document.set_value(MetadataField::Event, text(tags, &["XMP-iptcExt:Event"]));
    document.set_list(
        MetadataField::People,
        list(tags, &["XMP-iptcExt:PersonInImage"]),
    );
    document.set_value(
        MetadataField::Rating,
        number(tags, &["XMP-xmp:Rating"]).and_then(|rating| {
            let rating = rating.round() as i64;
            (0..=5).contains(&rating).then(|| rating.to_string())
        }),
    );
    document.set_value(MetadataField::Label, text(tags, &["XMP-xmp:Label"]));
    document.set_value(
        MetadataField::Notes,
        text(tags, &["EXIF:UserComment", "XMP-exif:UserComment"]),
    );
    document.set_value(
        MetadataField::Identifier,
        text(tags, &["XMP-dc:Identifier", "XMP-xmp:Identifier"]),
    );

    document.set_value(
        MetadataField::Creator,
        text(
            tags,
            &[
                "XMP-dc:Creator",
                "IFD0:Artist",
                "EXIF:Artist",
                "IPTC:By-line",
            ],
        ),
    );
    document.set_value(
        MetadataField::Copyright,
        text(
            tags,
            &[
                "XMP-dc:Rights",
                "IFD0:Copyright",
                "EXIF:Copyright",
                "IPTC:CopyrightNotice",
            ],
        ),
    );
    document.set_value(
        MetadataField::Credit,
        text(tags, &["XMP-photoshop:Credit", "IPTC:Credit"]),
    );
    document.set_value(
        MetadataField::Source,
        text(tags, &["XMP-photoshop:Source", "IPTC:Source"]),
    );
    document.set_value(MetadataField::Publisher, text(tags, &["XMP-dc:Publisher"]));
    document.set_value(MetadataField::License, text(tags, &["XMP-cc:License"]));
    document.set_value(
        MetadataField::UsageTerms,
        text(tags, &["XMP-xmpRights:UsageTerms"]),
    );
    document.set_value(
        MetadataField::LicenseUrl,
        text(tags, &["XMP-xmpRights:WebStatement"]),
    );
    document.set_value(
        MetadataField::PortfolioUrl,
        contact_value(
            tags,
            "CiUrlWork",
            &[
                "XMP-iptcCore:CreatorContactInfoCiUrlWork",
                "XMP-iptcCore:CreatorWorkURL",
            ],
        ),
    );
    document.set_value(
        MetadataField::Email,
        contact_value(
            tags,
            "CiEmailWork",
            &[
                "XMP-iptcCore:CreatorContactInfoCiEmailWork",
                "XMP-iptcCore:CreatorWorkEmail",
            ],
        ),
    );
    document.set_value(
        MetadataField::Instructions,
        text(
            tags,
            &["XMP-photoshop:Instructions", "IPTC:SpecialInstructions"],
        ),
    );

    document.set_value(
        MetadataField::DateOriginal,
        text(
            tags,
            &["EXIF:DateTimeOriginal", "XMP-photoshop:DateCreated"],
        ),
    );
    document.set_value(
        MetadataField::OffsetTime,
        text(
            tags,
            &[
                "EXIF:OffsetTimeOriginal",
                "EXIF:OffsetTimeDigitized",
                "EXIF:OffsetTime",
            ],
        ),
    );
    document.set_value(
        MetadataField::Orientation,
        number(
            tags,
            &["IFD0:Orientation", "EXIF:Orientation", "IFD1:Orientation"],
        )
        .map(|value| (value.round() as i64).to_string()),
    );

    document.technical = TechnicalInfo {
        make: text(tags, &["IFD0:Make", "EXIF:Make", "XMP-tiff:Make"]),
        model: text(tags, &["IFD0:Model", "EXIF:Model", "XMP-tiff:Model"]),
        lens: text(
            tags,
            &[
                "EXIF:LensModel",
                "EXIF:Lens",
                "XMP-aux:Lens",
                "XMP-exifEX:LensModel",
            ],
        ),
        focal_length: number(tags, &["EXIF:FocalLength"]).map(mm),
        f_number: number(tags, &["EXIF:FNumber"]).map(aperture),
        exposure_time: number(tags, &["EXIF:ExposureTime"]).map(seconds),
        iso: number(
            tags,
            &[
                "EXIF:ISO",
                "EXIF:ISOSetting",
                "EXIF:RecommendedExposureIndex",
            ],
        )
        .map(|value| value.round() as i64),
        color_profile: text(tags, &["ICC_Profile:ProfileDescription", "EXIF:ColorSpace"]),
        date_created: text(tags, &["EXIF:CreateDate", "XMP-xmp:CreateDate"]),
        date_modified: text(tags, &["EXIF:ModifyDate", "XMP-xmp:ModifyDate"]),
    };

    let latitude = number(tags, &["Composite:GPSLatitude"]).or_else(|| {
        gps_axis(
            tags,
            &["GPS:GPSLatitude", "EXIF:GPSLatitude"],
            &["GPS:GPSLatitudeRef", "EXIF:GPSLatitudeRef"],
        )
    });
    let longitude = number(tags, &["Composite:GPSLongitude"]).or_else(|| {
        gps_axis(
            tags,
            &["GPS:GPSLongitude", "EXIF:GPSLongitude"],
            &["GPS:GPSLongitudeRef", "EXIF:GPSLongitudeRef"],
        )
    });
    document.gps = match (latitude, longitude) {
        (Some(latitude), Some(longitude)) => Some(GpsPosition {
            latitude,
            longitude,
        }),
        _ => None,
    };

    document
}

// ---------------------------------------------------------------------------
// Riassunto per l'indice SQLite
// ---------------------------------------------------------------------------

/// Campi dei metadati usati dall'indice e dai filtri del browser.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MetadataSummary {
    pub has_gps: bool,
    pub rating: Option<u8>,
    pub date_original: Option<String>,
    pub keywords: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<i64>,
    pub f_number: Option<String>,
    pub exposure_time: Option<String>,
    pub focal_length: Option<String>,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
}

pub fn summary_from_tags(tags: &TagMap) -> MetadataSummary {
    let document = document_from_tags(tags);
    let keywords = document.list(MetadataField::Keywords);
    MetadataSummary {
        has_gps: document.gps.is_some(),
        rating: document.rating(),
        date_original: document
            .value(MetadataField::DateOriginal)
            .map(str::to_owned),
        keywords: (!keywords.is_empty()).then(|| keywords.join(", ")),
        camera_make: document.technical.make.clone(),
        camera_model: document.technical.model.clone(),
        lens: document.technical.lens.clone(),
        iso: document.technical.iso,
        f_number: document.technical.f_number.clone(),
        exposure_time: document.technical.exposure_time.clone(),
        focal_length: document.technical.focal_length.clone(),
        gps_lat: document.gps.map(|gps| gps.latitude),
        gps_lon: document.gps.map(|gps| gps.longitude),
    }
}

// ---------------------------------------------------------------------------
// Scrittura
// ---------------------------------------------------------------------------

/// L'IPTC nativo esiste solo in JPEG e TIFF; gli altri formati ricevono
/// gli stessi valori nei tag XMP.
fn iptc_supportato(format: FormatKind) -> bool {
    matches!(format, FormatKind::Jpeg | FormatKind::Tiff)
}

fn embedded_tags(field: MetadataField) -> &'static [&'static str] {
    match field {
        MetadataField::Title => &["XMP-dc:Title", "IPTC:ObjectName"],
        MetadataField::Headline => &["XMP-photoshop:Headline", "IPTC:Headline"],
        MetadataField::Description => &["XMP-dc:Description", "EXIF:ImageDescription"],
        MetadataField::Caption => &["IPTC:Caption-Abstract"],
        MetadataField::AltText => &["XMP-iptcCore:AltTextAccessibility"],
        MetadataField::Keywords => &["XMP-dc:Subject", "IPTC:Keywords"],
        MetadataField::Category => &["XMP-photoshop:Category", "IPTC:Category"],
        MetadataField::Event => &["XMP-iptcExt:Event"],
        MetadataField::People => &["XMP-iptcExt:PersonInImage"],
        MetadataField::Rating => &["XMP-xmp:Rating"],
        MetadataField::Label => &["XMP-xmp:Label"],
        MetadataField::Notes => &["EXIF:UserComment"],
        MetadataField::Identifier => &["XMP-dc:Identifier"],
        MetadataField::Creator => &["XMP-dc:Creator", "EXIF:Artist", "IPTC:By-line"],
        MetadataField::Copyright => &["XMP-dc:Rights", "EXIF:Copyright", "IPTC:CopyrightNotice"],
        MetadataField::Credit => &["XMP-photoshop:Credit", "IPTC:Credit"],
        MetadataField::Source => &["XMP-photoshop:Source", "IPTC:Source"],
        MetadataField::Publisher => &["XMP-dc:Publisher"],
        MetadataField::License => &["XMP-cc:License"],
        MetadataField::UsageTerms => &["XMP-xmpRights:UsageTerms"],
        MetadataField::LicenseUrl => &["XMP-xmpRights:WebStatement"],
        MetadataField::PortfolioUrl => &["XMP-iptcCore:CreatorWorkURL"],
        MetadataField::Email => &["XMP-iptcCore:CreatorWorkEmail"],
        MetadataField::Instructions => &["XMP-photoshop:Instructions", "IPTC:SpecialInstructions"],
        MetadataField::DateOriginal => &["EXIF:DateTimeOriginal"],
        MetadataField::OffsetTime => &["EXIF:OffsetTimeOriginal"],
        MetadataField::Orientation => &["EXIF:Orientation"],
    }
}

/// Nei sidecar XMP finiscono soltanto tag XMP; i campi senza equivalente
/// XMP (didascalia IPTC, fuso orario, orientamento) vengono saltati.
fn sidecar_tags(field: MetadataField) -> &'static [&'static str] {
    match field {
        MetadataField::Title => &["XMP-dc:Title"],
        MetadataField::Headline => &["XMP-photoshop:Headline"],
        MetadataField::Description => &["XMP-dc:Description"],
        MetadataField::Caption => &[],
        MetadataField::AltText => &["XMP-iptcCore:AltTextAccessibility"],
        MetadataField::Keywords => &["XMP-dc:Subject"],
        MetadataField::Category => &["XMP-photoshop:Category"],
        MetadataField::Event => &["XMP-iptcExt:Event"],
        MetadataField::People => &["XMP-iptcExt:PersonInImage"],
        MetadataField::Rating => &["XMP-xmp:Rating"],
        MetadataField::Label => &["XMP-xmp:Label"],
        MetadataField::Notes => &["XMP-exif:UserComment"],
        MetadataField::Identifier => &["XMP-dc:Identifier"],
        MetadataField::Creator => &["XMP-dc:Creator"],
        MetadataField::Copyright => &["XMP-dc:Rights"],
        MetadataField::Credit => &["XMP-photoshop:Credit"],
        MetadataField::Source => &["XMP-photoshop:Source"],
        MetadataField::Publisher => &["XMP-dc:Publisher"],
        MetadataField::License => &["XMP-cc:License"],
        MetadataField::UsageTerms => &["XMP-xmpRights:UsageTerms"],
        MetadataField::LicenseUrl => &["XMP-xmpRights:WebStatement"],
        MetadataField::PortfolioUrl => &["XMP-iptcCore:CreatorWorkURL"],
        MetadataField::Email => &["XMP-iptcCore:CreatorWorkEmail"],
        MetadataField::Instructions => &["XMP-photoshop:Instructions"],
        MetadataField::DateOriginal => &["XMP-photoshop:DateCreated"],
        MetadataField::OffsetTime => &[],
        MetadataField::Orientation => &[],
    }
}

fn normalizza_data(valore: &str) -> Result<String, String> {
    let normalizzato = valore.trim().replace('T', " ");
    let mut parti = normalizzato.split_whitespace();
    let data = parti.next().ok_or("data mancante")?;
    let ora = parti.next();

    let mut numeri_data = Vec::with_capacity(3);
    for pezzo in data.split([':', '-', '/']) {
        numeri_data.push(pezzo.parse::<u32>().map_err(|_| "data non valida")?);
    }
    if numeri_data.len() != 3 {
        return Err("usa il formato AAAA:MM:GG hh:mm:ss".to_owned());
    }
    let (anno, mese, giorno) = (numeri_data[0], numeri_data[1], numeri_data[2]);

    let (mut ore, mut minuti, mut secondi) = (0u32, 0u32, 0u32);
    if let Some(ora) = ora {
        let mut numeri_ora = Vec::with_capacity(3);
        for pezzo in ora.split(':') {
            numeri_ora.push(pezzo.parse::<u32>().map_err(|_| "orario non valido")?);
        }
        if numeri_ora.is_empty() || numeri_ora.len() > 3 {
            return Err("usa il formato AAAA:MM:GG hh:mm:ss".to_owned());
        }
        ore = numeri_ora[0];
        minuti = numeri_ora.get(1).copied().unwrap_or(0);
        secondi = numeri_ora.get(2).copied().unwrap_or(0);
    }

    if !(1000..=9999).contains(&anno)
        || !(1..=12).contains(&mese)
        || !(1..=31).contains(&giorno)
        || ore > 23
        || minuti > 59
        || secondi > 60
    {
        return Err("data o orario fuori intervallo".to_owned());
    }

    Ok(format!(
        "{anno:04}:{mese:02}:{giorno:02} {ore:02}:{minuti:02}:{secondi:02}"
    ))
}

fn normalizza_offset(valore: &str) -> Result<String, String> {
    let trimmed = valore.trim();
    let bytes = trimmed.as_bytes();
    let valido = match bytes {
        [sign, h1, h2, b':', m1, m2] => {
            matches!(sign, b'+' | b'-')
                && h1.is_ascii_digit()
                && h2.is_ascii_digit()
                && m1.is_ascii_digit()
                && m2.is_ascii_digit()
        }
        _ => false,
    };
    if !valido {
        return Err("usa il formato +HH:MM".to_owned());
    }
    let ore: u32 = trimmed[1..3].parse().unwrap_or(99);
    let minuti: u32 = trimmed[4..6].parse().unwrap_or(99);
    if ore > 14 || minuti > 59 {
        return Err("fuso orario fuori intervallo".to_owned());
    }
    Ok(trimmed.to_owned())
}

/// Trasforma la modifica di un campo nelle assegnazioni ExifTool
/// (`Gruppo:Tag=valore`); `None` o stringa vuota cancellano il campo.
pub fn changes_for(
    field: MetadataField,
    value: Option<&str>,
    format: FormatKind,
    sidecar: bool,
) -> Result<Vec<String>, String> {
    let grezzo = value.map(str::trim).filter(|text| !text.is_empty());

    let normalizzato = match field {
        MetadataField::Rating => match grezzo {
            None => None,
            Some(text) => {
                let rating: i64 = text
                    .parse()
                    .map_err(|_| "la valutazione va da 0 a 5 stelle".to_owned())?;
                if !(0..=5).contains(&rating) {
                    return Err("la valutazione va da 0 a 5 stelle".to_owned());
                }
                Some(rating.to_string())
            }
        },
        MetadataField::Orientation => match grezzo {
            None => None,
            Some(text) => {
                let orientation: i64 = text
                    .parse()
                    .map_err(|_| "l'orientamento va da 1 a 8".to_owned())?;
                if !(1..=8).contains(&orientation) {
                    return Err("l'orientamento va da 1 a 8".to_owned());
                }
                Some(orientation.to_string())
            }
        },
        MetadataField::DateOriginal => match grezzo {
            None => None,
            Some(text) => Some(normalizza_data(text)?),
        },
        MetadataField::OffsetTime => match grezzo {
            None => None,
            Some(text) => Some(normalizza_offset(text)?),
        },
        _ => grezzo.map(str::to_owned),
    };

    let tags = if sidecar {
        sidecar_tags(field)
    } else {
        embedded_tags(field)
    };
    let scrivibile_iptc = !sidecar && iptc_supportato(format);

    let mut assegnazioni = Vec::new();
    for tag in tags {
        if tag.starts_with("IPTC:") && !scrivibile_iptc {
            continue;
        }
        if field.is_list() {
            assegnazioni.push(format!("{tag}="));
            if let Some(valore) = normalizzato.as_deref() {
                for item in valore
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                {
                    assegnazioni.push(format!("{tag}={item}"));
                }
            }
        } else {
            assegnazioni.push(format!("{tag}={}", normalizzato.as_deref().unwrap_or("")));
        }
    }
    Ok(assegnazioni)
}

// ---------------------------------------------------------------------------
// Tag grezzi (modalità avanzata)
// ---------------------------------------------------------------------------

/// Tag grezzo per la vista avanzata dell'editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTag {
    pub group: String,
    pub name: String,
    pub value: String,
    pub writable: bool,
}

fn display_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => (if *flag { "true" } else { "false" }).to_owned(),
        Value::Array(items) => items
            .iter()
            .map(display_value)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(_) => serde_json::to_string(value).unwrap_or_default(),
        Value::Null => "—".to_owned(),
    }
}

/// Gruppi di sola lettura o calcolati da ExifTool.
fn writable_hint(group: &str) -> bool {
    !matches!(
        group,
        "ExifTool"
            | "File"
            | "System"
            | "Composite"
            | "ICC_Profile"
            | "JUMBF"
            | "CBOR"
            | "Jpeg2000"
            | "PNG"
            | "GIF"
    )
}

/// Converte la mappa completa dei tag nella vista avanzata ordinata.
pub fn raw_tags_from(tags: &TagMap) -> Vec<RawTag> {
    let mut out: Vec<RawTag> = tags
        .iter()
        .filter(|(key, _)| key.as_str() != "SourceFile")
        .filter(|(key, _)| !key.ends_with(":Error") && !key.ends_with(":Warning"))
        .map(|(key, value)| {
            let (group, name) = key.split_once(':').unwrap_or(("", key.as_str()));
            RawTag {
                group: group.to_owned(),
                name: name.to_owned(),
                value: display_value(value),
                writable: writable_hint(group),
            }
        })
        .collect();
    out.sort_by(|a, b| {
        (a.group.as_str(), a.name.as_str()).cmp(&(b.group.as_str(), b.name.as_str()))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mappa(voci: &[(&str, Value)]) -> TagMap {
        voci.iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn l_iptc_viene_scritto_solo_dove_esiste() {
        let jpeg =
            changes_for(MetadataField::Title, Some("Mare"), FormatKind::Jpeg, false).unwrap();
        assert_eq!(jpeg, vec!["XMP-dc:Title=Mare", "IPTC:ObjectName=Mare"]);

        let png = changes_for(MetadataField::Title, Some("Mare"), FormatKind::Png, false).unwrap();
        assert_eq!(png, vec!["XMP-dc:Title=Mare"]);
    }

    #[test]
    fn il_copyright_usa_tutti_i_contenitori_supportati() {
        let jpeg = changes_for(
            MetadataField::Copyright,
            Some("© 2026 Mario"),
            FormatKind::Jpeg,
            false,
        )
        .unwrap();
        assert_eq!(
            jpeg,
            vec![
                "XMP-dc:Rights=© 2026 Mario",
                "EXIF:Copyright=© 2026 Mario",
                "IPTC:CopyrightNotice=© 2026 Mario",
            ]
        );

        let png = changes_for(
            MetadataField::Copyright,
            Some("© 2026 Mario"),
            FormatKind::Png,
            false,
        )
        .unwrap();
        assert_eq!(
            png,
            vec!["XMP-dc:Rights=© 2026 Mario", "EXIF:Copyright=© 2026 Mario",]
        );
    }

    #[test]
    fn il_sidecar_riceve_solo_tag_xmp() {
        let sidecar = changes_for(
            MetadataField::Creator,
            Some("Mario"),
            FormatKind::Jpeg,
            true,
        )
        .unwrap();
        assert_eq!(sidecar, vec!["XMP-dc:Creator=Mario"]);

        assert!(
            changes_for(
                MetadataField::Orientation,
                Some("6"),
                FormatKind::Jpeg,
                true
            )
            .unwrap()
            .is_empty(),
            "l'orientamento non ha equivalente XMP"
        );
        assert!(
            changes_for(MetadataField::Caption, Some("x"), FormatKind::Jpeg, true)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn i_valori_vuoti_cancellano_i_tag() {
        let cancellato =
            changes_for(MetadataField::Title, Some("  "), FormatKind::Jpeg, false).unwrap();
        assert_eq!(cancellato, vec!["XMP-dc:Title=", "IPTC:ObjectName="]);

        let none = changes_for(MetadataField::Rating, None, FormatKind::Png, false).unwrap();
        assert_eq!(none, vec!["XMP-xmp:Rating="]);
    }

    #[test]
    fn le_liste_vengono_svuotate_e_riscritte() {
        let keywords = changes_for(
            MetadataField::Keywords,
            Some("mare, tramonto ,,"),
            FormatKind::Jpeg,
            false,
        )
        .unwrap();
        assert_eq!(
            keywords,
            vec![
                "XMP-dc:Subject=",
                "XMP-dc:Subject=mare",
                "XMP-dc:Subject=tramonto",
                "IPTC:Keywords=",
                "IPTC:Keywords=mare",
                "IPTC:Keywords=tramonto",
            ]
        );
    }

    #[test]
    fn la_valutazione_viene_validata() {
        let errore =
            changes_for(MetadataField::Rating, Some("9"), FormatKind::Jpeg, false).unwrap_err();
        assert!(errore.contains("0 a 5"));
        let errore =
            changes_for(MetadataField::Rating, Some("x"), FormatKind::Jpeg, false).unwrap_err();
        assert!(errore.contains("0 a 5"));
        assert_eq!(
            changes_for(MetadataField::Rating, Some("4"), FormatKind::Jpeg, false).unwrap(),
            vec!["XMP-xmp:Rating=4"]
        );
    }

    #[test]
    fn la_data_viene_normalizzata_nel_formato_exif() {
        let data = changes_for(
            MetadataField::DateOriginal,
            Some("2024-05-01 08:30"),
            FormatKind::Jpeg,
            false,
        )
        .unwrap();
        assert_eq!(data, vec!["EXIF:DateTimeOriginal=2024:05:01 08:30:00"]);

        let data = changes_for(
            MetadataField::DateOriginal,
            Some("2024:05:01T08:30:15"),
            FormatKind::Png,
            false,
        )
        .unwrap();
        assert_eq!(data, vec!["EXIF:DateTimeOriginal=2024:05:01 08:30:15"]);

        assert!(changes_for(
            MetadataField::DateOriginal,
            Some("2024-13-01"),
            FormatKind::Png,
            false
        )
        .is_err());
    }

    #[test]
    fn il_fuso_orario_viene_validato() {
        assert_eq!(
            changes_for(
                MetadataField::OffsetTime,
                Some("+02:00"),
                FormatKind::Png,
                false
            )
            .unwrap(),
            vec!["EXIF:OffsetTimeOriginal=+02:00"]
        );
        assert!(changes_for(
            MetadataField::OffsetTime,
            Some("02:00"),
            FormatKind::Png,
            false
        )
        .is_err());
    }

    #[test]
    fn la_lettura_rispetta_le_priorita() {
        let tags = mappa(&[
            ("XMP-dc:Title", json!("Titolo XMP")),
            ("IPTC:ObjectName", json!("Titolo IPTC")),
            ("IFD0:Artist", json!("Mario")),
            ("XMP-dc:Rights", json!("© 2026 Mario")),
            ("XMP-dc:Subject", json!(["mare", "tramonto"])),
            ("XMP-xmp:Rating", json!(4)),
            ("GPS:GPSLatitude", json!(44.4949)),
            ("GPS:GPSLatitudeRef", json!("S")),
            ("GPS:GPSLongitude", json!(11.3426)),
            ("EXIF:DateTimeOriginal", json!("2024:05:01 08:30:00")),
            (
                "XMP-iptcCore:CreatorContactInfo",
                json!({"CiUrlWork": "https://portfolio.example"}),
            ),
            ("IFD0:Make", json!("FUJIFILM")),
            ("IFD0:Model", json!("X-T5")),
            ("EXIF:FNumber", json!(2.8)),
            ("EXIF:ExposureTime", json!(0.004)),
            ("EXIF:FocalLength", json!(35.0)),
            ("EXIF:ISO", json!(400)),
        ]);

        let doc = document_from_tags(&tags);
        assert_eq!(doc.value(MetadataField::Title), Some("Titolo XMP"));
        assert_eq!(doc.value(MetadataField::Creator), Some("Mario"));
        assert_eq!(doc.value(MetadataField::Copyright), Some("© 2026 Mario"));
        assert_eq!(doc.list(MetadataField::Keywords), ["mare", "tramonto"]);
        assert_eq!(doc.rating(), Some(4));
        assert_eq!(
            doc.gps,
            Some(GpsPosition {
                latitude: -44.4949,
                longitude: 11.3426
            })
        );
        assert_eq!(
            doc.value(MetadataField::PortfolioUrl),
            Some("https://portfolio.example")
        );
        assert_eq!(
            doc.value(MetadataField::DateOriginal),
            Some("2024:05:01 08:30:00")
        );
        assert_eq!(doc.technical.f_number.as_deref(), Some("f/2.8"));
        assert_eq!(doc.technical.exposure_time.as_deref(), Some("1/250 s"));
        assert_eq!(doc.technical.focal_length.as_deref(), Some("35 mm"));
        assert_eq!(doc.technical.iso, Some(400));
    }

    #[test]
    fn il_riassunto_estrae_i_campi_dell_indice() {
        let tags = mappa(&[
            ("XMP-dc:Subject", json!(["mare", "tramonto"])),
            ("XMP-xmp:Rating", json!(5)),
            ("Composite:GPSLatitude", json!(44.4949)),
            ("Composite:GPSLongitude", json!(11.3426)),
            ("EXIF:DateTimeOriginal", json!("2024:05:01 08:30:00")),
            ("IFD0:Make", json!("NIKON CORPORATION")),
            ("IFD0:Model", json!("NIKON Z 6")),
        ]);
        let summary = summary_from_tags(&tags);
        assert!(summary.has_gps);
        assert_eq!(summary.rating, Some(5));
        assert_eq!(summary.keywords.as_deref(), Some("mare, tramonto"));
        assert_eq!(summary.camera_make.as_deref(), Some("NIKON CORPORATION"));
        assert_eq!(summary.gps_lat, Some(44.4949));
    }

    #[test]
    fn i_tag_grezzi_ordinano_e_segnano_la_scrivibilita() {
        let tags = mappa(&[
            ("SourceFile", json!("foto.jpg")),
            ("XMP-dc:Title", json!("Titolo")),
            ("File:FileSize", json!("2 MB")),
            ("ExifTool:ExifToolVersion", json!(13.5)),
            ("XMP-photoshop:Credit", json!("Studio")),
            ("IFD0:Orientation", json!(6)),
        ]);
        let raw = raw_tags_from(&tags);
        let chiavi: Vec<String> = raw
            .iter()
            .map(|t| format!("{}:{}", t.group, t.name))
            .collect();
        assert_eq!(
            chiavi,
            [
                "ExifTool:ExifToolVersion",
                "File:FileSize",
                "IFD0:Orientation",
                "XMP-dc:Title",
                "XMP-photoshop:Credit"
            ]
        );
        assert!(!raw[0].writable, "ExifTool è un gruppo di sola lettura");
        assert!(!raw[1].writable);
        assert!(raw[2].writable, "IFD0 è scrivibile");
    }
}
