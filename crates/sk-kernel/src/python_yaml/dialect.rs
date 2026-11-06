//! The two Source YAML dialects, as one resolver/construction policy pair.
//!
//! Source does not use one YAML dialect:
//!
//! | document | loader | dialect |
//! |---|---|---|
//! | `mission.yaml` | `yaml.safe_load` (PyYAML 6.0.3) | YAML **1.1** |
//! | profiles, offerings, `project.path_conventions`, activation | `ruamel.yaml.YAML(typ="safe")` (0.19.1) | YAML **1.2** |
//!
//! Their implicit resolvers and construction rules genuinely differ, and the
//! differences are observable in these schemas (every row is a pinned Source
//! receipt, see the L4 report):
//!
//! | document | PyYAML `safe_load` | ruamel safe |
//! |---|---|---|
//! | `name: yes` | `bool True` | `str "yes"` |
//! | `v: 017` | `int 15` (octal) | `int 17` |
//! | `v: 0o17` | `str "0o17"` | `int 15` |
//! | `v: 1:30` | `int 90` (sexagesimal) | `str "1:30"` |
//! | `v: 1e5` | `str "1e5"` | `float 100000.0` |
//! | `v: 2026-11-05` | `datetime.date` | `datetime.date` |
//! | `!!bool y` | `KeyError` | `True` |
//! | `!!omap [{a: b}]` | `[('a', 'b')]` | `ordereddict([('a','b')])` |
//! | duplicate key | last value, first position | `DuplicateKeyError` |
//!
//! A single blanket "fidelity" switch is therefore not usable: supplying the
//! ruamel policy to a Mission `mission.yaml` read (or the reverse) silently
//! accepts and refuses the wrong documents.

/// One Source loader's YAML dialect.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Dialect {
    /// `yaml.safe_load`: PyYAML 6.0.3, YAML 1.1 (`mission.yaml`).
    PyYamlSafe11,
    /// `ruamel.yaml.YAML(typ="safe")` 0.19.1, YAML 1.2 (profiles/offerings).
    RuamelSafe12,
}

impl Dialect {
    /// The loader this dialect models, for diagnostics.
    pub const fn loader_name(self) -> &'static str {
        match self {
            Self::PyYamlSafe11 => "PyYAML safe_load (YAML 1.1)",
            Self::RuamelSafe12 => "ruamel.yaml YAML(typ=\"safe\") (YAML 1.2)",
        }
    }

    /// Whether this loader refuses a repeated mapping key.
    ///
    /// ruamel's safe loader raises `DuplicateKeyError` (a `YAMLError`); PyYAML
    /// keeps the last value in the first key's position.
    pub const fn refuses_duplicate_keys(self) -> bool {
        matches!(self, Self::RuamelSafe12)
    }

    /// The `!!bool` word table of this dialect (after lowercasing).
    ///
    /// PyYAML's table is `yes/no/true/false/on/off`; ruamel's adds the single
    /// letter `y`/`n` spellings. Both raise a bare `KeyError` otherwise.
    pub fn bool_word(self, lowered: &str) -> Option<bool> {
        match lowered {
            "yes" | "true" | "on" => Some(true),
            "no" | "false" | "off" => Some(false),
            "y" if self == Self::RuamelSafe12 => Some(true),
            "n" if self == Self::RuamelSafe12 => Some(false),
            _ => None,
        }
    }
}

/// The dialect's implicit resolver, in resolver priority order.
pub fn implicit_scalar_kind(text: &str, dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::PyYamlSafe11 => implicit_scalar_kind_yaml11(text),
        Dialect::RuamelSafe12 => implicit_scalar_kind_yaml12(text),
    }
}

/// PyYAML's YAML 1.1 resolver (`yaml/resolver.py`, `SafeLoader`).
///
/// `bool` is the full 1.1 vocabulary (but not the bare `y`/`n`, which PyYAML
/// leaves as strings); `int` accepts a leading-zero octal spelling, `0b`/`0x`
/// prefixes and a base-60 sexagesimal spelling; `float` needs a dot (so `1e5` is
/// a string in 1.1) and also accepts a sexagesimal spelling and `.inf`/`.nan`;
/// `null` is `~`, the null words, or empty; then the timestamp.
fn implicit_scalar_kind_yaml11(text: &str) -> &'static str {
    if is_bool_11(text) {
        return "bool";
    }
    if is_float_11(text) {
        return "float";
    }
    if is_int_11(text) {
        return "int";
    }
    if matches!(text, "~" | "null" | "Null" | "NULL" | "") {
        return "null";
    }
    if is_timestamp_resolver(text) {
        return "timestamp";
    }
    "str"
}

