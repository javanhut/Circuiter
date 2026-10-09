//! Engineering-notation quantities: parsing `4.7k`, `100nF`, `4k7`, `1meg`, `2.2µH` and
//! formatting values back to the same style.
//!
//! Circuiter uses SI prefixes, so `m` is milli and `M` is mega. `meg` is also accepted
//! for mega, since SPICE users type it from habit. (SPICE itself treats `M` as milli;
//! the netlister handles that conversion.)

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuantityError {
    pub input: String,
    pub reason: &'static str,
}

impl fmt::Display for QuantityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid quantity `{}`: {}", self.input, self.reason)
    }
}

impl std::error::Error for QuantityError {}

const PREFIXES: &[(&str, f64)] = &[
    ("meg", 1e6),
    ("Meg", 1e6),
    ("MEG", 1e6),
    ("f", 1e-15),
    ("p", 1e-12),
    ("n", 1e-9),
    ("u", 1e-6),
    ("µ", 1e-6), // U+00B5 micro sign
    ("μ", 1e-6), // U+03BC greek mu
    ("m", 1e-3),
    ("k", 1e3),
    ("K", 1e3),
    ("M", 1e6),
    ("G", 1e9),
    ("T", 1e12),
];

fn prefix_scale(s: &str) -> Option<(f64, usize)> {
    PREFIXES.iter().find(|(p, _)| s.starts_with(p)).map(|(p, scale)| (*scale, p.len()))
}

/// Parses an engineering-notation quantity.
///
/// `unit` is the parameter's unit symbol (e.g. `"F"`, `"Ω"`). When given, a trailing unit
/// symbol is accepted and stripped, so `100nF` and `100n` are equivalent. For ohms,
/// RKM notation with `R` as the decimal point (`4R7`) is also accepted.
pub fn parse_quantity(input: &str, unit: Option<&str>) -> Result<f64, QuantityError> {
    let err = |reason| QuantityError { input: input.to_owned(), reason };
    let mut s = input.trim();
    if s.is_empty() {
        return Err(err("empty value"));
    }

    let is_ohms = unit == Some("Ω");
    if let Some(u) = unit {
        let aliases: &[&str] = if is_ohms { &["Ω", "ohms", "ohm", "Ohm"] } else { &[u] };
        for alias in aliases {
            if let Some(stripped) = s.strip_suffix(alias) {
                s = stripped.trim_end();
                break;
            }
        }
    }

    let num_end = scan_number(s);
    if num_end == 0 {
        return Err(err("expected a number"));
    }
    let (num, rest) = s.split_at(num_end);
    let rest = rest.trim_start(); // allow `10 k`
    let mantissa: f64 = num.parse().map_err(|_| err("malformed number"))?;
    if rest.is_empty() {
        return Ok(mantissa);
    }

    let plain_integer = num.bytes().all(|b| b.is_ascii_digit());

    // RKM: `4R7` means 4.7 Ω.
    if is_ohms && plain_integer {
        if let Some(frac) = rest.strip_prefix('R') {
            return rkm(num, frac, 1.0).ok_or_else(|| err("malformed RKM value"));
        }
    }

    let (scale, len) = prefix_scale(rest).ok_or_else(|| err("unknown suffix"))?;
    let after = &rest[len..];
    if after.is_empty() {
        return Ok(mantissa * scale);
    }
    // RKM with a prefix: `4k7` means 4.7k.
    if plain_integer {
        if let Some(v) = rkm(num, after, scale) {
            return Ok(v);
        }
    }
    Err(err("unexpected text after the prefix"))
}

fn rkm(int_part: &str, frac: &str, scale: f64) -> Option<f64> {
    if frac.is_empty() || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    format!("{int_part}.{frac}").parse::<f64>().ok().map(|v| v * scale)
}

