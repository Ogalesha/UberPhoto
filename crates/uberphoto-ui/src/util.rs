use std::time::SystemTime;

/// Formattazione leggibile di una dimensione in byte.
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let value = bytes as f64;
    if value >= GB {
        format!("{:.2} GB", value / GB)
    } else if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.0} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Data/ora locale da un `SystemTime`.
pub fn format_time(time: SystemTime) -> String {
    let datetime: chrono::DateTime<chrono::Local> = time.into();
    datetime.format("%d/%m/%Y %H:%M:%S").to_string()
}

/// Tronca `text` a `max_chars` caratteri aggiungendo un'ellissi.
pub fn elide(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = text.chars().take(keep).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatta_le_dimensione_in_unita_leggibili() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.00 GB");
    }

    #[test]
    fn elide_solo_quando_serve() {
        assert_eq!(elide("corto", 10), "corto");
        assert_eq!(elide("questo è molto lungo", 10), "questo è …");
        assert_eq!(elide("àèìòù", 5), "àèìòù");
    }
}