fn is_bool_11(text: &str) -> bool {
    matches!(
        text,
        "yes"
            | "Yes"
            | "YES"
            | "no"
            | "No"
            | "NO"
            | "true"
            | "True"
            | "TRUE"
            | "false"
            | "False"
            | "FALSE"
            | "on"
            | "On"
            | "ON"
            | "off"
            | "Off"
            | "OFF"
    )
}

/// `[-+]?0b[0-1_]+ | [-+]?0[0-7_]+ | [-+]?(0|[1-9][0-9_]*) | [-+]?0x[0-9a-fA-F_]+ | [-+]?[1-9][0-9_]*(:[0-5]?[0-9])+`
fn is_int_11(text: &str) -> bool {
    let body = text
        .strip_prefix('-')
        .or_else(|| text.strip_prefix('+'))
        .unwrap_or(text);
    if body.is_empty() {
        return false;
    }
    let digits_all = |rest: &str, allowed: &dyn Fn(char) -> bool| {
        !rest.is_empty()
            && rest
                .chars()
                .all(|character| allowed(character) || character == '_')
    };
    if let Some(rest) = body.strip_prefix("0b") {
        return digits_all(rest, &|character: char| matches!(character, '0' | '1'));
    }
    if let Some(rest) = body.strip_prefix("0x") {
        return digits_all(rest, &|character: char| character.is_ascii_hexdigit());
    }
    if let Some(rest) = body.strip_prefix('0')
        && digits_all(rest, &|character: char| matches!(character, '0'..='7'))
    {
        return true;
    }
    if body == "0" {
        return true;
    }
    let sexagesimal = |rest: &str| -> bool {
        let mut parts = rest.split(':');
        let Some(first) = parts.next() else {
            return false;
        };
        if first.is_empty()
            || first.starts_with('0')
            || !first
                .chars()
                .all(|character| character.is_ascii_digit() || character == '_')
        {
            return false;
        }
        let mut any = false;
        for part in parts {
            any = true;
            if part.is_empty()
                || part.len() > 2
                || !part.chars().all(|character| character.is_ascii_digit())
            {
                return false;
            }
            if part.parse::<u32>().map(|value| value > 59).unwrap_or(true) {
                return false;
            }
        }
        any
    };
    if body.contains(':') {
        return sexagesimal(body);
    }
    body.chars()
        .all(|character| character.is_ascii_digit() || character == '_')
        && !body.starts_with('0')
        && body != "_"
        && body.chars().any(|character| character != '_')
}

/// `[-+]?([0-9][0-9_]*)?\.[0-9_]*([eE][-+][0-9]+)? | [-+]?[0-9][0-9_]*(:[0-5]?[0-9])+\.[0-9_]* | [-+]?\.(inf|Inf|INF) | \.(nan|NaN|NAN)`
fn is_float_11(text: &str) -> bool {
    let body = text
        .strip_prefix('-')
        .or_else(|| text.strip_prefix('+'))
        .unwrap_or(text);
    if matches!(body, ".inf" | ".Inf" | ".INF") {
        return true;
    }
    if matches!(text, ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    if body.contains(':') {
        // Sexagesimal float: `[-+]?[0-9][0-9_]*(:[0-5]?[0-9])+\.[0-9_]*`
        let Some((head, fraction)) = body.rsplit_once('.') else {
            return false;
        };
        let mut parts = head.split(':');
        let Some(first) = parts.next() else {
            return false;
        };
        if first.is_empty()
            || !first
                .chars()
                .all(|character| character.is_ascii_digit() || character == '_')
        {
            return false;
        }
        let mut any = false;
        for part in parts {
            any = true;
            if part.is_empty() || !part.chars().all(|character| character.is_ascii_digit()) {
                return false;
            }
        }
        return any
            && fraction
                .chars()
                .all(|character| character.is_ascii_digit() || character == '_');
    }
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(position) => (&body[..position], Some(&body[position + 1..])),
        None => (body, None),
    };
    let Some((whole, fraction)) = mantissa.split_once('.') else {
        return false;
    };
    let whole_ok = whole
        .chars()
        .all(|character| character.is_ascii_digit() || character == '_');
    let fraction_ok = fraction
        .chars()
        .all(|character| character.is_ascii_digit() || character == '_');
    // `[-+]?([0-9][0-9_]*)?\.[0-9_]*`: both sides may be empty (`.` is a float
    // literal by the resolver, and then `float('.')` is the reference's
    // `ValueError`), so only the digit/_ alphabets are constrained here.
    let mantissa_ok = whole_ok && fraction_ok;
    mantissa_ok && exponent_11_ok(exponent)
}