/// Returns the byte length of the leading decimal number (with optional sign, fraction and
/// exponent). An `e` is only treated as an exponent when digits follow it.
fn scan_number(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let digits_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i == digits_start || (i == digits_start + 1 && b[digits_start] == b'.') {
        return 0;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    i
}

/// Formats a value with an SI prefix and up to four significant digits, e.g. `4.7kΩ`.
///
/// Dimensionless values and compound units that read badly with a prefix (`1/V`, `√V`)
/// are written as plain numbers with a spaced unit instead: `0.04 1/V`, `0.5`.
pub fn format_quantity(value: f64, unit: &str) -> String {
    if value == 0.0 || !value.is_finite() {
        return format!("{value}{unit}");
    }
    if unit.is_empty() || unit.starts_with("1/") || unit.contains('√') {
        let text = significant(value, 4);
        return if unit.is_empty() { text } else { format!("{text} {unit}") };
    }
    const DISPLAY: &[(f64, &str)] = &[
        (1e12, "T"),
        (1e9, "G"),
        (1e6, "M"),
        (1e3, "k"),
        (1.0, ""),
        (1e-3, "m"),
        (1e-6, "µ"),
        (1e-9, "n"),
        (1e-12, "p"),
        (1e-15, "f"),
    ];
    let mag = value.abs();
    let (scale, prefix) = DISPLAY
        .iter()
        .copied()
        .find(|(scale, _)| mag >= *scale * (1.0 - 1e-9))
        .unwrap_or((1e-15, "f"));
    let mantissa = value / scale;
    let digits_before_point = (mantissa.abs().log10().floor() as i32 + 1).max(1);
    let decimals = (4 - digits_before_point).max(0) as usize;
    let mut text = format!("{mantissa:.decimals$}");
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_owned();
    }
    format!("{text}{prefix}{unit}")
}

/// Formats `value` with up to `digits` significant digits, without exponent notation for
/// everyday magnitudes.
fn significant(value: f64, digits: i32) -> String {
    let magnitude = value.abs().log10().floor() as i32;
    let decimals = (digits - 1 - magnitude).max(0) as usize;
    let mut text = format!("{value:.decimals$}");
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_owned();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-12 * b.abs().max(1e-30)
    }

    #[test]
    fn parses_plain_and_prefixed_numbers() {
        assert_eq!(parse_quantity("42", None).unwrap(), 42.0);
        assert!(approx(parse_quantity("4.7k", None).unwrap(), 4700.0));
        assert!(approx(parse_quantity("100n", None).unwrap(), 100e-9));
        assert!(approx(parse_quantity("-3.3m", None).unwrap(), -3.3e-3));
        assert!(approx(parse_quantity("1e-3", None).unwrap(), 1e-3));
        assert!(approx(parse_quantity("2.2µ", None).unwrap(), 2.2e-6));
    }

    #[test]
    fn m_is_milli_and_capital_m_or_meg_is_mega() {
        assert!(approx(parse_quantity("1m", None).unwrap(), 1e-3));
        assert!(approx(parse_quantity("1M", None).unwrap(), 1e6));
        assert!(approx(parse_quantity("1meg", None).unwrap(), 1e6));
    }

    #[test]
    fn strips_unit_symbols() {
        assert!(approx(parse_quantity("100nF", Some("F")).unwrap(), 100e-9));
        assert!(approx(parse_quantity("1F", Some("F")).unwrap(), 1.0));
        assert!(approx(parse_quantity("1f", Some("F")).unwrap(), 1e-15));
        assert!(approx(parse_quantity("10 kΩ", Some("Ω")).unwrap(), 10e3));
        assert!(approx(parse_quantity("10kohm", Some("Ω")).unwrap(), 10e3));
    }

    #[test]
    fn parses_rkm_notation() {
        assert!(approx(parse_quantity("4k7", None).unwrap(), 4700.0));
        assert!(approx(parse_quantity("2u2", Some("F")).unwrap(), 2.2e-6));
        assert!(approx(parse_quantity("4R7", Some("Ω")).unwrap(), 4.7));
    }

    #[test]
    fn rejects_garbage() {
        for bad in ["", "k", "abc", "4.7x", "1e", "4.7k7", "4R7"] {
            assert!(parse_quantity(bad, None).is_err(), "{bad} should not parse");
        }
    }

    #[test]
    fn formats_with_si_prefix() {
        assert_eq!(format_quantity(4700.0, "Ω"), "4.7kΩ");
        assert_eq!(format_quantity(100e-9, "F"), "100nF");
        assert_eq!(format_quantity(-3.3e-3, "V"), "-3.3mV");
        assert_eq!(format_quantity(1.0, "A"), "1A");
        assert_eq!(format_quantity(0.0, "V"), "0V");
        assert_eq!(format_quantity(1e-3, "s"), "1ms");
    }

    #[test]
    fn formats_dimensionless_and_compound_units_without_prefixes() {
        assert_eq!(format_quantity(0.5, ""), "0.5");
        assert_eq!(format_quantity(1.752, ""), "1.752");
        assert_eq!(format_quantity(0.04, "1/V"), "0.04 1/V");
        assert_eq!(format_quantity(0.4, "√V"), "0.4 √V");
        assert_eq!(format_quantity(1234.6, ""), "1235");
    }
}
