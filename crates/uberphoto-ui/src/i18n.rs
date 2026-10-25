//! Localizzazione dell'interfaccia.
//!
//! Le stringhe sorgente restano in italiano nel codice e fungono da chiave di
//! traduzione. `tr` restituisce la variante nella lingua attiva, con fallback
//! all'italiano per le voci non ancora tradotte.
//!
//! La tabella `TRANSLATIONS` è ordinata per chiave italiana; per i messaggi
//! dinamici le traduzioni usano segnaposto `{nome}` riempiti da `tr_args`.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

mod data {
    include!("i18n_data.rs");
}

#[cfg(test)]
use data::TRANSLATED_LANGUAGES;
pub use data::TRANSLATIONS;

/// Lingue supportate dall'interfaccia, nell'ordine mostrato nel selettore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    It,
    En,
    De,
    Fr,
    Es,
    Pl,
    Tr,
    Ru,
    Sq,
}

impl Language {
    pub const ALL: [Language; 9] = [
        Language::It,
        Language::En,
        Language::De,
        Language::Fr,
        Language::Es,
        Language::Pl,
        Language::Tr,
        Language::Ru,
        Language::Sq,
    ];

    pub const fn index(self) -> usize {
        match self {
            Language::It => 0,
            Language::En => 1,
            Language::De => 2,
            Language::Fr => 3,
            Language::Es => 4,
            Language::Pl => 5,
            Language::Tr => 6,
            Language::Ru => 7,
            Language::Sq => 8,
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Language::It => "it",
            Language::En => "en",
            Language::De => "de",
            Language::Fr => "fr",
            Language::Es => "es",
            Language::Pl => "pl",
            Language::Tr => "tr",
            Language::Ru => "ru",
            Language::Sq => "sq",
        }
    }

    /// Nome della lingua nella lingua stessa.
    pub const fn native_name(self) -> &'static str {
        match self {
            Language::It => "Italiano",
            Language::En => "English",
            Language::De => "Deutsch",
            Language::Fr => "Français",
            Language::Es => "Español",
            Language::Pl => "Polski",
            Language::Tr => "Türkçe",
            Language::Ru => "Русский",
            Language::Sq => "Shqip",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        let code = code.trim().to_ascii_lowercase();
        let code = code.split(['_', '-', '.']).next().unwrap_or(code.as_str());
        Language::ALL.into_iter().find(|lang| lang.code() == code)
    }

    /// Lingua di sistema, con inglese come ripiego.
    pub fn detect() -> Self {
        let mut values = Vec::new();
        for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(value) = std::env::var(key) {
                if !value.trim().is_empty() {
                    values.push(value);
                }
            }
        }
        Language::da_valori_locale(values).unwrap_or(Language::En)
    }

    fn parse_locale_values(values: impl IntoIterator<Item = String>) -> Option<Self> {
        values
            .into_iter()
            .filter_map(|value| Language::from_code(&value))
            .next()
    }

    fn da_valori_locale(values: impl IntoIterator<Item = String>) -> Option<Self> {
        Language::parse_locale_values(values).or_else(|| {
            sys_locale::get_locale()
                .as_deref()
                .and_then(Language::from_code)
        })
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(Language::It as u8);

pub fn set_current(language: Language) {
    CURRENT.store(language as u8, Ordering::Relaxed);
}

pub fn current() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        0 => Language::It,
        1 => Language::En,
        2 => Language::De,
        3 => Language::Fr,
        4 => Language::Es,
        5 => Language::Pl,
        6 => Language::Tr,
        7 => Language::Ru,
        _ => Language::Sq,
    }
}

