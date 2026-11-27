//! A small JSON value, parser and writer (the workspace has no serde_json).

use std::fmt::Write as _;

/// Nesting deeper than this is refused (a request cannot make the parser
/// recurse without bound).
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError(pub String);

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Json {
    /// An object from pairs.
    pub fn obj<const N: usize>(pairs: [(&str, Json); N]) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }

    pub fn str(s: impl Into<String>) -> Json {
        Json::Str(s.into())
    }

    /// A float, or null when it is not finite.
    pub fn num(v: f64) -> Json {
        if v.is_finite() { Json::Num(v) } else { Json::Null }
    }

    pub fn uint(v: u64) -> Json {
        i64::try_from(v).map_or(Json::Num(v as f64), Json::Int)
    }

    /// The member `key` of an object (the last one, if it repeats).
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(pairs) => pairs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// An integer: an integer literal, or a float with no fraction.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Int(i) => Some(*i),
            Json::Num(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => Some(*f as i64),
            _ => None,
        }
    }

    pub fn is_obj(&self) -> bool {
        matches!(self, Json::Obj(_))
    }

    /// The compact text of the value.
    pub fn text(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    pub fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Int(i) => {
                let _ = write!(out, "{i}");
            }
            Json::Num(f) => {
                if f.is_finite() {
                    let _ = write!(out, "{f}");
                } else {
                    out.push_str("null");
                }
            }
            Json::Str(s) => write_string(out, s),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Obj(pairs) => {
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(out, k);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parses one JSON document (UTF-8, nothing after it but white space).
pub fn parse(bytes: &[u8]) -> Result<Json, JsonError> {
    let text = std::str::from_utf8(bytes).map_err(|_| JsonError("not UTF-8".into()))?;
    let mut p = Parser { s: text.as_bytes(), text, i: 0 };
    p.space();
    let v = p.value(0)?;
    p.space();
    if p.i != p.s.len() {
        return Err(p.fail("text after the value"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
}

impl Parser<'_> {
    fn fail(&self, what: &str) -> JsonError {
        JsonError(format!("{what} at byte {}", self.i))
    }

    fn space(&mut self) {
        while matches!(self.s.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> Result<(), JsonError> {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            Ok(())
        } else {
            Err(self.fail("unexpected text"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.fail("nested too deep"));
        }
        match self.s.get(self.i) {
            None => Err(self.fail("unexpected end")),
            Some(b'n') => self.eat("null").map(|()| Json::Null),
            Some(b't') => self.eat("true").map(|()| Json::Bool(true)),
            Some(b'f') => self.eat("false").map(|()| Json::Bool(false)),
            Some(b'"') => self.string().map(Json::Str),
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.space();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Json::Arr(items));
                }
                loop {
                    self.space();
                    items.push(self.value(depth + 1)?);
                    self.space();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(items));
                        }
                        _ => return Err(self.fail("expected , or ]")),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut pairs = Vec::new();
                self.space();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(pairs));
                }
                loop {
                    self.space();
                    if self.s.get(self.i) != Some(&b'"') {
                        return Err(self.fail("expected a member name"));
                    }
                    let key = self.string()?;
                    self.space();
                    if self.s.get(self.i) != Some(&b':') {
                        return Err(self.fail("expected :"));
                    }
                    self.i += 1;
                    self.space();
                    pairs.push((key, self.value(depth + 1)?));
                    self.space();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(pairs));
                        }
                        _ => return Err(self.fail("expected , or }")),
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.fail("unexpected character")),
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.i;
        while matches!(self.s.get(self.i), Some(b'0'..=b'9')) {
            self.i += 1;
        }
        self.i - start
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.i;
        if self.s.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        match self.s.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.fail("bad number")),
        }
        let mut float = false;
        if self.s.get(self.i) == Some(&b'.') {
            float = true;
            self.i += 1;
            if self.digits() == 0 {
                return Err(self.fail("bad number"));
            }
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            float = true;
            self.i += 1;
            if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if self.digits() == 0 {
                return Err(self.fail("bad number"));
            }
        }
        let t = &self.text[start..self.i];
        if !float && let Ok(i) = t.parse::<i64>() {
            return Ok(Json::Int(i));
        }
        t.parse::<f64>().map(Json::Num).map_err(|_| self.fail("bad number"))
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let h = self.s.get(self.i..self.i + 4).ok_or_else(|| self.fail("short \\u escape"))?;
        let h = std::str::from_utf8(h).map_err(|_| self.fail("bad \\u escape"))?;
        let v = u32::from_str_radix(h, 16).map_err(|_| self.fail("bad \\u escape"))?;
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.i += 1; // the opening quote
        let mut out = String::new();
        loop {
            let Some(&b) = self.s.get(self.i) else {
                return Err(self.fail("string not closed"));
            };
            match b {
                b'"' => {
                    self.i += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.i += 1;
                    let Some(&e) = self.s.get(self.i) else {
                        return Err(self.fail("string not closed"));
                    };
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let c = if (0xd800..0xdc00).contains(&hi) {
                                if !self.s[self.i..].starts_with(b"\\u") {
                                    return Err(self.fail("lone surrogate"));
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&lo) {
                                    return Err(self.fail("lone surrogate"));
                                }
                                0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00)
                            } else {
                                hi
                            };
                            out.push(char::from_u32(c).ok_or_else(|| self.fail("bad \\u escape"))?);
                        }
                        _ => return Err(self.fail("bad escape")),
                    }
                }
                0..=0x1f => return Err(self.fail("control character in a string")),
                _ => {
                    // A run of plain characters (the text is valid UTF-8).
                    let start = self.i;
                    while let Some(&b) = self.s.get(self.i) {
                        if b == b'"' || b == b'\\' || b < 0x20 {
                            break;
                        }
                        self.i += 1;
                    }
                    out.push_str(&self.text[start..self.i]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_values() {
        let v = parse(br#" {"a":[1,-2,3.5,1e2,true,false,null],"b":{"c":"x\ny\u00e9\ud83d\ude00"}} "#).unwrap();
        assert_eq!(v.get("a"), Some(&Json::Arr(vec![
            Json::Int(1), Json::Int(-2), Json::Num(3.5), Json::Num(100.0),
            Json::Bool(true), Json::Bool(false), Json::Null,
        ])));
        assert_eq!(v.get("b").and_then(|b| b.get("c")).and_then(Json::as_str), Some("x\ny\u{e9}\u{1f600}"));
        assert_eq!(parse(v.text().as_bytes()).unwrap().text(), v.text());
    }

    #[test]
    fn refuses_bad_text() {
        for bad in ["", "{", "[1,]", "{\"a\":}", "01", "1.", "\"a", "\"\\x\"", "tru", "1 2", "\"\u{1}\"", "\"\\ud800\"", "[1}"] {
            assert!(parse(bad.as_bytes()).is_err(), "{bad:?}");
        }
        assert!(parse(&[b'"', 0xff, b'"']).is_err());
        let deep = "[".repeat(200) + &"]".repeat(200);
        assert!(parse(deep.as_bytes()).is_err());
    }

    #[test]
    fn writes_escapes_and_non_finite() {
        assert_eq!(Json::str("a\"b\\c\n\u{1}").text(), "\"a\\\"b\\\\c\\n\\u0001\"");
        assert_eq!(Json::num(f64::NAN).text(), "null");
        assert!(Json::uint(u64::MAX).text().parse::<f64>().is_ok());
    }

    #[test]
    fn integers_keep_their_digits() {
        assert_eq!(parse(b"9007199254740993").unwrap(), Json::Int(9007199254740993));
        assert_eq!(parse(b"2.0").unwrap().as_i64(), Some(2));
        assert_eq!(parse(b"2.5").unwrap().as_i64(), None);
    }
}