/// `[eE][-+][0-9]+` — PyYAML's 1.1 float exponent needs an explicit sign.
fn exponent_11_ok(exponent: Option<&str>) -> bool {
    let Some(rest) = exponent else {
        return true;
    };
    let digits = rest.strip_prefix('-').or_else(|| rest.strip_prefix('+'));
    digits.is_some_and(|digits| {
        !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit())
    })
}

/// The reference's YAML 1.2 implicit resolver (ruamel `typ="safe"`).
///
/// `bool` only has the three core spellings (`yes`/`on` are strings in YAML
/// 1.2, then Pydantic's own lax bool accepts them); `int` accepts a leading
/// zero (so `017` is seventeen), binary/octal/hex prefixes and `_` separators;
/// `float` requires a dot or an exponent and only allows `.inf`/`.nan` in their
/// exact core spellings; `null` is `~`/`null`/`Null`/`NULL`/empty.
fn implicit_scalar_kind_yaml12(text: &str) -> &'static str {
    if matches!(text, "true" | "True" | "TRUE" | "false" | "False" | "FALSE") {
        return "bool";
    }
    if is_float_resolver(text) {
        return "float";
    }
    if is_int_resolver(text) {
        return "int";
    }
    if matches!(text, "~" | "null" | "Null" | "NULL" | "") {
        return "null";
    }
    if is_timestamp_resolver(text) {
        return "timestamp";
    }
    "str"
}

fn is_int_resolver(text: &str) -> bool {
    let body = text
        .strip_prefix('-')
        .or_else(|| text.strip_prefix('+'))
        .unwrap_or(text);
    if body.is_empty() {
        return false;
    }
    let digits_all = |rest: &str, allowed: &dyn Fn(char) -> bool| {
        !rest.is_empty()
            && rest
                .chars()
                .all(|character| allowed(character) || character == '_')
    };
    if let Some(rest) = body.strip_prefix("0x") {
        return digits_all(rest, &|character: char| character.is_ascii_hexdigit());
    }
    if let Some(rest) = body.strip_prefix("0b") {
        return digits_all(rest, &|character: char| matches!(character, '0' | '1'));
    }
    if let Some(rest) = body.strip_prefix("0o") {
        return digits_all(rest, &|character: char| matches!(character, '0'..='7'));
    }
    // `[-+]?0o?[0-7_]+` (a leading zero may be followed by optional `o`) or
    // `[-+]?[0-9_]+` (any decimal, leading zeros included).
    if let Some(rest) = body.strip_prefix('0') {
        if let Some(octal) = rest.strip_prefix('o') {
            if digits_all(octal, &|character: char| matches!(character, '0'..='7')) {
                return true;
            }
        } else if digits_all(rest, &|character: char| matches!(character, '0'..='7')) {
            return true;
        }
    }
    digits_all(body, &|character: char| character.is_ascii_digit())
}