/// Traduce una stringa italiana. Se la voce non esiste, resta in italiano.
pub fn tr(text: &'static str) -> &'static str {
    let language = current();
    if language == Language::It {
        return text;
    }
    let index = language.index();
    // La tabella è generata ordinata: la ricerca binaria evita una mappa a runtime.
    match TRANSLATIONS.binary_search_by_key(&text, |(key, _)| *key) {
        Ok(position) => {
            let variants = &TRANSLATIONS[position].1;
            let translated = variants[index];
            if translated.is_empty() {
                text
            } else {
                translated
            }
        }
        Err(_) => text,
    }
}

/// Variante italiana dal catalogo, usata per le forme plurali.
///
/// Le chiavi con `{count}` possono contenere più forme separate da `|`
/// (singolare|plurale, oppure una|pochi|molti). Per l'italiano `tr` restituisce
/// la chiave, quindi `tr_args` deve leggere la variante dal catalogo.
fn variante_italiana(text: &'static str) -> &'static str {
    match TRANSLATIONS.binary_search_by_key(&text, |(key, _)| *key) {
        Ok(position) => TRANSLATIONS[position].1[0],
        Err(_) => text,
    }
}

/// Sceglie la forma plurale in base alla lingua e al conteggio.
///
/// Le forme sono separate da `|`: due forme per la maggior parte delle lingue
/// (una|molte), tre per russo e polacco (una|poche|molte). Le lingue senza
/// flessione dopo il numero usano una sola forma e non passano di qui.
fn forma_plurale(testo: &str, language: Language, count: usize) -> Option<&str> {
    let forme: Vec<&str> = testo.split('|').collect();
    if forme.len() < 2 {
        return None;
    }
    let indice = match language {
        Language::Ru if forme.len() >= 3 => {
            if count % 10 == 1 && count % 100 != 11 {
                0
            } else if (2..=4).contains(&(count % 10)) && !(12..=14).contains(&(count % 100)) {
                1
            } else {
                2
            }
        }
        Language::Pl if forme.len() >= 3 => {
            if count == 1 {
                0
            } else if (2..=4).contains(&(count % 10)) && !(12..=14).contains(&(count % 100)) {
                1
            } else {
                2
            }
        }
        Language::Fr => {
            if count <= 1 {
                0
            } else {
                1
            }
        }
        _ => {
            if count == 1 {
                0
            } else {
                1
            }
        }
    };
    forme.get(indice).copied()
}

/// Traduce e sostituisce i segnaposto `{nome}` con i valori indicati.
pub fn tr_args(text: &'static str, args: &[(&str, String)]) -> String {
    let language = current();
    let mut translated = if language == Language::It {
        variante_italiana(text).to_owned()
    } else {
        tr(text).to_owned()
    };
    if let Some((_, value)) = args.iter().find(|(name, _)| *name == "count") {
        if let Ok(count) = value.parse::<usize>() {
            let forma = forma_plurale(&translated, language, count).map(str::to_owned);
            if let Some(forma) = forma {
                translated = forma;
            }
        }
    }
    for (name, value) in args {
        translated = translated.replace(&format!("{{{name}}}"), value);
    }
    translated
}

/// Statistiche usate dai test di completezza del catalogo.
#[cfg(test)]
pub fn missing_translations(language: Language) -> Vec<&'static str> {
    if language == Language::It {
        return Vec::new();
    }
    let index = language.index();
    TRANSLATIONS
        .iter()
        .filter(|(_, variants)| variants[index].is_empty())
        .map(|(key, _)| *key)
        .collect()
}

/// Numero di stringhe tradotte per la lingua indicata.
#[cfg(test)]
pub fn translated_count(language: Language) -> usize {
    if language == Language::It {
        return TRANSLATIONS.len();
    }
    let index = language.index();
    TRANSLATIONS
        .iter()
        .filter(|(_, variants)| !variants[index].is_empty())
        .count()
}

#[macro_export]
macro_rules! t {
    ($text:literal) => {
        $crate::i18n::tr($text)
    };
    ($text:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::tr_args(
            $text,
            &[$((stringify!($name), ($value).to_string())),+],
        )
    };
}

/// Etichette localizzate per i modelli che espongono testi italiani.
pub fn sort_key_label(key: uberphoto_files::SortKey) -> &'static str {
    use uberphoto_files::SortKey;
    tr(match key {
        SortKey::Name => "Nome",
        SortKey::Modified => "Data modifica",
        SortKey::Size => "Dimensione",
        SortKey::AcquisitionDate => "Data di acquisizione",
        SortKey::Rating => "Valutazione",
    })
}

