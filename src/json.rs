//! Minimal JSON parser/serializer (no external crates).
//!
//! Only what an OpenAI-compatible chat client needs: objects, arrays, strings
//! (with \uXXXX and surrogate pairs), numbers, bools, null.

#[derive(Debug, Clone)]
pub enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    pub fn get(&self, key: &str) -> Option<&J> {
        match self {
            J::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&Vec<J>> {
        match self {
            J::Arr(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_num(&self) -> Option<f64> {
        match self {
            J::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn s(v: &str) -> J {
        J::Str(v.to_string())
    }
    pub fn o(pairs: Vec<(&str, J)>) -> J {
        J::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn dump(&self) -> String {
        let mut out = String::with_capacity(256);
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            J::Null => out.push_str("null"),
            J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            J::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    out.push_str(&format!("{}", *n as i64));
                } else {
                    out.push_str(&format!("{}", n));
                }
            }
            J::Str(s) => write_str(s, out),
            J::Arr(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            J::Obj(kv) => {
                out.push('{');
                for (i, (k, v)) in kv.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---------------------------------------------------------------- parsing

pub fn parse(input: &str) -> Option<J> {
    let b = input.as_bytes();
    let mut i = 0usize;
    let v = value(b, &mut i)?;
    ws(b, &mut i);
    if i == b.len() {
        Some(v)
    } else {
        None
    }
}

fn ws(b: &[u8], i: &mut usize) {
    while *i < b.len() && matches!(b[*i], b' ' | b'\t' | b'\n' | b'\r') {
        *i += 1;
    }
}

fn value(b: &[u8], i: &mut usize) -> Option<J> {
    ws(b, i);
    if *i >= b.len() {
        return None;
    }
    match b[*i] {
        b'{' => obj(b, i),
        b'[' => arr(b, i),
        b'"' => string(b, i).map(J::Str),
        b't' => lit(b, i, b"true", J::Bool(true)),
        b'f' => lit(b, i, b"false", J::Bool(false)),
        b'n' => lit(b, i, b"null", J::Null),
        _ => number(b, i),
    }
}

fn lit(b: &[u8], i: &mut usize, word: &[u8], v: J) -> Option<J> {
    if b.len() >= *i + word.len() && &b[*i..*i + word.len()] == word {
        *i += word.len();
        Some(v)
    } else {
        None
    }
}

fn obj(b: &[u8], i: &mut usize) -> Option<J> {
    *i += 1; // {
    let mut kv = Vec::new();
    ws(b, i);
    if *i < b.len() && b[*i] == b'}' {
        *i += 1;
        return Some(J::Obj(kv));
    }
    loop {
        ws(b, i);
        let k = string(b, i)?;
        ws(b, i);
        if *i >= b.len() || b[*i] != b':' {
            return None;
        }
        *i += 1;
        let v = value(b, i)?;
        kv.push((k, v));
        ws(b, i);
        if *i >= b.len() {
            return None;
        }
        match b[*i] {
            b',' => *i += 1,
            b'}' => {
                *i += 1;
                return Some(J::Obj(kv));
            }
            _ => return None,
        }
    }
}

fn arr(b: &[u8], i: &mut usize) -> Option<J> {
    *i += 1; // [
    let mut items = Vec::new();
    ws(b, i);
    if *i < b.len() && b[*i] == b']' {
        *i += 1;
        return Some(J::Arr(items));
    }
    loop {
        let v = value(b, i)?;
        items.push(v);
        ws(b, i);
        if *i >= b.len() {
            return None;
        }
        match b[*i] {
            b',' => *i += 1,
            b']' => {
                *i += 1;
                return Some(J::Arr(items));
            }
            _ => return None,
        }
    }
}

fn string(b: &[u8], i: &mut usize) -> Option<String> {
    if *i >= b.len() || b[*i] != b'"' {
        return None;
    }
    *i += 1;
    let mut out = String::new();
    loop {
        if *i >= b.len() {
            return None;
        }
        match b[*i] {
            b'"' => {
                *i += 1;
                return Some(out);
            }
            b'\\' => {
                *i += 1;
                if *i >= b.len() {
                    return None;
                }
                let esc = b[*i];
                *i += 1;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let hi = hex4(b, i)?;
                        let mut cp = hi as u32;
                        if (0xD800..0xDC00).contains(&cp) {
                            // surrogate pair
                            if *i + 1 < b.len() && b[*i] == b'\\' && b[*i + 1] == b'u' {
                                *i += 2;
                                let lo = hex4(b, i)? as u32;
                                if (0xDC00..0xE000).contains(&lo) {
                                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                }
                            }
                        }
                        out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                    }
                    _ => return None,
                }
            }
            _ => {
                // raw UTF-8 run up to the next quote/backslash
                let start = *i;
                while *i < b.len() && b[*i] != b'"' && b[*i] != b'\\' {
                    *i += 1;
                }
                out.push_str(std::str::from_utf8(&b[start..*i]).ok()?);
            }
        }
    }
}

fn hex4(b: &[u8], i: &mut usize) -> Option<u16> {
    if *i + 4 > b.len() {
        return None;
    }
    let s = std::str::from_utf8(&b[*i..*i + 4]).ok()?;
    *i += 4;
    u16::from_str_radix(s, 16).ok()
}

fn number(b: &[u8], i: &mut usize) -> Option<J> {
    let start = *i;
    while *i < b.len() && matches!(b[*i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
        *i += 1;
    }
    if start == *i {
        return None;
    }
    std::str::from_utf8(&b[start..*i]).ok()?.parse::<f64>().ok().map(J::Num)
}