fn is_float_resolver(text: &str) -> bool {
    // `[-+]?\.(?:inf|Inf|INF)` / `\.(?:nan|NaN|NAN)` — the sign is optional for
    // the infinities and *not allowed* for the NaN spellings.
    if let Some(body) = text.strip_prefix('-').or_else(|| text.strip_prefix('+')) {
        if matches!(body, ".inf" | ".Inf" | ".INF") {
            return true;
        }
    } else if matches!(text, ".inf" | ".Inf" | ".INF") {
        return true;
    }
    if matches!(text, ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    let body = text
        .strip_prefix('-')
        .or_else(|| text.strip_prefix('+'))
        .unwrap_or(text);
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(position) => (&body[..position], Some(&body[position + 1..])),
        None => (body, None),
    };
    match mantissa.split_once('.') {
        Some((whole, fraction)) => {
            let whole_ok = !whole.is_empty()
                && whole
                    .chars()
                    .all(|character| character.is_ascii_digit() || character == '_');
            let first_ok = whole.starts_with(|character: char| character.is_ascii_digit());
            let fraction_ok = fraction
                .chars()
                .all(|character| character.is_ascii_digit() || character == '_');
            // `[-+]?[0-9][0-9_]*\.[0-9_]*(?:[eE][-+]?[0-9]+)?`
            if whole_ok && first_ok && fraction_ok && exponent_ok(exponent, false) {
                return true;
            }
            // `[-+]?\.[0-9_]+(?:[eE][-+][0-9]+)?` — an exponent needs its sign.
            if whole.is_empty()
                && !fraction.is_empty()
                && fraction_ok
                && exponent_ok(exponent, true)
            {
                return true;
            }
            false
        }
        None => {
            // `[-+]?(?:[0-9][0-9_]*)(?:[eE][-+]?[0-9]+)` — the exponent is
            // mandatory here, and a mantissa without a dot is not a float
            // otherwise (`1e5` matches, plain `1` is an int).
            !mantissa.is_empty()
                && mantissa
                    .chars()
                    .all(|character| character.is_ascii_digit() || character == '_')
                && mantissa.starts_with(|character: char| character.is_ascii_digit())
                && exponent.is_some()
                && exponent_ok(exponent, false)
        }
    }
}

/// The exponent is optional when `sign_required` is false, and always a plain
/// decimal when present (`[0-9]+`, never `_`).
fn exponent_ok(exponent: Option<&str>, sign_required: bool) -> bool {
    let Some(rest) = exponent else {
        return true;
    };
    let digits = if sign_required {
        rest.strip_prefix('-').or_else(|| rest.strip_prefix('+'))
    } else {
        rest.strip_prefix('-')
            .or_else(|| rest.strip_prefix('+'))
            .or(Some(rest))
    };
    digits.is_some_and(|digits| {
        !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit())
    })
}

/// The reference resolver's timestamp pattern (stricter than `!!timestamp`:
/// a date without a time must have two-digit month and day).
pub fn is_timestamp_resolver(text: &str) -> bool {
    let bytes = text.as_bytes();
    let digits = |index: &mut usize, min: usize, max: usize| -> bool {
        let start = *index;
        while *index < bytes.len() && *index - start < max && bytes[*index].is_ascii_digit() {
            *index += 1;
        }
        let length = *index - start;
        (min..=max).contains(&length)
    };
    let expect = |index: &mut usize, byte: u8| -> bool {
        if bytes.get(*index) == Some(&byte) {
            *index += 1;
            true
        } else {
            false
        }
    };
    let mut index = 0usize;
    if !digits(&mut index, 4, 4) || !expect(&mut index, b'-') {
        return false;
    }
    {
        // The date-only alternative requires exactly two month/day digits.
        let mut date_only = index;
        if digits(&mut date_only, 2, 2)
            && expect(&mut date_only, b'-')
            && digits(&mut date_only, 2, 2)
            && date_only == bytes.len()
        {
            return true;
        }
    }
    // Otherwise only the time-bearing alternative can match.
    if !digits(&mut index, 1, 2) || !expect(&mut index, b'-') || !digits(&mut index, 1, 2) {
        return false;
    }
    if index >= bytes.len() {
        return false;
    }
    if bytes[index] == b'T' || bytes[index] == b't' {
        index += 1;
    } else if bytes[index] == b' ' || bytes[index] == b'\t' {
        while bytes.get(index) == Some(&b' ') || bytes.get(index) == Some(&b'\t') {
            index += 1;
        }
    } else {
        return false;
    }
    if !digits(&mut index, 1, 2) || !expect(&mut index, b':') || !digits(&mut index, 2, 2) {
        return false;
    }
    if !expect(&mut index, b':') || !digits(&mut index, 2, 2) {
        return false;
    }
    if expect(&mut index, b'.') {
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
    }
    while bytes.get(index) == Some(&b' ') || bytes.get(index) == Some(&b'\t') {
        index += 1;
    }
    if bytes.get(index) == Some(&b'Z') {
        index += 1;
    } else if matches!(bytes.get(index), Some(b'+') | Some(b'-')) {
        index += 1;
        if !digits(&mut index, 1, 2) {
            return false;
        }
        if expect(&mut index, b':') && !digits(&mut index, 2, 2) {
            return false;
        }
    }
    index == bytes.len()
}
