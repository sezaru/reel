//! CSS colour text → straight RGBA in 0…1.

use crate::canvas::Col;

/// "#rgb", "#rgba", "#rrggbb", "#rrggbbaa", "rgb(…)", "rgba(…)" (commas or spaces, alpha
/// after a comma or a slash, as a number or a percentage), "transparent", "white", "black".
pub fn parse(s: &str) -> Option<Col> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix('#') {
        let d = |c: char| c.to_digit(16).map(|v| v as f64);
        let cs: Vec<char> = h.chars().collect();
        return match cs.len() {
            3 | 4 => {
                let v: Option<Vec<f64>> = cs.iter().map(|c| d(*c).map(|x| x * 17.0 / 255.0)).collect();
                let v = v?;
                Some([v[0], v[1], v[2], v.get(3).copied().unwrap_or(1.0)])
            }
            6 | 8 => {
                let byte = |i: usize| Some((d(cs[i])? * 16.0 + d(cs[i + 1])?) / 255.0);
                Some([byte(0)?, byte(2)?, byte(4)?, if cs.len() == 8 { byte(6)? } else { 1.0 }])
            }
            _ => None,
        };
    }
    match s {
        "transparent" => return Some([0.0; 4]),
        "white" => return Some([1.0; 4]),
        "black" => return Some([0.0, 0.0, 0.0, 1.0]),
        _ => {}
    }
    let inner = s.strip_prefix("rgba(").or_else(|| s.strip_prefix("rgb("))?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(|c: char| c == ',' || c == '/' || c.is_whitespace()).filter(|p| !p.is_empty()).collect();
    if parts.len() < 3 || parts.len() > 4 {
        return None;
    }
    let chan = |p: &str| -> Option<f64> {
        match p.strip_suffix('%') {
            Some(v) => v.parse::<f64>().ok().map(|v| v / 100.0),
            None => p.parse::<f64>().ok().map(|v| v / 255.0),
        }
    };
    let a = match parts.get(3) {
        Some(p) => match p.strip_suffix('%') {
            Some(v) => v.parse::<f64>().ok()? / 100.0,
            None => p.parse::<f64>().ok()?,
        },
        None => 1.0,
    };
    Some([chan(parts[0])?, chan(parts[1])?, chan(parts[2])?, a])
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn forms() {
        assert_eq!(parse("#fff"), Some([1.0, 1.0, 1.0, 1.0]));
        assert_eq!(parse("#ff000080").map(|c| (c[0], (c[3] * 255.0).round())), Some((1.0, 128.0)));
        assert_eq!(parse("rgba(255, 0, 0, 0.5)"), Some([1.0, 0.0, 0.0, 0.5]));
        assert_eq!(parse("rgb(255 0 0 / 50%)"), Some([1.0, 0.0, 0.0, 0.5]));
        assert_eq!(parse("nope"), None);
    }
}
