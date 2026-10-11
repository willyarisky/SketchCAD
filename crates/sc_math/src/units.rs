//! Length units, formatting and Measurements-box (VCB) parsing.
//!
//! Internally every length is in inches.

use serde::{Deserialize, Serialize};

pub const MM_PER_INCH: f64 = 25.4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LengthUnit {
    /// Feet and inches with fractional inches, e.g. `5' 6 1/4"`.
    Architectural,
    Inches,
    Feet,
    Millimeters,
    Centimeters,
    Meters,
}

impl LengthUnit {
    pub const ALL: [LengthUnit; 6] = [
        LengthUnit::Architectural,
        LengthUnit::Inches,
        LengthUnit::Feet,
        LengthUnit::Millimeters,
        LengthUnit::Centimeters,
        LengthUnit::Meters,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LengthUnit::Architectural => "Architectural (ft/in)",
            LengthUnit::Inches => "Inches",
            LengthUnit::Feet => "Feet",
            LengthUnit::Millimeters => "Millimeters",
            LengthUnit::Centimeters => "Centimeters",
            LengthUnit::Meters => "Meters",
        }
    }

    /// Inches per one of this unit (Architectural counts as inches for bare numbers).
    pub fn inches_per_unit(self) -> f64 {
        match self {
            LengthUnit::Architectural | LengthUnit::Inches => 1.0,
            LengthUnit::Feet => 12.0,
            LengthUnit::Millimeters => 1.0 / MM_PER_INCH,
            LengthUnit::Centimeters => 10.0 / MM_PER_INCH,
            LengthUnit::Meters => 1000.0 / MM_PER_INCH,
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            LengthUnit::Architectural | LengthUnit::Inches => "\"",
            LengthUnit::Feet => "'",
            LengthUnit::Millimeters => "mm",
            LengthUnit::Centimeters => "cm",
            LengthUnit::Meters => "m",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct UnitSettings {
    pub unit: LengthUnit,
    /// Decimal places for decimal units.
    pub precision: u8,
    /// Fraction denominator for architectural units (e.g. 16 for 1/16").
    pub fraction: u32,
}

impl Default for UnitSettings {
    fn default() -> Self {
        Self { unit: LengthUnit::Millimeters, precision: 0, fraction: 16 }
    }
}

impl UnitSettings {
    pub fn format(&self, inches: f64) -> String {
        format_length(inches, self)
    }

    pub fn parse(&self, s: &str) -> Option<f64> {
        parse_length(s, self.unit)
    }
}

pub fn format_length(inches: f64, u: &UnitSettings) -> String {
    let neg = inches < 0.0;
    let v = inches.abs();
    let body = match u.unit {
        LengthUnit::Architectural => {
            let denom = u.fraction.max(1) as f64;
            let total = (v * denom).round() / denom;
            let feet = (total / 12.0).floor();
            let rem = total - feet * 12.0;
            let whole = rem.floor();
            let frac = ((rem - whole) * denom).round() as u32;
            let frac_s = if frac > 0 {
                let g = gcd(frac, u.fraction.max(1));
                format!(" {}/{}", frac / g, u.fraction.max(1) / g)
            } else {
                String::new()
            };
            if feet > 0.0 { format!("{}' {}{}\"", feet as i64, whole as i64, frac_s) } else { format!("{}{}\"", whole as i64, frac_s) }
        }
        unit => {
            let val = v / unit.inches_per_unit();
            format!("{:.*}{}", u.precision as usize, val, unit.suffix())
        }
    };
    let approx = {
        // SketchUp prefixes "~" when the displayed value is rounded.
        let back = parse_length(&body, u.unit).unwrap_or(v);
        if (back - v).abs() > 1e-6 { "~ " } else { "" }
    };
    format!("{}{}{}", approx, if neg { "-" } else { "" }, body)
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Parse a length as typed in the Measurements box.
///
/// Accepts `12`, `12.5`, `5'`, `5'6"`, `5' 6 1/2"`, `6"`, `1/2"`, `300mm`, `30cm`, `1.2m`, `2ft`, `3in`.
/// A bare number is interpreted in `default_unit`.
pub fn parse_length(s: &str, default_unit: LengthUnit) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    let s = s.trim_start_matches('~').trim();
    if s.is_empty() {
        return None;
    }
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r.trim()),
        None => (false, s),
    };
    let lower = s.to_ascii_lowercase();
    let v = if let Some(n) = lower.strip_suffix("mm") {
        parse_number(n)? / MM_PER_INCH
    } else if let Some(n) = lower.strip_suffix("cm") {
        parse_number(n)? * 10.0 / MM_PER_INCH
    } else if let Some(n) = lower.strip_suffix("km") {
        parse_number(n)? * 1_000_000.0 / MM_PER_INCH
    } else if let Some(n) = lower.strip_suffix('m') {
        parse_number(n)? * 1000.0 / MM_PER_INCH
    } else if let Some(n) = lower.strip_suffix("ft") {
        parse_number(n)? * 12.0
    } else if let Some(n) = lower.strip_suffix("in") {
        parse_number(n)?
    } else if lower.contains('\'') || lower.contains('"') {
        parse_feet_inches(&lower)?
    } else {
        parse_number(&lower)? * default_unit.inches_per_unit()
    };
    Some(if neg { -v } else { v })
}