pub fn metadata_field_label(field: uberphoto_metadata::MetadataField) -> &'static str {
    use uberphoto_metadata::MetadataField as F;
    tr(match field {
        F::Title => "Titolo",
        F::Headline => "Headline",
        F::Description => "Descrizione",
        F::Caption => "Didascalia",
        F::AltText => "Testo alternativo",
        F::Keywords => "Parole chiave",
        F::Category => "Categoria",
        F::Event => "Evento",
        F::People => "Persone raffigurate",
        F::Rating => "Valutazione",
        F::Label => "Etichetta colore",
        F::Notes => "Note",
        F::Identifier => "Identificativo",
        F::Creator => "Autore",
        F::Copyright => "Copyright",
        F::Credit => "Credit line",
        F::Source => "Fonte",
        F::Publisher => "Editore",
        F::License => "Licenza",
        F::UsageTerms => "Termini d'uso",
        F::LicenseUrl => "URL licenza",
        F::PortfolioUrl => "URL portfolio / sito",
        F::Email => "Email di contatto",
        F::Instructions => "Istruzioni d'uso",
        F::DateOriginal => "Data di scatto",
        F::OffsetTime => "Fuso orario",
        F::Orientation => "Orientamento",
    })
}

pub fn metadata_field_hint(field: uberphoto_metadata::MetadataField) -> &'static str {
    use uberphoto_metadata::MetadataField as F;
    tr(match field {
        F::Keywords => "valori separati da virgole",
        F::Rating => "0–5 stelle",
        F::DateOriginal => "AAAA:MM:GG hh:mm:ss",
        F::OffsetTime => "+HH:MM",
        F::Orientation => "1–8 (EXIF)",
        F::People => "nomi separati da virgole",
        _ => "",
    })
}

pub fn metadata_section_label(section: uberphoto_metadata::Section) -> &'static str {
    use uberphoto_metadata::Section;
    tr(match section {
        Section::Descriptive => "Descrittive",
        Section::Author => "Autore e copyright",
        Section::Temporal => "Data e orientamento",
    })
}

pub fn location_role_label(role: uberphoto_metadata::LocationRole) -> &'static str {
    use uberphoto_metadata::LocationRole;
    tr(match role {
        LocationRole::Created => "Località di scatto",
        LocationRole::Shown => "Località raffigurata",
    })
}

pub fn location_role_hint(role: uberphoto_metadata::LocationRole) -> &'static str {
    use uberphoto_metadata::LocationRole;
    tr(match role {
        LocationRole::Created => "dove è stata scattata la fotografia",
        LocationRole::Shown => "cosa raffigura l'immagine",
    })
}

pub fn location_field_label(field: uberphoto_metadata::LocationField) -> &'static str {
    use uberphoto_metadata::LocationField as F;
    tr(match field {
        F::Place => "Nome luogo",
        F::Sublocation => "Sublocation",
        F::City => "Città",
        F::State => "Regione / provincia",
        F::Country => "Paese",
        F::CountryCode => "Codice ISO",
    })
}

pub fn location_field_hint(field: uberphoto_metadata::LocationField) -> &'static str {
    use uberphoto_metadata::LocationField as F;
    tr(match field {
        F::CountryCode => "due lettere, es. IT",
        F::Place => "nome del luogo",
        _ => "",
    })
}

pub fn edit_field_label(field: uberphoto_image::EditField) -> &'static str {
    use uberphoto_image::EditField as F;
    tr(match field {
        F::Exposure => "Esposizione",
        F::Brightness => "Luminosità",
        F::Contrast => "Contrasto",
        F::Saturation => "Saturazione",
        F::Vibrance => "Vibrance",
        F::Temperature => "Temperatura",
        F::Tint => "Tinta",
        F::Highlights => "Alte luci",
        F::Shadows => "Ombre",
        F::Whites => "Bianchi",
        F::Blacks => "Neri",
        F::Gamma => "Gamma",
        F::Sharpness => "Nitidezza",
        F::Rotation => "Rotazione",
        F::Crop => "Ritaglio",
    })
}

