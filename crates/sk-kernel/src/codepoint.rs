//! Canonical Python `str` code-point primitive for the Rust workspace.
//!
//! A CPython `str` is a sequence of code points, not bytes or Unicode scalars:
//! `json.loads` produces lone `\uD800`-style surrogate units from escapes and
//! Python preserves them. Rust `String` cannot hold a lone surrogate, so this
//! newtype keeps the exact units. Equality is code-point equality, which is
//! also Python `dict` key equality. This primitive never pairs surrogate units:
//! `[U+D83D, U+DE00]` is distinct from `[U+1F600]`. The legacy JSON reader pairs
//! adjacent surrogate escapes according to its own grammar; other input
//! formats need not do so.
//!
//! This is the workspace's single Python-string authority. The field stays
//! private and every public constructor/append primitive validates
//! `0..=U+10FFFF` (surrogates included, because they are valid CPython code
//! units). `Display` is an escaped *diagnostic* rendering, never an
//! authoritative wire form; [`PythonString::codepoints`] and
//! [`PythonString::repr`] carry the exact data. The canonical CPython `repr`
//! formatter lives beside this type in [`crate::python_repr`].

/// A decoded Python `str`: an exact sequence of code points.
///
/// This is the crate's single Python-string authority and the public typed
/// code-point path for router/profile data that a Python `str` may carry (NUL,
/// non-printable text, lone surrogates): no wrapper, lossy conversion or second
/// string type is introduced beside it.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct PythonString(Vec<u32>);

/// A code unit that cannot be a Python string code point (`> U+10FFFF`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidCodepoint {
    code: u32,
}

impl InvalidCodepoint {
    /// The rejected value.
    pub fn code(self) -> u32 {
        self.code
    }
}

impl std::fmt::Display for InvalidCodepoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "U+{:X} is not a Python string code point (maximum U+10FFFF)",
            self.code
        )
    }
}

impl std::error::Error for InvalidCodepoint {}

impl std::fmt::Display for PythonString {
    /// Escaped diagnostic rendering (non-scalar code units always escape).
    ///
    /// Not a wire form: use [`PythonString::codepoints`] for exact data.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut units = String::with_capacity(self.0.len());
        for &code in &self.0 {
            match char::from_u32(code) {
                Some(character) => units.push(character),
                None => units.push_str(&format!("\\u{code:04x}")),
            }
        }
        formatter.write_str(&units)
    }
}

impl PythonString {
    /// A validated code-point string from explicit code units.
    ///
    /// Every unit must be `0..=U+10FFFF`; surrogate units are accepted because a
    /// CPython `str` may hold them. This is the typed codepoint path the router
    /// uses for request text, ids, hints, keywords and error fields.
    pub fn from_codepoints(codepoints: &[u32]) -> Result<Self, InvalidCodepoint> {
        let mut units = Vec::with_capacity(codepoints.len());
        for &code in codepoints {
            if code > 0x10ffff {
                return Err(InvalidCodepoint { code });
            }
            units.push(code);
        }
        Ok(Self(units))
    }

    /// A validated code-point string that takes ownership of known-valid units.
    ///
    /// The range check is the same as [`PythonString::from_codepoints`], but a
    /// caller that already owns the units (for example a token split from an
    /// existing `PythonString`) avoids copying them. Invalid input is refused
    /// with the first offending unit; it is never accepted on trust.
    pub fn from_units(units: Vec<u32>) -> Result<Self, InvalidCodepoint> {
        match units.iter().copied().find(|&code| code > 0x10ffff) {
            Some(code) => Err(InvalidCodepoint { code }),
            None => Ok(Self(units)),
        }
    }

    /// A code-point string from Rust text (each scalar kept verbatim).
    ///
    /// Infallible: every Unicode scalar is a valid Python string code point.
    /// Use [`PythonString::from_codepoints`] for the exact typed path that also
    /// accepts surrogate code units.
    pub fn from_scalars(text: &str) -> Self {
        let mut string = Self::default();
        string.push_scalars(text);
        string
    }

    /// Append the code points of ASCII/Unicode text.
    ///
    /// Infallible: a Rust `&str` contains only Unicode scalars.
    pub fn push_scalars(&mut self, text: &str) {
        self.0.extend(text.chars().map(u32::from));
    }

    /// Append one Python string code point, including a lone surrogate unit.
    ///
    /// The unit must be `0..=U+10FFFF`. An invalid unit is refused before the
    /// string is changed, so callers never observe a partial append.
    pub fn push_codepoint(&mut self, code: u32) -> Result<(), InvalidCodepoint> {
        if code > 0x10ffff {
            return Err(InvalidCodepoint { code });
        }
        self.0.push(code);
        Ok(())
    }

    /// Append another validated code-point string verbatim.
    ///
    /// Infallible because the source string already passed construction-time
    /// validation; surrogate units are preserved.
    pub fn append(&mut self, other: &Self) {
        self.0.extend_from_slice(&other.0);
    }

    /// The exact code units, in order.
    pub fn codepoints(&self) -> &[u32] {
        &self.0
    }

    /// Whether this string is empty (`str` of length 0).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// CPython `str.upper()` over the exact code units.
    ///
    /// Delegates to the one shared text authority
    /// ([`crate::python_text::upper_codepoints`]) so no second case table exists.
    /// Full 1:N mappings expand, and surrogate units are preserved verbatim
    /// (the pinned interpreter maps every one of them to itself).
    #[must_use]
    pub fn upper(&self) -> Self {
        Self::from_units(crate::python_text::upper_codepoints(&self.0))
            .expect("uppercase mappings of valid code points stay valid code points")
    }

    /// CPython `repr()` through the one canonical formatter.
    ///
    /// Shared with the Op writer path, so an `available` list or a diagnostic
    /// field escapes exactly like the reference instead of inventing a second
    /// quoting rule.
    pub fn repr(&self) -> String {
        crate::python_repr::python_repr_codepoints(&self.0)
    }

    /// The text when every code unit is a Unicode scalar.
    ///
    /// `Err` names the first surrogate unit, so a caller that needs a Rust
    /// `String` never silently drops or replaces authored code points.
    pub fn to_scalar_string(&self) -> Result<String, u32> {
        let mut out = String::with_capacity(self.0.len());
        for &code in &self.0 {
            match char::from_u32(code) {
                Some(character) => out.push(character),
                None => return Err(code),
            }
        }
        Ok(out)
    }
}