fn parse_feet_inches(s: &str) -> Option<f64> {
    let mut total = 0.0;
    let rest = if let Some(idx) = s.find('\'') {
        let feet = s[..idx].trim();
        if !feet.is_empty() {
            total += parse_number(feet)? * 12.0;
        }
        &s[idx + 1..]
    } else {
        s
    };
    let rest = rest.trim().trim_end_matches('"').trim();
    if !rest.is_empty() {
        total += parse_number(rest)?;
    }
    Some(total)
}

/// Number with optional mixed fraction: `3`, `3.25`, `1/2`, `3 1/2`.
fn parse_number(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut total = 0.0;
    for part in s.split_whitespace() {
        if let Some((n, d)) = part.split_once('/') {
            let d: f64 = d.trim().parse().ok()?;
            if d == 0.0 {
                return None;
            }
            total += n.trim().parse::<f64>().ok()? / d;
        } else {
            total += part.parse::<f64>().ok()?;
        }
    }
    Some(total)
}

/// Parsed Measurements box input.
#[derive(Clone, Debug, PartialEq)]
pub enum VcbInput {
    /// One or more comma/semicolon separated lengths (e.g. `100` or `200,300`).
    Lengths(Vec<f64>),
    /// Absolute coordinate `[x,y,z]`.
    Absolute([f64; 3]),
    /// Relative coordinate `<x,y,z>`.
    Relative([f64; 3]),
    /// Array multiplier `3x` / `*3` / `x3`.
    Multiply(u32),
    /// Array divider `/3`.
    Divide(u32),
    /// Segment count `24s`.
    Sides(u32),
    /// Anything else, passed through for tool-specific handling.
    Raw(String),
}

pub fn parse_vcb(s: &str, unit: LengthUnit) -> Option<VcbInput> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    if let Some(n) = lower.strip_suffix('x').or_else(|| lower.strip_prefix('x')).or_else(|| lower.strip_prefix('*'))
        && let Ok(k) = n.trim().parse() {
            return Some(VcbInput::Multiply(k));
        }
    if let Some(n) = lower.strip_prefix('/')
        && let Ok(k) = n.trim().parse() {
            return Some(VcbInput::Divide(k));
        }
    if let Some(n) = lower.strip_suffix('s')
        && let Ok(k) = n.trim().parse() {
            return Some(VcbInput::Sides(k));
        }
    let triple = |inner: &str| -> Option<[f64; 3]> {
        let parts: Vec<_> = inner.split([',', ';']).collect();
        if parts.len() != 3 {
            return None;
        }
        Some([parse_length(parts[0], unit)?, parse_length(parts[1], unit)?, parse_length(parts[2], unit)?])
    };
    if let Some(inner) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        return triple(inner).map(VcbInput::Absolute);
    }
    if let Some(inner) = t.strip_prefix('<').and_then(|r| r.strip_suffix('>')) {
        return triple(inner).map(VcbInput::Relative);
    }
    // Lengths: allow ";" always, "," only when not used as decimal separator (heuristic: split on ',' if
    // every piece parses as a length).
    let try_split = |sep: &[char]| -> Option<Vec<f64>> { t.split(sep).map(|p| parse_length(p, unit)).collect() };
    if let Some(v) = try_split(&[';']).filter(|v| v.len() > 1) {
        return Some(VcbInput::Lengths(v));
    }
    if let Some(v) = try_split(&[',']) {
        return Some(VcbInput::Lengths(v));
    }
    if let Some(v) = parse_length(t, unit) {
        return Some(VcbInput::Lengths(vec![v]));
    }
    Some(VcbInput::Raw(t.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_various() {
        let a = LengthUnit::Architectural;
        assert_eq!(parse_length("5'6\"", a), Some(66.0));
        assert_eq!(parse_length("5' 6 1/2\"", a), Some(66.5));
        assert_eq!(parse_length("1/2\"", a), Some(0.5));
        assert_eq!(parse_length("2'", a), Some(24.0));
        assert!((parse_length("254mm", a).unwrap() - 10.0).abs() < 1e-9);
        assert!((parse_length("1m", a).unwrap() - 39.37007874).abs() < 1e-6);
        assert!((parse_length("100", LengthUnit::Millimeters).unwrap() - 3.937007874).abs() < 1e-6);
        assert_eq!(parse_length("-3", a), Some(-3.0));
    }

    #[test]
    fn format_arch() {
        let u = UnitSettings { unit: LengthUnit::Architectural, precision: 0, fraction: 16 };
        assert_eq!(format_length(66.5, &u), "5' 6 1/2\"");
        assert_eq!(format_length(6.0, &u), "6\"");
    }

    #[test]
    fn format_mm() {
        let u = UnitSettings::default();
        assert_eq!(format_length(10.0, &u), "254mm");
        assert_eq!(format_length(1.0, &u), "~ 25mm");
    }

    #[test]
    fn vcb() {
        let u = LengthUnit::Millimeters;
        assert_eq!(parse_vcb("3x", u), Some(VcbInput::Multiply(3)));
        assert_eq!(parse_vcb("/4", u), Some(VcbInput::Divide(4)));
        assert_eq!(parse_vcb("24s", u), Some(VcbInput::Sides(24)));
        match parse_vcb("254mm,508mm", u) {
            Some(VcbInput::Lengths(v)) => assert!((v[0] - 10.0).abs() < 1e-9 && (v[1] - 20.0).abs() < 1e-9),
            o => panic!("{o:?}"),
        }
        match parse_vcb("[0,0,25.4]", u) {
            Some(VcbInput::Absolute(p)) => assert!((p[2] - 1.0).abs() < 1e-9),
            o => panic!("{o:?}"),
        }
    }
}
