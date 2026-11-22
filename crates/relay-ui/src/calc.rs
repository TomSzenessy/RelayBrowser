//! Inline calculator for the palette: a small recursive-descent evaluator for
//! `+ - * / % ^`, parentheses, unary minus and decimals. It evaluates nothing
//! else and never panics; anything it cannot prove is arithmetic is `None`.

const MAX_DEPTH: usize = 64;
const MAX_INPUT: usize = 256;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    LParen,
    RParen,
}

fn lex(s: &str) -> Option<Vec<Tok>> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = vec![];
    while i < b.len() {
        let c = b[i];
        match c {
            b' ' | b'\t' => i += 1,
            b'+' | b'-' | b'*' | b'/' | b'%' | b'^' | b'(' | b')' => {
                out.push(match c {
                    b'+' => Tok::Plus,
                    b'-' => Tok::Minus,
                    b'*' => Tok::Star,
                    b'/' => Tok::Slash,
                    b'%' => Tok::Percent,
                    b'^' => Tok::Caret,
                    b'(' => Tok::LParen,
                    _ => Tok::RParen,
                });
                i += 1;
            }
            b'0'..=b'9' | b'.' => {
                let start = i;
                let mut dots = 0;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                    dots += (b[i] == b'.') as usize;
                    i += 1;
                }
                let text = &s[start..i];
                // `1.2.3` (versions, IPs), `.`, and zero-padded runs like
                // `2024-01-05` (dates) are not numbers.
                if dots > 1 || text == "." || (text.len() > 1 && text.starts_with('0') && !text.starts_with("0.")) {
                    return None;
                }
                out.push(Tok::Num(text.parse().ok()?));
            }
            _ => return None,
        }
    }
    Some(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    binary_ops: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn expr(&mut self, depth: usize) -> Option<f64> {
        if depth > MAX_DEPTH {
            return None;
        }
        let mut v = self.term(depth)?;
        while let Some(t) = self.peek() {
            let plus = match t {
                Tok::Plus => true,
                Tok::Minus => false,
                _ => break,
            };
            self.pos += 1;
            self.binary_ops += 1;
            let r = self.term(depth)?;
            v = if plus { v + r } else { v - r };
        }
        Some(v)
    }

    fn term(&mut self, depth: usize) -> Option<f64> {
        let mut v = self.unary(depth)?;
        while let Some(t) = self.peek().cloned() {
            if !matches!(t, Tok::Star | Tok::Slash | Tok::Percent) {
                break;
            }
            self.pos += 1;
            self.binary_ops += 1;
            let r = self.unary(depth)?;
            v = match t {
                Tok::Star => v * r,
                _ if r == 0.0 => return None,
                Tok::Slash => v / r,
                _ => v % r,
            };
        }
        Some(v)
    }

    /// Unary minus binds looser than `^`: `-2^2 == -4`.
    fn unary(&mut self, depth: usize) -> Option<f64> {
        if depth > MAX_DEPTH {
            return None;
        }
        match self.peek() {
            Some(Tok::Minus) => {
                self.pos += 1;
                Some(-self.unary(depth + 1)?)
            }
            Some(Tok::Plus) => {
                self.pos += 1;
                self.unary(depth + 1)
            }
            _ => self.power(depth),
        }
    }

    fn power(&mut self, depth: usize) -> Option<f64> {
        let base = self.atom(depth)?;
        if self.peek() == Some(&Tok::Caret) {
            self.pos += 1;
            self.binary_ops += 1;
            // Right associative; the exponent may itself be negative.
            let exp = self.unary(depth + 1)?;
            return Some(base.powf(exp));
        }
        Some(base)
    }

    fn atom(&mut self, depth: usize) -> Option<f64> {
        match self.toks.get(self.pos)?.clone() {
            Tok::Num(n) => {
                self.pos += 1;
                Some(n)
            }
            Tok::LParen => {
                self.pos += 1;
                let v = self.expr(depth + 1)?;
                if self.peek() != Some(&Tok::RParen) {
                    return None;
                }
                self.pos += 1;
                Some(v)
            }
            _ => None,
        }
    }
}

