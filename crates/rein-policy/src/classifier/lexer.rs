// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Shell lexer for quote removal, ANSI-C escapes, $IFS splitting, braces, and substitutions

/// Extracted substitutions and validation status from a raw shell command string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexedScript {
    /// Sub-scripts inside unescaped `$(...)`, `` `...` ``, or `<(...)`.
    pub substitutions: Vec<String>,
    /// Whether a substitution or unresolved expansion sits in command position (`C02`, `C07`).
    pub dynamic_command: bool,
}

/// Validates quote/substitution balance and extracts active sub-commands (`$(...)`, `` `...` ``, `<(...)`).
pub fn scan_script(script: &str) -> Result<LexedScript, &'static str> {
    if script.contains("[[") && (script.contains("$(") || script.contains('`')) {
        return Ok(LexedScript {
            substitutions: Vec::new(),
            dynamic_command: true,
        });
    }
    let trimmed = script.trim_start();
    let dynamic_command = trimmed.starts_with("$(") || trimmed.starts_with('`');
    let mut substitutions = Vec::new();
    extract_substitutions(script, &mut substitutions)?;
    Ok(LexedScript {
        substitutions,
        dynamic_command,
    })
}

fn extract_substitutions(s: &str, out: &mut Vec<String>) -> Result<(), &'static str> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;

    while i < bytes.len() {
        let b = bytes[i];
        if in_single {
            if b == b'\'' {
                in_single = false;
            }
            i += 1;
        } else if b == b'\\' {
            if i + 1 >= bytes.len() {
                return Err("trailing backslash");
            }
            i += 2;
        } else if b == b'\'' && !in_double {
            if i > 0 && bytes[i - 1] == b'$' {
                i = skip_ansi_c_quote(bytes, i + 1)?;
            } else {
                in_single = true;
                i += 1;
            }
        } else if b == b'"' {
            in_double = !in_double;
            i += 1;
        } else if (b == b'$' || (!in_double && b == b'<'))
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'('
        {
            let (inner, next_i) = slice_balanced_parens(s, i + 2)?;
            out.push(inner.to_owned());
            extract_substitutions(inner, out)?;
            i = next_i;
        } else if b == b'`' {
            let (inner, next_i) = slice_backticks(s, i + 1)?;
            out.push(inner.to_owned());
            i = next_i;
        } else {
            i += 1;
        }
    }
    if in_single || in_double {
        return Err("unterminated quote");
    }
    Ok(())
}

fn skip_ansi_c_quote(bytes: &[u8], mut i: usize) -> Result<usize, &'static str> {
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
        } else if bytes[i] == b'\'' {
            return Ok(i + 1);
        } else {
            i += 1;
        }
    }
    Err("unterminated ANSI-C quote")
}

fn slice_balanced_parens(s: &str, start: usize) -> Result<(&str, usize), &'static str> {
    let bytes = s.as_bytes();
    let mut depth = 1usize;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((&s[start..i], i + 1));
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    Err("unterminated $(...) substitution")
}

fn slice_backticks(s: &str, start: usize) -> Result<(&str, usize), &'static str> {
    let bytes = s.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
        } else if bytes[i] == b'`' {
            return Ok((&s[start..i], i + 1));
        } else {
            i += 1;
        }
    }
    Err("unterminated backtick substitution")
}

/// Normalizes a raw shell token by expanding `$IFS`, braces, ANSI-C `$'...'`, quotes, and `\`.
#[must_use]
pub fn normalize_and_expand_word(raw: &str) -> Vec<String> {
    if raw.starts_with('\'') || raw.starts_with('"') {
        return vec![unquote_word(raw)];
    }
    let ifs_replaced = raw.replace("${IFS}", " ").replace("$IFS", " ");
    let mut out = Vec::new();
    for piece in ifs_replaced.split_whitespace() {
        for expanded in expand_braces(piece) {
            out.push(unquote_word(&expanded));
        }
    }
    out
}

fn expand_braces(word: &str) -> Vec<String> {
    let Some(open) = word.find('{') else {
        return vec![word.to_owned()];
    };
    if open > 0 && word.as_bytes()[open - 1] == b'$' {
        return vec![word.to_owned()];
    }
    let Some(rel_close) = word[open + 1..].find('}') else {
        return vec![word.to_owned()];
    };
    let close = open + 1 + rel_close;
    let inner = &word[open + 1..close];
    if !inner.contains(',') {
        return vec![word.to_owned()];
    }
    let prefix = &word[..open];
    let suffix = &word[close + 1..];
    inner
        .split(',')
        .map(|part| format!("{prefix}{part}{suffix}"))
        .collect()
}

/// Removes quote delimiters (`'`, `"`, `$'...'`) and decodes hex/backslash escapes (`\x74`, `\rm`).
#[must_use]
pub fn unquote_word(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::new();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
            i = decode_ansi_c_segment(bytes, i + 2, &mut out);
        } else if bytes[i] == b'\'' {
            i = copy_single_quoted(bytes, i + 1, &mut out);
        } else if bytes[i] == b'"' {
            i = copy_double_quoted(bytes, i + 1, &mut out);
        } else if bytes[i] == b'\\' && i + 1 < bytes.len() {
            out.push(bytes[i + 1] as char);
            i += 2;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn decode_ansi_c_segment(bytes: &[u8], mut i: usize, out: &mut String) -> usize {
    while i < bytes.len() && bytes[i] != b'\'' {
        if bytes[i] == b'\\' && i + 3 < bytes.len() && bytes[i + 1] == b'x' {
            let hex = std::str::from_utf8(&bytes[i + 2..i + 4]).unwrap_or("");
            if let Ok(val) = u8::from_str_radix(hex, 16) {
                out.push(val as char);
                i += 4;
                continue;
            }
        } else if bytes[i] == b'\\' && i + 1 < bytes.len() {
            out.push(bytes[i + 1] as char);
            i += 2;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    (i + 1).min(bytes.len())
}

fn copy_single_quoted(bytes: &[u8], mut i: usize, out: &mut String) -> usize {
    while i < bytes.len() && bytes[i] != b'\'' {
        out.push(bytes[i] as char);
        i += 1;
    }
    (i + 1).min(bytes.len())
}

fn copy_double_quoted(bytes: &[u8], mut i: usize, out: &mut String) -> usize {
    while i < bytes.len() && bytes[i] != b'"' {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            out.push(bytes[i + 1] as char);
            i += 2;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    (i + 1).min(bytes.len())
}
