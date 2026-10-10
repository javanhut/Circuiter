//! SPICE numbers (`4.7k`, `1meg`, `10uF`) and `{...}` expressions.

/// Parses a SPICE number. Scale suffixes are case-insensitive: `T` `G` `MEG` `K` `M`
/// (milli) `MIL` `U` `N` `P` `F`. Letters after the number and suffix are units and are
/// ignored, as in SPICE: `10uF` is 10e-6.
pub fn parse_number(text: &str) -> Option<f64> {
    let t = text.trim();
    let bytes = t.as_bytes();
    let mut end = 0;
    let mut seen_digit = false;
    if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
        end += 1;
    }
    while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b'.') {
        seen_digit |= bytes[end].is_ascii_digit();
        end += 1;
    }
    if !seen_digit {
        return None;
    }
    // Exponent, only if digits follow (so `1e` stays a unit, though unusual).
    let digits_end = end;
    let mut exponent: i32 = 0;
    if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
        let mut k = end + 1;
        if k < bytes.len() && (bytes[k] == b'+' || bytes[k] == b'-') {
            k += 1;
        }
        if k < bytes.len() && bytes[k].is_ascii_digit() {
            while k < bytes.len() && bytes[k].is_ascii_digit() {
                k += 1;
            }
            exponent = t[end + 1..k].parse().ok()?;
            end = k;
        }
    }
    let rest = t[end..].to_ascii_lowercase();
    if !rest.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if rest.starts_with("mil") {
        let v: f64 = format!("{}e{exponent}", &t[..digits_end]).parse().ok()?;
        return Some(v * 25.4e-6);
    }
    // Apply the scale as a decimal exponent so `10u` is exactly 1e-5.
    let scale = if rest.starts_with("meg") {
        6
    } else {
        match rest.chars().next() {
            Some('t') => 12,
            Some('g') => 9,
            Some('k') => 3,
            Some('m') => -3,
            Some('u') => -6,
            Some('n') => -9,
            Some('p') => -12,
            Some('f') => -15,
            Some('a') => -18,
            _ => 0,
        }
    };
    format!("{}e{}", &t[..digits_end], exponent + scale).parse().ok()
}

/// Evaluates an arithmetic expression: numbers with scale suffixes, `+ - * / ^` (`**`
/// too), parentheses and unary minus. Used for `{...}` values.
pub fn eval(text: &str) -> Result<f64, String> {
    eval_with(text, &|_| None)
}

/// Like [`eval`], with named parameters looked up by `vars` (case-insensitively; the
/// callback receives lower case) and the functions `sqrt exp ln log log10 abs sin cos
/// tan atan pow min max`.
pub fn eval_with(text: &str, vars: &dyn Fn(&str) -> Option<f64>) -> Result<f64, String> {
    let mut p = Expr { s: text.as_bytes(), i: 0, text, vars };
    let v = p.sum()?;
    p.ws();
    if p.i != p.s.len() {
        return Err(format!("unexpected `{}` in expression `{text}`", &text[p.i..]));
    }
    Ok(v)
}

struct Expr<'a> {
    s: &'a [u8],
    i: usize,
    text: &'a str,
    vars: &'a dyn Fn(&str) -> Option<f64>,
}

