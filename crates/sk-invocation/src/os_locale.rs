//! Read-only native OS locale availability and codeset facts.
//!
//! The Op encoder must not decide a filesystem charset from the *text* of a
//! locale name: `en_US.UTF-8` may be unavailable, `en_US` may actually be
//! ISO-8859-1, and an unknown suffix proves nothing. This module is the one
//! bounded, read-only authority for what the host actually has.
//!
//! On Linux/glibc it resolves a requested POSIX locale name the way libc does
//! for the `LC_CTYPE` category: expand `locale.alias`, search `LOCPATH`
//! directories before `/usr/lib/locale`, try the literal name, the normalized
//! codeset spelling (`en_US.UTF-8` → `en_US.utf8`), and finally the
//! language/territory base, accepting the base only when its actual
//! `LC_CTYPE` codeset is the requested codeset (equal name or documented
//! charmap alias). The codeset is read from the glibc `LC_CTYPE` file header:
//! native-endian (or byte-swapped) magic `0x20090720`, string count, `u32`
//! offset array from byte 8, and slot 14 (`_NL_CTYPE_CODESET_NAME`).
//!
//! The fact is deliberately three-valued beyond "resolved": a locale that is
//! genuinely absent is [`LocaleFact::Unavailable`] (libc stays in the C
//! locale), while unreadable data, an unsupported file format, a locale
//! archive, a path-shaped locale name, or a codeset relation this module
//! cannot decide is [`LocaleFact::Uncertain`]. Uncertainty is never converted
//! into a C-locale guess; the encoder refuses it as a native capability gap.
//!
//! No locale data is written, no environment variable is mutated, no libc
//! symbol is called, and no Python process is executed. `locale-archive`
//! reading and non-glibc platforms are genuine remaining gaps, not source
//! verdicts.

use std::ffi::OsString;

/// What the host actually provides for one requested locale name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LocaleFact {
    /// The actual C/POSIX locale (libc's default, also after a failed lookup).
    C,
    /// Locale data was read and reports this filesystem codeset.
    Resolved {
        /// The codeset name stored in the locale's `LC_CTYPE` data.
        codeset: String,
    },
    /// No locale data for the name: libc's `setlocale` would fail and stay C.
    Unavailable,
    /// The name cannot be decided truthfully from readable native data.
    Uncertain {
        /// Why this adapter cannot decide, for the operational refusal.
        detail: String,
    },
}

/// Resolve `requested` against the host's real locale data.
///
/// `lookup` is the read-only environment seam (`LOCPATH` is read here); the
/// caller's startup policy (`PYTHONUTF8`, coercion, precedence) is not this
/// module's concern.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) fn resolve(
    lookup: &impl Fn(&str) -> Option<OsString>,
    requested: Option<&str>,
) -> LocaleFact {
    glibc::resolve(lookup, requested)
}

/// Non-glibc Unix: locale data formats are not read here.
///
/// Returning "resolved" would be a guess and returning "unavailable" would
/// falsely claim the source falls back to C, so a non-C name is a truthful
/// native uncertainty (operational refusal). The C/POSIX locale needs no data.
#[cfg(all(unix, not(all(target_os = "linux", target_env = "gnu"))))]
pub(crate) fn resolve(
    lookup: &impl Fn(&str) -> Option<OsString>,
    requested: Option<&str>,
) -> LocaleFact {
    let _ = lookup;
    match requested {
        None | Some("C" | "POSIX") => LocaleFact::C,
        Some(name) => LocaleFact::Uncertain {
            detail: format!(
                "native locale data for `{name}` is not readable on this non-glibc platform"
            ),
        },
    }
}

/// Non-Unix platforms are outside this authority.
#[cfg(not(unix))]
pub(crate) fn resolve(
    lookup: &impl Fn(&str) -> Option<OsString>,
    requested: Option<&str>,
) -> LocaleFact {
    let _ = (lookup, requested);
    LocaleFact::Uncertain {
        detail: "native locale facts are not implemented on this platform".to_owned(),
    }
}