pub fn export_preset_label(preset: uberphoto_export::ExportPreset) -> &'static str {
    use uberphoto_export::ExportPreset;
    tr(match preset {
        ExportPreset::FullMetadata => "Full Metadata",
        ExportPreset::WebOptimized => "Web Optimized",
        ExportPreset::Privacy => "Privacy",
    })
}

pub fn export_preset_hint(preset: uberphoto_export::ExportPreset) -> &'static str {
    use uberphoto_export::ExportPreset;
    tr(match preset {
        ExportPreset::FullMetadata => {
            "Conserva tutti i metadati compatibili con il formato scelto."
        }
        ExportPreset::WebOptimized => {
            "Conserva titolo, descrizione, keywords, autore, copyright e valutazione."
        }
        ExportPreset::Privacy => "Rimuove GPS e i dati sensibili selezionati.",
    })
}

pub fn export_format_label(format: uberphoto_export::ExportFormat) -> &'static str {
    use uberphoto_export::ExportFormat;
    tr(match format {
        ExportFormat::Jpeg => "JPEG",
        ExportFormat::Png => "PNG",
        ExportFormat::Webp => "WebP (senza perdita)",
        ExportFormat::Tiff => "TIFF",
    })
}

pub fn name_mode_label(mode: uberphoto_export::NameMode) -> &'static str {
    use uberphoto_export::NameMode;
    tr(match mode {
        NameMode::KeepName => "Mantieni nome",
        NameMode::Prefix => "Prefisso",
        NameMode::Suffix => "Suffisso",
        NameMode::WebSafe => "Nome web-safe",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    #[test]
    fn codici_lingua_riconosciuti() {
        for language in Language::ALL {
            assert_eq!(Language::from_code(language.code()), Some(language));
        }
        assert_eq!(Language::from_code("de_DE.UTF-8"), Some(Language::De));
        assert_eq!(Language::from_code("sq_AL"), Some(Language::Sq));
        assert_eq!(Language::from_code("xx"), None);
    }

    #[test]
    fn valori_locale_senza_estensione_sono_riconosciuti() {
        assert_eq!(
            Language::parse_locale_values(["it_IT.UTF-8".to_owned(), "en".to_owned()]),
            Some(Language::It)
        );
        assert_eq!(
            Language::parse_locale_values(["C.UTF-8".to_owned(), "de".to_owned()]),
            Some(Language::De)
        );
        assert_eq!(Language::parse_locale_values(Vec::<String>::new()), None);
        assert_eq!(Language::from_code("it_IT.UTF-8"), Some(Language::It));
        assert_eq!(Language::from_code("xx"), None);
    }

    #[test]
    fn tabella_ordinata_e_senza_duplicati() {
        assert!(
            TRANSLATIONS.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "le chiavi di traduzione devono essere ordinate e univoche"
        );
    }

    #[test]
    fn tutte_le_lingue_sono_tradotte() {
        for language in TRANSLATED_LANGUAGES {
            let missing = missing_translations(*language);
            assert!(
                missing.is_empty(),
                "traduzioni mancanti per {}: {:?}",
                language.code(),
                missing
            );
            assert_eq!(translated_count(*language), TRANSLATIONS.len());
        }
    }

    #[test]
    fn il_macro_torna_i_tipi_attesi() {
        let statico: &'static str = t!("Salva");
        let dinamico: String = t!("Applica a {count} foto", count = 1);
        let _ = statico;
        assert!(dinamico.contains('1'), "{dinamico}");
    }

    #[test]
    fn i_segnaposto_sono_preservati() {
        for (chiave, varianti) in TRANSLATIONS {
            let attesi = segnaposto(chiave);
            for (indice, variante) in varianti.iter().enumerate() {
                for forma in variante.split('|') {
                    assert_eq!(
                        segnaposto(forma),
                        attesi,
                        "segnaposto persi nella variante {indice} di {chiave:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn le_varianti_italiane_sono_coerenti() {
        for (chiave, varianti) in TRANSLATIONS {
            assert!(
                varianti[0] == *chiave || varianti[0].contains('|'),
                "la variante italiana di {chiave:?} deve coincidere con la chiave o contenere forme plurali"
            );
        }
    }

    #[test]
    fn solo_le_chiavi_con_conteggio_hanno_forme_plurali() {
        for (chiave, varianti) in TRANSLATIONS {
            if varianti.iter().any(|variante| variante.contains('|')) {
                assert!(
                    chiave.contains("{count}"),
                    "forme plurali inattese nella chiave {chiave:?}"
                );
            }
        }
    }

    #[test]
    fn le_forme_plurali_seguono_le_regole_delle_lingue() {
        let russo = "файл|файла|файлов";
        assert_eq!(forma_plurale(russo, Language::Ru, 1), Some("файл"));
        assert_eq!(forma_plurale(russo, Language::Ru, 2), Some("файла"));
        assert_eq!(forma_plurale(russo, Language::Ru, 5), Some("файлов"));
        assert_eq!(forma_plurale(russo, Language::Ru, 11), Some("файлов"));
        assert_eq!(forma_plurale(russo, Language::Ru, 21), Some("файл"));
        assert_eq!(forma_plurale(russo, Language::Ru, 22), Some("файла"));

        let polacco = "plik|pliki|plików";
        assert_eq!(forma_plurale(polacco, Language::Pl, 1), Some("plik"));
        assert_eq!(forma_plurale(polacco, Language::Pl, 2), Some("pliki"));
        assert_eq!(forma_plurale(polacco, Language::Pl, 5), Some("plików"));
        assert_eq!(forma_plurale(polacco, Language::Pl, 12), Some("plików"));
        assert_eq!(forma_plurale(polacco, Language::Pl, 22), Some("pliki"));

        let inglese = "photo|photos";
        assert_eq!(forma_plurale(inglese, Language::En, 1), Some("photo"));
        assert_eq!(forma_plurale(inglese, Language::En, 2), Some("photos"));

        let francese = "photo|photos";
        assert_eq!(forma_plurale(francese, Language::Fr, 0), Some("photo"));
        assert_eq!(forma_plurale(francese, Language::Fr, 1), Some("photo"));
        assert_eq!(forma_plurale(francese, Language::Fr, 2), Some("photos"));

        assert_eq!(forma_plurale("foto", Language::It, 3), None);
    }

    #[test]
    fn le_chiavi_con_conteggio_hanno_la_forma_italiana() {
        let variante = variante_italiana("{count} foto selezionate");
        assert_eq!(
            forma_plurale(variante, Language::It, 1),
            Some("{count} foto selezionata")
        );
        assert_eq!(
            forma_plurale(variante, Language::It, 4),
            Some("{count} foto selezionate")
        );
    }

    #[test]
    fn i_font_predefiniti_coprono_le_traduzioni() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |_| {});
        output.textures_delta.clear();
        let mut mancanti = Vec::new();
        ctx.fonts_mut(|fonts| {
            let font = egui::FontId::proportional(14.0);
            let mut controlla = |testo: &str| {
                for carattere in testo.chars().filter(|carattere| !carattere.is_whitespace()) {
                    if !fonts.has_glyph(&font, carattere) {
                        mancanti.push(carattere);
                    }
                }
            };
            for language in Language::ALL {
                controlla(language.native_name());
            }
            for (_, varianti) in TRANSLATIONS {
                for variante in *varianti {
                    for forma in variante.split('|') {
                        controlla(forma);
                    }
                }
            }
        });
        mancanti.sort_unstable();
        mancanti.dedup();
        assert!(
            mancanti.is_empty(),
            "caratteri senza glifo nei font predefiniti: {:?}",
            mancanti
        );
    }

    #[test]
    fn ogni_uso_del_macro_ha_una_traduzione() {
        let mut usate = BTreeSet::new();
        for (_, sorgente) in sorgenti_crate() {
            for literal in invocazioni_literal_t(&sorgente) {
                usate.insert(literal);
            }
        }
        let catalogo: BTreeSet<&str> = TRANSLATIONS.iter().map(|(chiave, _)| *chiave).collect();
        let usate: BTreeSet<&str> = usate.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            usate, catalogo,
            "ogni uso di t! deve avere la sua chiave nel catalogo"
        );
    }

    fn sorgenti_crate() -> Vec<(std::path::PathBuf, String)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).expect("root src") {
            let path = entry.expect("src entry").path();
            if path.extension().and_then(|estensione| estensione.to_str()) == Some("rs") {
                let testo = std::fs::read_to_string(&path)
                    .unwrap_or_else(|errore| panic!("lettura {}: {errore}", path.display()));
                out.push((path, testo));
            }
        }
        out
    }

    fn invocazioni_literal_t(sorgente: &str) -> Vec<String> {
        let caratteri: Vec<char> = sorgente.chars().collect();
        let mut trovati = Vec::new();
        let mut indice = 0;
        while indice < caratteri.len() {
            let sequenza = caratteri[indice] == 't'
                && caratteri.get(indice + 1) == Some(&'!')
                && caratteri.get(indice + 2) == Some(&'(');
            if sequenza {
                let precedente_ok = indice == 0
                    || !(caratteri[indice - 1].is_ascii_alphanumeric()
                        || caratteri[indice - 1] == '_');
                if precedente_ok {
                    if let Some((literal, fine)) = leggi_stringa(&caratteri, indice + 3) {
                        trovati.push(literal);
                        indice = fine;
                        continue;
                    }
                }
            }
            indice += 1;
        }
        trovati
    }

    fn leggi_stringa(caratteri: &[char], inizio: usize) -> Option<(String, usize)> {
        let mut i = inizio;
        while i < caratteri.len() && caratteri[i].is_whitespace() {
            i += 1;
        }
        if caratteri.get(i) != Some(&'"') {
            return None;
        }
        i += 1;
        let mut valore = String::new();
        while i < caratteri.len() {
            let carattere = caratteri[i];
            i += 1;
            match carattere {
                '"' => return Some((valore, i)),
                '\\' => {
                    let escape = caratteri.get(i).copied().unwrap_or('\\');
                    i += 1;
                    match escape {
                        'n' => valore.push('\n'),
                        'r' => valore.push('\r'),
                        't' => valore.push('\t'),
                        '\\' => valore.push('\\'),
                        '"' => valore.push('"'),
                        '\'' => valore.push('\''),
                        '0' => valore.push('\0'),
                        'x' => {
                            let cifre: String = caratteri.iter().skip(i).take(2).collect();
                            i += 2;
                            if let Ok(codice) = u8::from_str_radix(&cifre, 16) {
                                valore.push(codice as char);
                            } else {
                                return None;
                            }
                        }
                        'u' => {
                            if caratteri.get(i) != Some(&'{') {
                                return None;
                            }
                            i += 1;
                            let mut esadecimale = String::new();
                            while let Some(&c) = caratteri.get(i) {
                                if c == '}' {
                                    break;
                                }
                                esadecimale.push(c);
                                i += 1;
                            }
                            if caratteri.get(i) != Some(&'}') {
                                return None;
                            }
                            i += 1;
                            let codice = u32::from_str_radix(&esadecimale, 16).ok()?;
                            valore.push(char::from_u32(codice)?);
                        }
                        _ => valore.push(escape),
                    }
                }
                _ => valore.push(carattere),
            }
        }
        None
    }

    fn segnaposto(testo: &str) -> BTreeSet<String> {
        let caratteri: Vec<char> = testo.chars().collect();
        let mut out = BTreeSet::new();
        let mut indice = 0;
        while indice < caratteri.len() {
            if caratteri[indice] == '{' {
                let mut fine = indice;
                while fine < caratteri.len() && caratteri[fine] != '}' {
                    fine += 1;
                }
                if fine < caratteri.len() {
                    let nome: String = caratteri[indice + 1..fine].iter().collect();
                    if !nome.is_empty()
                        && nome.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        out.insert(nome);
                        indice = fine + 1;
                        continue;
                    }
                }
            }
            indice += 1;
        }
        out
    }
}