fn format_number(v: f64) -> Option<String> {
    if !v.is_finite() {
        return None;
    }
    let v = if v == 0.0 { 0.0 } else { v };
    if v.fract() == 0.0 {
        return Some(if v.abs() < 1e21 { format!("{v}") } else { format!("{v:e}") });
    }
    let s = format!("{v:.10}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    Some(if s.is_empty() || s == "-0" { "0".to_string() } else { s.to_string() })
}

/// Evaluate `input` as arithmetic. `None` for non-math, bare numbers, version
/// or IP-like text, division by zero, and results that are not finite.
pub fn eval(input: &str) -> Option<String> {
    let t = input.trim();
    if t.len() > MAX_INPUT {
        return None;
    }
    let (t, forced) = match t.strip_prefix('=') {
        Some(rest) => (rest.trim(), true),
        None => (t, false),
    };
    let toks = lex(t)?;
    if toks.is_empty() {
        return None;
    }
    let mut p = Parser { toks, pos: 0, binary_ops: 0 };
    let v = p.expr(0)?;
    if p.pos != p.toks.len() {
        return None;
    }
    let bare_number = p.toks.len() == 1;
    if bare_number || (p.binary_ops == 0 && !forced) {
        return None;
    }
    format_number(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(s: &str) -> Option<String> {
        eval(s)
    }

    #[test]
    fn basic_arithmetic() {
        for (i, want) in [("1+2", "3"), ("2*3+4", "10"), ("2+3*4", "14"), ("(2+3)*4", "20"), ("10/4", "2.5"), ("7%3", "1"), ("2^10", "1024"), ("10-3-2", "5"), ("100/10/5", "2")] {
            assert_eq!(e(i).as_deref(), Some(want), "{i}");
        }
    }

    #[test]
    fn precedence_and_associativity() {
        assert_eq!(e("2^3^2").as_deref(), Some("512"));
        assert_eq!(e("-2^2").as_deref(), Some("-4"));
        assert_eq!(e("(-2)^2").as_deref(), Some("4"));
        assert_eq!(e("2^-1").as_deref(), Some("0.5"));
        assert_eq!(e("2*-3").as_deref(), Some("-6"));
        assert_eq!(e("5--3").as_deref(), Some("8"));
        assert_eq!(e("5+ +3").as_deref(), Some("8"));
        assert_eq!(e("7%4*2").as_deref(), Some("6"));
    }

    #[test]
    fn decimals_and_formatting() {
        assert_eq!(e("0.1+0.2").as_deref(), Some("0.3"));
        assert_eq!(e("1/3").as_deref(), Some("0.3333333333"));
        assert_eq!(e("2/3").as_deref(), Some("0.6666666667"));
        assert_eq!(e("1.5*2").as_deref(), Some("3"));
        assert_eq!(e(".5+.5").as_deref(), Some("1"));
        assert_eq!(e("5.+1").as_deref(), Some("6"));
        assert_eq!(e("0.00000000001*1").as_deref(), Some("0"));
        assert_eq!(e("-0.00000000001*1").as_deref(), Some("0"));
        assert_eq!(e("0*-1").as_deref(), Some("0"));
        assert_eq!(e("2^50").as_deref(), Some("1125899906842624"));
    }

    #[test]
    fn whitespace_and_leading_equals() {
        assert_eq!(e("  1 +   2 ").as_deref(), Some("3"));
        assert_eq!(e("= 1+2").as_deref(), Some("3"));
        assert_eq!(e("=1+2").as_deref(), Some("3"));
        assert_eq!(e("=-5").as_deref(), Some("-5"));
        assert_eq!(e("=(5)").as_deref(), Some("5"));
        assert_eq!(e("=5"), None);
    }

    #[test]
    fn bare_numbers_versions_and_ips_are_not_math() {
        for s in ["5", "-5", "+5", "(5)", "2.5", "1.2.3", "192.168.1.1", "10.0.0.1/8", "v1.2", "1.2.3-beta", "2024-01-05", "007", "", "   ", "=", "."] {
            assert_eq!(e(s), None, "{s:?}");
        }
    }

    #[test]
    fn non_math_text_is_none() {
        for s in ["hello", "2 apples + 3", "1+a", "sin(1)", "1e5+1", "1,000+1", "0x10+1", "1+", "+", "*2", "1+*2", "(1+2", "1+2)", "()", "1 2", "2(3)", "(1)(2)", "1;2", "rm -rf /", "!gh 1+1", "1+2 foo", "1 + 2 = 3"] {
            assert_eq!(e(s), None, "{s:?}");
        }
    }

    #[test]
    fn division_by_zero_overflow_nan_inf() {
        for s in ["1/0", "0/0", "5%0", "1/(2-2)", "10^1000", "10^400*10^400", "(-8)^0.5", "0^-1", "9^9^9^9", "1000000^1000000"] {
            assert_eq!(e(s), None, "{s}");
        }
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let ok = format!("{}1+1{}", "(".repeat(60), ")".repeat(60));
        assert_eq!(e(&ok).as_deref(), Some("2"));
        let deep = format!("{}1+1{}", "(".repeat(100), ")".repeat(100));
        assert_eq!(e(&deep), None);
        let huge = format!("{}1+1{}", "(".repeat(5000), ")".repeat(5000));
        assert_eq!(e(&huge), None);
        let minuses = format!("{}1+1", "-".repeat(100));
        assert_eq!(e(&minuses), None);
        let pows = vec!["2"; 100].join("^");
        assert_eq!(e(&pows), None);
        let long_sum = vec!["1"; 5000].join("+");
        assert_eq!(e(&long_sum), None, "oversize input rejected");
    }

    #[test]
    fn fuzz_style_inputs_never_panic() {
        let alphabet: Vec<char> = "0123456789.+-*/%^() =a!\u{e9}\u{1F600}".chars().collect();
        let mut seed: u64 = 0x9E3779B97F4A7C15;
        for _ in 0..20_000 {
            let mut s = String::new();
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let len = (seed % 24) as usize;
            for _ in 0..len {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                s.push(alphabet[(seed % alphabet.len() as u64) as usize]);
            }
            if let Some(out) = eval(&s) {
                assert!(!out.contains("NaN") && !out.contains("inf"), "{s:?} -> {out}");
            }
        }
    }
}