/// Canonical identity of a locale codeset spelling.
///
/// The `ISO-8859-N` family accepts the spellings observed on this host's
/// glibc (`ISO-8859-N`, `ISO8859-N`, `ISO8859N`, plus the documented
/// `ISO_8859-1`/`ISO_8859-1:1987` aliases for Latin-1). Other spellings are
/// not guessed: `None` means "not one of the spellings this adapter knows".
pub(crate) fn canonical_codeset(codeset: &str) -> Option<String> {
    let lower = codeset.to_ascii_lowercase();
    let canonical = match lower.as_str() {
        "utf-8" | "utf8" => "utf-8",
        "iso-8859-1" | "iso8859-1" | "iso88591" | "iso_8859-1" | "iso_8859-1:1987" | "latin1"
        | "l1" | "cp819" | "ibm819" | "iso-ir-100" | "csisolatin1" => "iso-8859-1",
        "ansi_x3.4-1968" | "ansi_x3.4-1986" | "us-ascii" | "ascii" | "iso646-us" | "cp367"
        | "ibm367" | "us" | "iso-ir-6" | "csascii" => "ascii",
        _ => {
            let digits = lower
                .strip_prefix("iso-8859-")
                .or_else(|| lower.strip_prefix("iso8859-"))
                .or_else(|| lower.strip_prefix("iso8859"))?;
            let number: u8 = digits.parse().ok()?;
            if !(1..=16).contains(&number) {
                return None;
            }
            return Some(format!("iso-8859-{number}"));
        }
    };
    Some(canonical.to_owned())
}

