use std::cmp::Ordering;
use std::iter::Peekable;
use std::str::Chars;

/// Nomi che iniziano con `.` (convenzione Unix, rispettata anche da macOS;
/// su Windows i file nascosti usano un attributo, non gestito in Fase 1).
pub fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.')
}

/// Confronto "naturale", case-insensitive: le sequenze di cifre sono
/// confrontate come numeri, così `IMG_2.jpg` precede `IMG_10.jpg`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut left = a.chars().peekable();
    let mut right = b.chars().peekable();

    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                if x.is_ascii_digit() && y.is_ascii_digit() {
                    let num_left = take_digits(&mut left);
                    let num_right = take_digits(&mut right);
                    match num_left.cmp(&num_right) {
                        Ordering::Equal => {}
                        order => return order,
                    }
                } else {
                    left.next();
                    right.next();
                    match x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase()) {
                        Ordering::Equal => {}
                        order => return order,
                    }
                }
            }
        }
    }
}

/// Consuma una sequenza di cifre restituendo `(valore, numero di cifre)`.
/// Il confronto a coppie dà un tie-break deterministico per `01` vs `1`.
fn take_digits(chars: &mut Peekable<Chars<'_>>) -> (u128, usize) {
    let mut value: u128 = 0;
    let mut digits = 0usize;
    while let Some(c) = chars.peek().copied() {
        match c.to_digit(10) {
            Some(digit) => {
                value = value.saturating_mul(10).saturating_add(digit as u128);
                digits += 1;
                chars.next();
            }
            None => break,
        }
    }
    (value, digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordina_le_cifre_come_numeri() {
        assert_eq!(natural_cmp("IMG_2.jpg", "IMG_10.jpg"), Ordering::Less);
        assert_eq!(natural_cmp("IMG_10.jpg", "IMG_2.jpg"), Ordering::Greater);
        assert_eq!(natural_cmp("IMG_2.jpg", "IMG_2.jpg"), Ordering::Equal);
    }

    #[test]
    fn confronto_case_insensitive() {
        assert_eq!(natural_cmp("a.jpg", "B.jpg"), Ordering::Less);
        assert_eq!(natural_cmp("A.jpg", "a.jpg"), Ordering::Equal);
    }

    #[test]
    fn zeri_iniziali_deterministici() {
        assert_eq!(natural_cmp("img_1.jpg", "img_01.jpg"), Ordering::Less);
        assert_eq!(natural_cmp("img_01.jpg", "img_1.jpg"), Ordering::Greater);
    }

    #[test]
    fn prefissi_piu_corti_prima() {
        assert_eq!(natural_cmp("foto", "foto1"), Ordering::Less);
        assert_eq!(natural_cmp("foto2", "foto"), Ordering::Greater);
    }
}