impl Expr<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }

    fn sum(&mut self) -> Result<f64, String> {
        let mut v = self.product()?;
        while let Some(op @ (b'+' | b'-')) = self.peek() {
            self.i += 1;
            let r = self.product()?;
            v = if op == b'+' { v + r } else { v - r };
        }
        Ok(v)
    }

    fn product(&mut self) -> Result<f64, String> {
        let mut v = self.power()?;
        while let Some(op @ (b'*' | b'/')) = self.peek() {
            self.i += 1;
            let r = self.power()?;
            v = if op == b'*' { v * r } else { v / r };
        }
        Ok(v)
    }

    fn power(&mut self) -> Result<f64, String> {
        let base = self.unary()?;
        if self.peek() == Some(b'^') {
            self.i += 1;
            return Ok(base.powf(self.power()?));
        }
        if self.peek() == Some(b'*') && self.s.get(self.i + 1) == Some(&b'*') {
            self.i += 2;
            return Ok(base.powf(self.power()?));
        }
        Ok(base)
    }

    fn call(&mut self, name: &str) -> Result<f64, String> {
        self.i += 1; // `(`
        let mut args = vec![self.sum()?];
        while self.peek() == Some(b',') {
            self.i += 1;
            args.push(self.sum()?);
        }
        if self.peek() != Some(b')') {
            return Err(format!("missing `)` after the arguments of `{name}`"));
        }
        self.i += 1;
        let one = |f: fn(f64) -> f64| match args[..] {
            [x] => Ok(f(x)),
            _ => Err(format!("`{name}` takes one argument")),
        };
        let two = |f: fn(f64, f64) -> f64| match args[..] {
            [x, y] => Ok(f(x, y)),
            _ => Err(format!("`{name}` takes two arguments")),
        };
        match name {
            "sqrt" => one(f64::sqrt),
            "exp" => one(f64::exp),
            "ln" | "log" => one(f64::ln),
            "log10" => one(f64::log10),
            "abs" => one(f64::abs),
            "sin" => one(f64::sin),
            "cos" => one(f64::cos),
            "tan" => one(f64::tan),
            "atan" => one(f64::atan),
            "pow" | "pwr" => two(f64::powf),
            "min" => two(f64::min),
            "max" => two(f64::max),
            _ => Err(format!("unknown function `{name}`")),
        }
    }

    fn unary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some(b'-') => {
                self.i += 1;
                Ok(-self.unary()?)
            }
            Some(b'+') => {
                self.i += 1;
                self.unary()
            }
            Some(b'(') => {
                self.i += 1;
                let v = self.sum()?;
                if self.peek() != Some(b')') {
                    return Err(format!("missing `)` in `{}`", self.text));
                }
                self.i += 1;
                Ok(v)
            }
            _ => {
                let start = self.i;
                while self.i < self.s.len() {
                    let c = self.s[self.i];
                    let exponent_sign = (c == b'+' || c == b'-')
                        && self.i > start
                        && matches!(self.s[self.i - 1], b'e' | b'E')
                        && self.s[start..self.i - 1]
                            .iter()
                            .all(|b| b.is_ascii_digit() || *b == b'.');
                    if c.is_ascii_alphanumeric() || c == b'.' || c == b'_' || exponent_sign {
                        self.i += 1;
                    } else {
                        break;
                    }
                }
                let token = &self.text[start..self.i];
                if token.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
                    let name = token.to_ascii_lowercase();
                    if self.peek() == Some(b'(') {
                        return self.call(&name);
                    }
                    if name == "pi" {
                        return Ok(std::f64::consts::PI);
                    }
                    return (self.vars)(&name)
                        .ok_or_else(|| format!("unknown parameter `{token}`"));
                }
                parse_number(token).ok_or_else(|| format!("`{token}` is not a number"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_with_scale_suffixes() {
        assert_eq!(parse_number("4.7k"), Some(4700.0));
        assert_eq!(parse_number("1MEG"), Some(1e6));
        assert_eq!(parse_number("1m"), Some(1e-3));
        assert_eq!(parse_number("10uF"), Some(10e-6));
        assert_eq!(parse_number("1.5e-7"), Some(1.5e-7));
        assert_eq!(parse_number("-2"), Some(-2.0));
        assert_eq!(parse_number("3V"), Some(3.0));
        assert_eq!(parse_number("abc"), None);
        assert_eq!(parse_number("1-2"), None);
    }

    #[test]
    fn expressions() {
        assert_eq!(eval("1/1e8").unwrap(), 1e-8);
        assert!((eval("0.5/1e8-5e-11").unwrap() - 4.95e-9).abs() < 1e-20);
        assert_eq!(eval("-(2+3)*2^2").unwrap(), -20.0);
        assert_eq!(eval("1k*2").unwrap(), 2000.0);
        assert!(eval("1+").is_err());
    }

    #[test]
    fn parameters_and_functions() {
        let vars = |n: &str| match n {
            "rload" => Some(2e3),
            "gain_db" => Some(20.0),
            _ => None,
        };
        assert_eq!(eval_with("RLOAD*2", &vars).unwrap(), 4e3);
        assert!((eval_with("10**(gain_db/20)", &vars).unwrap() - 10.0).abs() < 1e-12);
        assert_eq!(eval_with("max(1, sqrt(16))", &vars).unwrap(), 4.0);
        assert!(eval_with("nope*2", &vars).unwrap_err().contains("nope"));
    }
}