/// Lowercase ASCII alphanumerics only (locale directory spellings are ASCII).
fn normalized(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

/// The Linux/glibc implementation of [`resolve`].
#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod glibc {
    use std::ffi::{OsStr, OsString};
    use std::fs::{self, File};
    use std::io::{ErrorKind, Read, Seek, SeekFrom};
    use std::path::PathBuf;

    use super::{LocaleFact, canonical_codeset, normalized};

    /// The default compiled locale root on this host.
    const LOCALE_ROOT: &str = "/usr/lib/locale";
    /// The alias files glibc consults (this host's `/usr/share/locale/locale.alias`).
    const ALIAS_DIRS: [&str; 2] = ["/usr/local/share/locale", "/usr/share/locale"];
    /// `_nl_file_header.magic`.
    const MAGIC: u32 = 0x2009_0720;
    /// `/usr/include/langinfo.h`: `_NL_CTYPE_CODESET_NAME`.
    const CODESET_SLOT: u64 = 14;
    /// Defensive bound for a plausible string count; the real files have 89.
    const MAX_STRINGS: u32 = 100_000;
    /// A codeset name is short; never read the whole locale file for it.
    const MAX_CODESET_BYTES: u64 = 128;
    /// Alias expansion is bounded so a cyclic alias file cannot loop.
    const MAX_ALIAS_DEPTH: usize = 8;

    /// What the lookup of one candidate locale directory found.
    enum Lookup {
        Found(String),
        Missing,
        Uncertain(String),
    }

    /// The relation between the requested and the actual codeset.
    enum CodesetRelation {
        Equal,
        Alias,
        Mismatch,
        Unknown,
    }

    /// A parsed POSIX locale name.
    struct LocaleName<'a> {
        base: &'a str,
        codeset: Option<&'a str>,
        modifier: Option<&'a str>,
    }

    pub(super) fn resolve(
        lookup: &impl Fn(&str) -> Option<OsString>,
        requested: Option<&str>,
    ) -> LocaleFact {
        let Some(requested) = requested else {
            return LocaleFact::C;
        };
        let directories = alias_directories();
        let expanded = expand_alias_in(&directories, requested);
        let name = expanded.as_deref().unwrap_or(requested);
        if name == "C" || name == "POSIX" {
            return LocaleFact::C;
        }
        if name.is_empty() {
            return LocaleFact::Unavailable;
        }
        if name.contains('/') || name == "." || name == ".." {
            return LocaleFact::Uncertain {
                detail: format!("the locale name `{name}` is a path, not a locale identifier"),
            };
        }

        let roots = roots(lookup);
        let archived = roots
            .iter()
            .any(|root| root.join("locale-archive").is_file());
        let parsed = split_name(name);
        for candidate in candidates(&parsed) {
            match locate(&roots, &candidate) {
                Lookup::Found(actual) => return resolved(actual, parsed.codeset),
                Lookup::Missing => {}
                Lookup::Uncertain(detail) => return LocaleFact::Uncertain { detail },
            }
        }
        if archived {
            LocaleFact::Uncertain {
                detail: format!(
                    "the requested locale `{name}` is not present as individual LC_CTYPE data \
                     and this host also carries an unsupported locale-archive"
                ),
            }
        } else {
            LocaleFact::Unavailable
        }
    }

    /// Accept a located locale only when its actual codeset matches the request.
    fn resolved(actual: String, requested: Option<&str>) -> LocaleFact {
        let Some(requested) = requested else {
            return LocaleFact::Resolved { codeset: actual };
        };
        match relation(&actual, requested) {
            CodesetRelation::Equal | CodesetRelation::Alias => {
                LocaleFact::Resolved { codeset: actual }
            }
            // glibc rejects `en_US.ISO-8859-2`-style mismatches instead of
            // loading the base locale, so libc really stays in C here.
            CodesetRelation::Mismatch => LocaleFact::Unavailable,
            CodesetRelation::Unknown => LocaleFact::Uncertain {
                detail: format!(
                    "locale data reports charset `{actual}`, and this adapter cannot decide \
                     whether the requested charset `{requested}` is an alias of it"
                ),
            },
        }
    }

    fn relation(actual: &str, requested: &str) -> CodesetRelation {
        if actual.eq_ignore_ascii_case(requested) {
            return CodesetRelation::Equal;
        }
        match (canonical_codeset(actual), canonical_codeset(requested)) {
            (Some(actual), Some(requested)) if actual == requested => CodesetRelation::Alias,
            // The implemented codesets' alias sets are bounded and known, so
            // any other spelling really is a mismatch for glibc.
            (Some(_), _) => CodesetRelation::Mismatch,
            (None, _) => CodesetRelation::Unknown,
        }
    }

    /// The candidate directory names, in libc's fallback order.
    fn candidates(name: &LocaleName<'_>) -> Vec<String> {
        let modifier = name
            .modifier
            .map(|modifier| format!("@{modifier}"))
            .unwrap_or_default();
        let normalized_codeset = name.codeset.map(normalized);
        let mut out: Vec<String> = Vec::new();
        let mut push = |value: String| {
            if !value.is_empty() && !out.contains(&value) {
                out.push(value);
            }
        };
        push(match name.codeset {
            Some(codeset) => format!("{}.{codeset}{modifier}", name.base),
            None => format!("{}{modifier}", name.base),
        });
        if let Some(codeset) = &normalized_codeset {
            push(format!("{}.{codeset}{modifier}", name.base));
        }
        if name.codeset.is_some() {
            push(format!("{}{modifier}", name.base));
        }
        if name.modifier.is_some() {
            if let Some(codeset) = name.codeset {
                push(format!("{}.{codeset}", name.base));
            }
            if let Some(codeset) = &normalized_codeset {
                push(format!("{}.{codeset}", name.base));
            }
        }
        if name.codeset.is_some() || name.modifier.is_some() {
            push(name.base.to_owned());
        }
        out
    }

    fn split_name(name: &str) -> LocaleName<'_> {
        let (without_modifier, modifier) = match name.split_once('@') {
            Some((name, modifier)) => (name, Some(modifier)),
            None => (name, None),
        };
        let (base, codeset) = match without_modifier.split_once('.') {
            Some((base, codeset)) => (base, Some(codeset)),
            None => (without_modifier, None),
        };
        LocaleName {
            base,
            codeset: codeset.filter(|codeset| !codeset.is_empty()),
            modifier: modifier.filter(|modifier| !modifier.is_empty()),
        }
    }

    /// `LOCPATH` entries first (if set), then the compiled default root.
    fn roots(lookup: &impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
        use std::os::unix::ffi::OsStrExt;

        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(raw) = lookup("LOCPATH").filter(|raw| !raw.is_empty()) {
            for part in raw.as_bytes().split(|byte| *byte == b':') {
                if !part.is_empty() {
                    let path = PathBuf::from(OsStr::from_bytes(part));
                    if !roots.contains(&path) {
                        roots.push(path);
                    }
                }
            }
        }
        let default = PathBuf::from(LOCALE_ROOT);
        if !roots.contains(&default) {
            roots.push(default);
        }
        roots
    }

    /// Read one candidate's `LC_CTYPE` codeset from the first root that has it.
    fn locate(roots: &[PathBuf], name: &str) -> Lookup {
        for root in roots {
            let directory = root.join(name);
            match fs::metadata(&directory) {
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => {
                    return Lookup::Uncertain(format!(
                        "cannot inspect locale directory {}: {error}",
                        directory.display()
                    ));
                }
                Ok(metadata) if !metadata.is_dir() => continue,
                Ok(_) => {}
            }
            let path = directory.join("LC_CTYPE");
            match File::open(&path) {
                // An incomplete directory does not shadow a later root.
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => {
                    return Lookup::Uncertain(format!(
                        "cannot read locale data {}: {error}",
                        path.display()
                    ));
                }
                Ok(mut file) => {
                    return match read_codeset(&mut file) {
                        Ok(codeset) => Lookup::Found(codeset),
                        Err(detail) => Lookup::Uncertain(format!("{}: {detail}", path.display())),
                    };
                }
            }
        }
        Lookup::Missing
    }

    /// Parse the glibc `LC_CTYPE` header and read slot 14's codeset string.
    fn read_codeset(file: &mut File) -> Result<String, String> {
        let length = file
            .metadata()
            .map_err(|error| format!("cannot inspect LC_CTYPE data: {error}"))?
            .len();
        if length < 8 {
            return Err("the LC_CTYPE data is shorter than its header".to_owned());
        }
        let mut header = [0u8; 8];
        file.read_exact(&mut header)
            .map_err(|error| format!("cannot read the LC_CTYPE header: {error}"))?;
        let magic = u32::from_ne_bytes([header[0], header[1], header[2], header[3]]);
        let swapped = if magic == MAGIC {
            false
        } else if u32::from_ne_bytes([header[3], header[2], header[1], header[0]]) == MAGIC {
            true
        } else {
            return Err("the LC_CTYPE data has an unsupported header magic".to_owned());
        };
        let strings = read_u32([header[4], header[5], header[6], header[7]], swapped);
        if u64::from(strings) <= CODESET_SLOT || strings > MAX_STRINGS {
            return Err(format!(
                "the LC_CTYPE data has an unsupported string count ({strings})"
            ));
        }
        let slot = 8 + CODESET_SLOT * 4;
        if slot + 4 > length {
            return Err("the LC_CTYPE data has no codeset slot".to_owned());
        }
        file.seek(SeekFrom::Start(slot))
            .map_err(|error| format!("cannot seek the LC_CTYPE offsets: {error}"))?;
        let mut raw = [0u8; 4];
        file.read_exact(&mut raw)
            .map_err(|error| format!("cannot read the LC_CTYPE codeset offset: {error}"))?;
        let offset = u64::from(read_u32(raw, swapped));
        if offset >= length {
            return Err("the LC_CTYPE codeset offset is outside the data".to_owned());
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| format!("cannot seek the LC_CTYPE codeset: {error}"))?;
        let available = usize::try_from((length - offset).min(MAX_CODESET_BYTES))
            .map_err(|_| "the LC_CTYPE codeset window is too large".to_owned())?;
        let mut buffer = vec![0u8; available];
        file.read_exact(&mut buffer)
            .map_err(|error| format!("cannot read the LC_CTYPE codeset: {error}"))?;
        let end = buffer
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| "the LC_CTYPE codeset name is not NUL-terminated".to_owned())?;
        let bytes = &buffer[..end];
        if bytes.is_empty() {
            return Err("the LC_CTYPE codeset name is empty".to_owned());
        }
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| "the LC_CTYPE codeset name is not ASCII text".to_owned())
    }

    fn read_u32(bytes: [u8; 4], swapped: bool) -> u32 {
        if swapped {
            u32::from_ne_bytes([bytes[3], bytes[2], bytes[1], bytes[0]])
        } else {
            u32::from_ne_bytes(bytes)
        }
    }

    fn alias_directories() -> Vec<PathBuf> {
        ALIAS_DIRS.iter().map(PathBuf::from).collect()
    }

    /// `locale.alias` expansion, bounded like libc's repeated lookup.
    fn expand_alias_in(directories: &[PathBuf], name: &str) -> Option<String> {
        let mut current = name.to_owned();
        for _ in 0..MAX_ALIAS_DEPTH {
            match alias_lookup_in(directories, &current) {
                Some(next) if next != current => current = next,
                _ => break,
            }
        }
        (current != name).then_some(current)
    }

    fn alias_lookup_in(directories: &[PathBuf], name: &str) -> Option<String> {
        for directory in directories {
            let path = directory.join("locale.alias");
            let Ok(contents) = fs::read_to_string(&path) else {
                continue;
            };
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let mut fields = line.split_whitespace();
                let (Some(alias), Some(value)) = (fields.next(), fields.next()) else {
                    continue;
                };
                if alias.eq_ignore_ascii_case(name) {
                    return Some(value.to_owned());
                }
            }
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Write;

        fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
            let owned: Vec<(String, String)> = pairs
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect();
            move |name| {
                owned
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| OsString::from(value.clone()))
            }
        }

        /// A minimal glibc-shaped `LC_CTYPE` file with slot 14 set.
        fn ctype_bytes(codeset: &str, swapped: bool) -> Vec<u8> {
            let strings = 89u32;
            let slot = 8 + CODESET_SLOT * 4;
            let codeset_offset = slot + (u64::from(strings) - CODESET_SLOT) * 4;
            let mut bytes = Vec::new();
            let mut append = |value: u32| {
                let mut raw = value.to_ne_bytes();
                if swapped {
                    raw.reverse();
                }
                bytes.extend_from_slice(&raw);
            };
            append(MAGIC);
            append(strings);
            for index in 0..strings {
                append(if u64::from(index) == CODESET_SLOT {
                    u32::try_from(codeset_offset).expect("fixture offset fits")
                } else {
                    0
                });
            }
            assert_eq!(bytes.len() as u64, codeset_offset);
            bytes.extend_from_slice(codeset.as_bytes());
            bytes.push(0);
            bytes
        }

        fn read_fixture(bytes: &[u8]) -> Result<String, String> {
            let mut file = tempfile::tempfile().expect("temporary LC_CTYPE fixture");
            file.write_all(bytes).expect("write fixture");
            file.seek(SeekFrom::Start(0)).expect("rewind fixture");
            read_codeset(&mut file)
        }

        #[test]
        fn header_slot_14_is_read_in_both_byte_orders() {
            for swapped in [false, true] {
                let bytes = ctype_bytes("ISO-8859-1", swapped);
                assert_eq!(read_fixture(&bytes), Ok("ISO-8859-1".to_owned()));
            }
        }

        #[test]
        fn unreadable_formats_are_refused_not_guessed() {
            assert!(read_fixture(b"short").is_err());
            let mut bad_magic = ctype_bytes("UTF-8", false);
            bad_magic[..4].copy_from_slice(&[0, 0, 0, 0]);
            assert!(
                read_fixture(&bad_magic)
                    .expect_err("bad magic")
                    .contains("unsupported header magic")
            );
            let mut short_count = ctype_bytes("UTF-8", false);
            short_count[4..8].copy_from_slice(&1u32.to_ne_bytes());
            assert!(
                read_fixture(&short_count)
                    .expect_err("short count")
                    .contains("string count")
            );
            let mut outside = ctype_bytes("UTF-8", false);
            let slot = 8 + (CODESET_SLOT * 4) as usize;
            outside[slot..slot + 4].copy_from_slice(&u32::MAX.to_ne_bytes());
            assert!(
                read_fixture(&outside)
                    .expect_err("offset outside")
                    .contains("outside the data")
            );
        }

        #[test]
        fn names_split_and_fall_back_in_libc_order() {
            let parsed = split_name("de_DE.UTF-8@euro");
            assert_eq!(parsed.base, "de_DE");
            assert_eq!(parsed.codeset, Some("UTF-8"));
            assert_eq!(parsed.modifier, Some("euro"));
            assert_eq!(
                candidates(&parsed),
                vec![
                    "de_DE.UTF-8@euro".to_owned(),
                    "de_DE.utf8@euro".to_owned(),
                    "de_DE@euro".to_owned(),
                    "de_DE.UTF-8".to_owned(),
                    "de_DE.utf8".to_owned(),
                    "de_DE".to_owned(),
                ]
            );
            let plain = split_name("en_US.ISO-8859-1");
            assert_eq!(
                candidates(&plain),
                vec![
                    "en_US.ISO-8859-1".to_owned(),
                    "en_US.iso88591".to_owned(),
                    "en_US".to_owned(),
                ]
            );
        }

        #[test]
        fn documented_glibc_codeset_aliases_match_only_the_observed_set() {
            for codeset in [
                "ISO-8859-1",
                "ISO8859-1",
                "ISO88591",
                "ISO_8859-1",
                "ISO_8859-1:1987",
                "LATIN1",
                "L1",
                "CP819",
                "IBM819",
                "ISO-IR-100",
                "CSISOLATIN1",
            ] {
                assert_eq!(
                    canonical_codeset(codeset).as_deref(),
                    Some("iso-8859-1"),
                    "{codeset} is an observed ISO-8859-1 alias"
                );
            }
            for codeset in ["ISO8859_1", "LATIN-1", "latin_1", "819", "8859-1"] {
                assert_eq!(
                    canonical_codeset(codeset),
                    None,
                    "{codeset} was rejected by this glibc"
                );
            }
            assert_eq!(canonical_codeset("UTF-8").as_deref(), Some("utf-8"));
            assert_eq!(canonical_codeset("utf8").as_deref(), Some("utf-8"));
            assert_eq!(canonical_codeset("UTF_8"), None);
            assert_eq!(
                canonical_codeset("ISO-8859-15").as_deref(),
                Some("iso-8859-15")
            );
            assert_eq!(
                canonical_codeset("ANSI_X3.4-1968").as_deref(),
                Some("ascii")
            );
            assert_eq!(canonical_codeset("US-ASCII").as_deref(), Some("ascii"));
        }

        #[test]
        fn alias_file_expansion_is_bounded_and_case_insensitive() {
            let directory = tempfile::tempdir().expect("alias fixture");
            fs::write(
                directory.path().join("locale.alias"),
                "# comment\n\nGerman\tde_DE.ISO-8859-1\nself self\nchain1 chain2\nchain2 chain3\n",
            )
            .expect("write alias fixture");
            let directories = vec![directory.path().to_path_buf()];
            assert_eq!(
                alias_lookup_in(&directories, "GERMAN"),
                Some("de_DE.ISO-8859-1".to_owned())
            );
            assert_eq!(
                expand_alias_in(&directories, "german"),
                Some("de_DE.ISO-8859-1".to_owned())
            );
            // Chained aliases expand repeatedly, and a self-alias terminates
            // immediately instead of looping.
            assert_eq!(
                expand_alias_in(&directories, "chain1"),
                Some("chain3".to_owned())
            );
            assert_eq!(expand_alias_in(&directories, "self"), None);
            assert_eq!(alias_lookup_in(&directories, "unknown"), None);
        }

        #[test]
        fn locpath_fixture_reads_real_host_ctype_data_and_matches_codesets() {
            let directory = tempfile::tempdir().expect("LOCPATH fixture");
            let locale_directory = directory.path().join("zz_ZZ");
            fs::create_dir_all(&locale_directory).expect("create locale directory");
            let host = [
                "/usr/lib/locale/C.utf8/LC_CTYPE",
                "/usr/lib/locale/C.UTF-8/LC_CTYPE",
            ]
            .iter()
            .map(std::path::Path::new)
            .find(|path| path.is_file())
            .expect("this glibc host has actual C.utf8 LC_CTYPE data");
            fs::copy(host, locale_directory.join("LC_CTYPE")).expect("copy actual LC_CTYPE");
            let locpath = directory.path().to_str().expect("UTF-8 fixture path");
            let pairs = [("LOCPATH", locpath)];
            let environment = lookup(&pairs);
            for name in ["zz_ZZ", "zz_ZZ.UTF-8", "zz_ZZ.UTF8"] {
                assert_eq!(
                    resolve(&environment, Some(name)),
                    LocaleFact::Resolved {
                        codeset: "UTF-8".to_owned()
                    },
                    "{name} resolves through LOCPATH"
                );
            }
            // The fixture's actual codeset is UTF-8: a Latin-1 suffix must not
            // be accepted from the base directory.
            assert_eq!(
                resolve(&environment, Some("zz_ZZ.LATIN1")),
                LocaleFact::Unavailable
            );
            assert_eq!(
                resolve(&environment, Some("zz_ZZ.BOGUS-8")),
                LocaleFact::Unavailable
            );
        }

        #[test]
        fn c_and_posix_names_need_no_data() {
            let empty = lookup(&[]);
            assert_eq!(resolve(&empty, None), LocaleFact::C);
            assert_eq!(resolve(&empty, Some("C")), LocaleFact::C);
            assert_eq!(resolve(&empty, Some("POSIX")), LocaleFact::C);
            assert_eq!(
                resolve(&empty, Some("xx_YY.BOGUS-8")),
                LocaleFact::Unavailable
            );
        }

        #[test]
        fn path_shaped_names_are_uncertain_not_looked_up() {
            let empty = lookup(&[]);
            assert!(matches!(
                resolve(&empty, Some("../en_US")),
                LocaleFact::Uncertain { detail } if detail.contains("path")
            ));
        }
    }
}
