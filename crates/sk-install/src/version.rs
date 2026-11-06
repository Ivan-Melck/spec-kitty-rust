//! PEP 440 versions and version specifiers, for `requires_core` and upgrade
//! target/version choices.
//!
//! The pack pin contract declares `requires_core` as a PEP 440 version
//! specifier and enforces it with `packaging.specifiers.SpecifierSet`. This
//! module is the native equivalent for the subset the pins and upgrade paths
//! use — every operator and wildcard form — implemented without a Python
//! runtime:
//!
//! - versions: `[N!]N(.N)*[{a|b|rc}N][.postN][.devN][+local]`
//! - specifiers: `===`, `==` (with `.*`), `!=` (with `.*`), `<=`, `>=`, `<`,
//!   `>` and `~=`, joined by commas (a conjunction).
//!
//! Ordering follows PEP 440 (`dev < pre < final < post`, local segments
//! ordered component-wise). `~=` is the compatible-release operator. The
//! `<`/`>` suffix rules match `packaging`: `<V` refuses a pre-release of `V`
//! when `V` is final, and `>V` refuses a post/local release of `V` when `V`
//! does not itself carry one.
//!
//! This is deliberately a *typed, pure* predicate: no I/O, no environment, no
//! fallback to a shell. An unparseable version or specifier is a typed error,
//! never "assume compatible".

use std::cmp::Ordering;
use std::fmt;

/// A parsed PEP 440 version.
#[derive(Clone, Debug)]
pub struct Version {
    epoch: u64,
    release: Vec<u64>,
    raw_release: Vec<u64>,
    release_segments: usize,
    pre: Option<(PreKind, u64)>,
    post: Option<u64>,
    dev: Option<u64>,
    local: Option<Vec<LocalSegment>>,
}

/// The pre-release phase of a version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreKind {
    /// `a` / `alpha`
    Alpha,
    /// `b` / `beta`
    Beta,
    /// `rc` / `c` / `pre` / `preview`
    Rc,
}

impl PreKind {
    fn rank(self) -> u8 {
        match self {
            Self::Alpha => 0,
            Self::Beta => 1,
            Self::Rc => 2,
        }
    }
}

/// One component of a local version label.
#[derive(Clone, Debug, Eq, PartialEq)]
enum LocalSegment {
    Numeric(u64),
    Alphabetic(String),
}

/// A version or specifier parse failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionError {
    value: String,
    reason: String,
}

impl VersionError {
    fn new(value: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            reason: reason.into(),
        }
    }

    /// The offending text.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Why it was rejected.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for VersionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid version {:?}: {}",
            self.value, self.reason
        )
    }
}

impl std::error::Error for VersionError {}

impl Version {
    /// Parse a PEP 440 public version (with an optional local label).
    pub fn parse(text: &str) -> Result<Self, VersionError> {
        let value = text.trim();
        if value.is_empty() {
            return Err(VersionError::new(text, "empty version"));
        }
        let mut rest = value;
        let mut epoch = 0_u64;
        if let Some(index) = rest.find('!') {
            epoch = rest[..index]
                .parse()
                .map_err(|_| VersionError::new(text, "invalid epoch"))?;
            rest = &rest[index + 1..];
        }
        let (public, local) = match rest.find('+') {
            Some(index) => (&rest[..index], Some(&rest[index + 1..])),
            None => (rest, None),
        };
        if public.is_empty() {
            return Err(VersionError::new(text, "missing release segment"));
        }

        // Release is the leading run of numeric dot-separated components.
        let mut release_end = public
            .find(|character: char| !character.is_ascii_digit() && character != '.')
            .unwrap_or(public.len());
        let mut release_text = &public[..release_end];
        // A suffix may be introduced by `.` (`4.0.0.dev1`); that dot is not an
        // empty release segment.
        while let Some(stripped) = release_text.strip_suffix('.') {
            release_text = stripped;
            release_end -= 1;
        }
        if release_text.is_empty() {
            return Err(VersionError::new(text, "missing release segment"));
        }
        let release_segments = release_text.split('.').count();
        let mut raw_release = Vec::new();
        for part in release_text.split('.') {
            if part.is_empty() {
                return Err(VersionError::new(text, "empty release segment"));
            }
            raw_release.push(
                part.parse()
                    .map_err(|_| VersionError::new(text, "invalid release segment"))?,
            );
        }
        let mut release = raw_release.clone();
        while release.len() > 1 && release.last() == Some(&0) {
            release.pop();
        }

        let mut suffix = &public[release_end..];
        let mut pre = None;
        let mut post = None;
        let mut dev = None;

        if !suffix.is_empty() {
            let (kind, consumed) = parse_pre_prefix(suffix)?;
            if let Some(kind) = kind {
                let (number, length) = parse_number(&suffix[consumed..], text)?;
                pre = Some((kind, number));
                suffix = &suffix[consumed + length..];
            }
        }
        if let Some(rest) = suffix.strip_prefix(".post") {
            let (number, length) = parse_number(rest, text)?;
            post = Some(number);
            suffix = &rest[length..];
        } else if let Some(rest) = suffix.strip_prefix("-post") {
            // A hyphenated post is accepted by packaging as well.
            let (number, length) = parse_number(rest, text)?;
            post = Some(number);
            suffix = &rest[length..];
        } else if let Some(rest) = suffix.strip_prefix('-')
            && !rest.is_empty()
            && rest.chars().all(|character| character.is_ascii_digit())
        {
            let (number, length) = parse_number(rest, text)?;
            post = Some(number);
            suffix = &rest[length..];
        }
        if let Some(rest) = suffix.strip_prefix(".dev") {
            let (number, length) = parse_number(rest, text)?;
            dev = Some(number);
            suffix = &rest[length..];
        } else if let Some(rest) = suffix.strip_prefix("-dev") {
            let (number, length) = parse_number(rest, text)?;
            dev = Some(number);
            suffix = &rest[length..];
        }
        if !suffix.is_empty() {
            return Err(VersionError::new(
                text,
                format!("unexpected suffix {suffix:?}"),
            ));
        }

        let local = match local {
            Some(raw) => {
                if raw.is_empty() {
                    return Err(VersionError::new(text, "empty local label"));
                }
                let mut segments = Vec::new();
                for part in raw.split(['.', '-', '_']) {
                    if part.is_empty() {
                        return Err(VersionError::new(text, "empty local segment"));
                    }
                    if part.chars().all(|character| character.is_ascii_digit()) {
                        segments
                            .push(LocalSegment::Numeric(part.parse().map_err(|_| {
                                VersionError::new(text, "local segment too large")
                            })?));
                    } else if part
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric())
                    {
                        segments.push(LocalSegment::Alphabetic(part.to_ascii_lowercase()));
                    } else {
                        return Err(VersionError::new(text, "invalid local segment"));
                    }
                }
                Some(segments)
            }
            None => None,
        };

        Ok(Self {
            epoch,
            release,
            raw_release,
            release_segments,
            pre,
            post,
            dev,
            local,
        })
    }

    /// Whether this is a pre-release (`a`/`b`/`rc`, including a dev release of one).
    pub fn is_prerelease(&self) -> bool {
        self.pre.is_some() || self.dev.is_some()
    }

    /// Whether this carries a post-release segment.
    pub fn is_postrelease(&self) -> bool {
        self.post.is_some()
    }

    /// The public version (epoch, release, pre, post and dev), normalized.
    pub fn public_string(&self) -> String {
        let mut text = String::new();
        if self.epoch != 0 {
            text.push_str(&format!("{}!", self.epoch));
        }
        text.push_str(
            &self
                .raw_release
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join("."),
        );
        if let Some((kind, number)) = &self.pre {
            text.push_str(match kind {
                PreKind::Alpha => "a",
                PreKind::Beta => "b",
                PreKind::Rc => "rc",
            });
            text.push_str(&number.to_string());
        }
        if let Some(number) = self.post {
            text.push_str(&format!(".post{number}"));
        }
        if let Some(number) = self.dev {
            text.push_str(&format!(".dev{number}"));
        }
        text
    }

    /// The release prefix used by `==X.Y.*` matching and `~=`.
    fn release_prefix(&self) -> &[u64] {
        &self.release
    }

    fn without_local(&self) -> Self {
        let mut copy = self.clone();
        copy.local = None;
        copy
    }

    fn comparison_key(&self) -> (u64, Vec<u64>, PreKey, PostKey, DevKey, LocalKey) {
        let pre = match (&self.pre, self.post, self.dev) {
            (None, None, Some(_)) => PreKey::NegativeInfinity,
            (None, _, _) => PreKey::Infinity,
            (Some((kind, number)), _, _) => PreKey::Value(kind.rank(), *number),
        };
        let post = match self.post {
            Some(number) => PostKey::Value(number),
            None => PostKey::NegativeInfinity,
        };
        let dev = match self.dev {
            Some(number) => DevKey::Value(number),
            None => DevKey::Infinity,
        };
        let local = match &self.local {
            Some(segments) => LocalKey::Value(
                segments
                    .iter()
                    .map(|segment| match segment {
                        LocalSegment::Numeric(number) => (1_u8, *number, String::new()),
                        LocalSegment::Alphabetic(text) => (0, 0, text.clone()),
                    })
                    .collect(),
            ),
            None => LocalKey::NegativeInfinity,
        };
        (self.epoch, self.release.clone(), pre, post, dev, local)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum PreKey {
    NegativeInfinity,
    Value(u8, u64),
    Infinity,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum PostKey {
    NegativeInfinity,
    Value(u64),
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum DevKey {
    Value(u64),
    Infinity,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum LocalKey {
    NegativeInfinity,
    Value(Vec<(u8, u64, String)>),
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.comparison_key() == other.comparison_key()
    }
}

impl Eq for Version {}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.comparison_key().cmp(&other.comparison_key())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = self.public_string();
        if let Some(segments) = &self.local {
            text.push('+');
            let rendered: Vec<String> = segments
                .iter()
                .map(|segment| match segment {
                    LocalSegment::Numeric(number) => number.to_string(),
                    LocalSegment::Alphabetic(value) => value.clone(),
                })
                .collect();
            text.push_str(&rendered.join("."));
        }
        formatter.write_str(&text)
    }
}

fn parse_pre_prefix(text: &str) -> Result<(Option<PreKind>, usize), VersionError> {
    for (prefix, kind) in [
        ("alpha", PreKind::Alpha),
        ("beta", PreKind::Beta),
        ("preview", PreKind::Rc),
        ("pre", PreKind::Rc),
        ("rc", PreKind::Rc),
        ("a", PreKind::Alpha),
        ("b", PreKind::Beta),
        ("c", PreKind::Rc),
    ] {
        if let Some(rest) = text.strip_prefix(prefix)
            && (rest.starts_with(|character: char| character.is_ascii_digit())
                || rest.starts_with(['.', '-']))
        {
            let mut offset = prefix.len();
            if rest.starts_with(['.', '-']) {
                offset += 1;
            }
            return Ok((Some(kind), offset));
        }
    }
    Ok((None, 0))
}

fn parse_number(text: &str, whole: &str) -> Result<(u64, usize), VersionError> {
    let trimmed = text.strip_prefix(['.', '-']).unwrap_or(text);
    let consumed_prefix = text.len() - trimmed.len();
    let end = trimmed
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(trimmed.len());
    if end == 0 {
        return Err(VersionError::new(whole, "missing numeric suffix"));
    }
    let number = trimmed[..end]
        .parse()
        .map_err(|_| VersionError::new(whole, "numeric suffix too large"))?;
    Ok((number, consumed_prefix + end))
}

/// One comparison clause of a version specifier.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Operator {
    ArbitraryEqual,
    Equal,
    NotEqual,
    LessThanOrEqual,
    GreaterThanOrEqual,
    LessThan,
    GreaterThan,
    Compatible,
}

/// A single parsed specifier clause.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Clause {
    operator: Operator,
    version: Version,
    /// `==X.Y.*` / `!=X.Y.*`: the release prefix before `.*`.
    wildcard: Option<Vec<u64>>,
    /// Raw text for `===`.
    arbitrary: Option<String>,
}

/// A conjunction of PEP 440 specifier clauses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpecifierSet {
    clauses: Vec<Clause>,
    source: String,
}

impl SpecifierSet {
    /// Parse a comma-separated specifier set.
    pub fn parse(text: &str) -> Result<Self, VersionError> {
        let value = text.trim();
        if value.is_empty() {
            return Err(VersionError::new(text, "empty version specifier"));
        }
        let mut clauses = Vec::new();
        for raw in value.split(',') {
            clauses.push(parse_clause(raw.trim(), text)?);
        }
        if clauses.is_empty() {
            return Err(VersionError::new(text, "empty version specifier"));
        }
        Ok(Self {
            clauses,
            source: value.to_string(),
        })
    }

    /// Whether `candidate` satisfies every clause.
    ///
    /// `prereleases` mirrors `SpecifierSet.contains(..., prereleases=...)`: the
    /// pack contract permits pre-releases (`requires_core >=4.0.0rc7,<5` must
    /// admit `4.0.0rc8`).
    pub fn contains(&self, candidate: &Version, prereleases: bool) -> bool {
        if candidate.is_prerelease() && !prereleases {
            return false;
        }
        self.clauses
            .iter()
            .all(|clause| clause.matches(candidate, prereleases))
    }

    /// Whether a textual version satisfies this set.
    pub fn contains_text(&self, candidate: &str, prereleases: bool) -> Result<bool, VersionError> {
        let parsed = Version::parse(candidate)?;
        Ok(self.contains(&parsed, prereleases))
    }

    /// The original specifier text.
    pub fn as_str(&self) -> &str {
        &self.source
    }
}

impl fmt::Display for SpecifierSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.source)
    }
}

fn parse_clause(raw: &str, whole: &str) -> Result<Clause, VersionError> {
    let (operator, rest) = if let Some(rest) = raw.strip_prefix("===") {
        (Operator::ArbitraryEqual, rest)
    } else if let Some(rest) = raw.strip_prefix("==") {
        (Operator::Equal, rest)
    } else if let Some(rest) = raw.strip_prefix("!=") {
        (Operator::NotEqual, rest)
    } else if let Some(rest) = raw.strip_prefix("<=") {
        (Operator::LessThanOrEqual, rest)
    } else if let Some(rest) = raw.strip_prefix(">=") {
        (Operator::GreaterThanOrEqual, rest)
    } else if let Some(rest) = raw.strip_prefix("~=") {
        (Operator::Compatible, rest)
    } else if let Some(rest) = raw.strip_prefix('<') {
        (Operator::LessThan, rest)
    } else if let Some(rest) = raw.strip_prefix('>') {
        (Operator::GreaterThan, rest)
    } else {
        return Err(VersionError::new(
            whole,
            format!("clause {raw:?} has no comparison operator"),
        ));
    };
    let body = rest.trim();
    if body.is_empty() {
        return Err(VersionError::new(
            whole,
            format!("clause {raw:?} has no version"),
        ));
    }
    if operator == Operator::ArbitraryEqual {
        return Ok(Clause {
            operator,
            version: Version {
                epoch: 0,
                release: vec![0],
                raw_release: vec![0],
                release_segments: 1,
                pre: None,
                post: None,
                dev: None,
                local: None,
            },
            wildcard: None,
            arbitrary: Some(body.to_string()),
        });
    }
    let (body, wildcard) = if let Some(prefix) = body.strip_suffix(".*") {
        if !matches!(operator, Operator::Equal | Operator::NotEqual) {
            return Err(VersionError::new(
                whole,
                format!("clause {raw:?} cannot use a wildcard"),
            ));
        }
        Version::parse(prefix)?;
        // A wildcard prefix is compared literally: `4.0.*` must not lose its
        // trailing zero to release normalization (that would match `4.1`).
        let mut release = Vec::new();
        for part in prefix.split('.') {
            release.push(
                part.parse()
                    .map_err(|_| VersionError::new(whole, "invalid wildcard prefix"))?,
            );
        }
        (prefix, Some(release))
    } else {
        (body, None)
    };
    let version = Version::parse(body)?;
    if let Some(prefix) = &wildcard {
        if version.pre.is_some() || version.post.is_some() || version.dev.is_some() {
            return Err(VersionError::new(
                whole,
                format!("clause {raw:?} has a wildcard after a suffix"),
            ));
        }
        if prefix.is_empty() {
            return Err(VersionError::new(whole, "empty wildcard prefix"));
        }
    }
    if operator == Operator::Compatible && version.release_segments < 2 {
        return Err(VersionError::new(
            whole,
            format!("clause {raw:?} requires at least two release segments"),
        ));
    }
    Ok(Clause {
        operator,
        version,
        wildcard,
        arbitrary: None,
    })
}

impl Clause {
    fn matches(&self, candidate: &Version, prereleases: bool) -> bool {
        match self.operator {
            Operator::ArbitraryEqual => {
                self.arbitrary.as_deref() == Some(candidate.to_string().as_str())
            }
            Operator::Equal => match &self.wildcard {
                Some(prefix) => release_starts_with(candidate.release_prefix(), prefix),
                None => versions_equal_public(candidate, &self.version),
            },
            Operator::NotEqual => match &self.wildcard {
                Some(prefix) => !release_starts_with(candidate.release_prefix(), prefix),
                None => !versions_equal_public(candidate, &self.version),
            },
            Operator::LessThanOrEqual => candidate.without_local() <= self.version.without_local(),
            Operator::GreaterThanOrEqual => {
                candidate.without_local() >= self.version.without_local()
            }
            Operator::LessThan => less_than(candidate, &self.version, prereleases),
            Operator::GreaterThan => greater_than(candidate, &self.version),
            Operator::Compatible => {
                if candidate.without_local() < self.version.without_local() {
                    return false;
                }
                let mut prefix = self.version.raw_release.clone();
                if prefix.len() > 1 {
                    prefix.pop();
                }
                release_starts_with(candidate.release_prefix(), &prefix)
            }
        }
    }
}

fn release_starts_with(release: &[u64], prefix: &[u64]) -> bool {
    if prefix.len() > release.len() {
        // `4.0.0` matches `4.0.0.*`; `4.0` matches `4.0.*` after zero-padding.
        let mut padded = release.to_vec();
        while padded.len() < prefix.len() {
            padded.push(0);
        }
        return padded == prefix;
    }
    release[..prefix.len()] == *prefix
}

fn versions_equal_public(left: &Version, right: &Version) -> bool {
    // A specifier without a local label compares only the public version.
    let left = left.without_local();
    let right = if right.local.is_some() {
        right.clone()
    } else {
        right.without_local()
    };
    left.comparison_key() == right.comparison_key()
}

fn less_than(candidate: &Version, spec: &Version, _prereleases: bool) -> bool {
    let candidate_public = candidate.without_local();
    let spec_public = spec.without_local();
    if candidate_public >= spec_public {
        return false;
    }
    // `<V` does not admit a pre-release of V when V is a final release.
    if !spec.is_prerelease() && candidate.is_prerelease() && same_release(candidate, spec) {
        return false;
    }
    true
}

fn greater_than(candidate: &Version, spec: &Version) -> bool {
    let candidate_public = candidate.without_local();
    let spec_public = spec.without_local();
    if candidate_public <= spec_public {
        return false;
    }
    // `>V` does not admit a post-release of V when V is not a post-release.
    if !spec.is_postrelease() && candidate.is_postrelease() && same_release(candidate, spec) {
        return false;
    }
    // `>V` does not admit a local version of the same public version.
    if spec.local.is_none()
        && candidate.local.is_some()
        && candidate_public.comparison_key() == spec_public.comparison_key()
    {
        return false;
    }
    true
}

fn same_release(left: &Version, right: &Version) -> bool {
    left.epoch == right.epoch && left.release == right.release
}

/// Parse a `requires_core` constraint, or `Ok(None)` when absent.
pub fn parse_requires_core(
    requires_core: Option<&str>,
) -> Result<Option<SpecifierSet>, VersionError> {
    match requires_core {
        None => Ok(None),
        Some(text) => SpecifierSet::parse(text).map(Some),
    }
}

/// Whether `core_version` satisfies `requires_core`.
///
/// An absent constraint means no constraint. Pre-releases are admitted, which
/// is the contract the vendoring path enforces (`prereleases=True`): the
/// canonical pin is `>=4.0.0rc7,<5` against a release-candidate core.
pub fn check_requires_core(
    requires_core: Option<&str>,
    core_version: &str,
) -> Result<bool, VersionError> {
    let Some(specifier) = parse_requires_core(requires_core)? else {
        return Ok(true);
    };
    specifier.contains_text(core_version, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|error| panic!("{text}: {error}"))
    }

    #[test]
    fn parses_and_orders_pep440_versions() {
        assert!(version("4.0.0.dev1") < version("4.0.0rc7"));
        assert!(version("4.0.0rc7") < version("4.0.0rc8"));
        assert!(version("4.0.0rc8") < version("4.0.0"));
        assert!(version("4.0.0") < version("4.0.0.post1"));
        assert!(version("4.0.0+abc") < version("4.0.0+abd"));
        assert_eq!(version("4.0"), version("4.0.0"));
        assert!(version("1!1.0") > version("2.0"));
        assert_eq!(version("4.0.0c1"), version("4.0.0rc1"));
        assert_eq!(version("4.0.0alpha2"), version("4.0.0a2"));
    }

    #[test]
    fn rejects_malformed_versions() {
        for text in ["", "4..0", "4.0.0rc", "4.0.0nonsense", "4.0.0+", "!4.0"] {
            assert!(Version::parse(text).is_err(), "{text:?} should be rejected");
        }
    }

    #[test]
    fn canonical_requires_core_admits_the_release_candidate_core() {
        let specifier = SpecifierSet::parse(">=4.0.0rc7,<5").expect("specifier");
        assert!(specifier.contains(&version("4.0.0rc8"), true));
        assert!(specifier.contains(&version("4.0.0"), true));
        assert!(specifier.contains(&version("4.9.9"), true));
        assert!(!specifier.contains(&version("5.0.0"), true));
        assert!(!specifier.contains(&version("3.9.0"), true));
        assert_eq!(
            check_requires_core(Some(">=4.0.0rc7,<5"), "4.0.0rc8"),
            Ok(true)
        );
        assert_eq!(check_requires_core(None, "0.0.1"), Ok(true));
    }

    #[test]
    fn wildcards_compatible_release_and_suffix_rules() {
        let wildcard = SpecifierSet::parse("==4.0.*").expect("wildcard");
        assert!(wildcard.contains(&version("4.0.1"), true));
        assert!(!wildcard.contains(&version("4.1.0"), true));

        let compatible = SpecifierSet::parse("~=4.0.0rc7").expect("compatible");
        assert!(compatible.contains(&version("4.0.0rc8"), true));
        assert!(compatible.contains(&version("4.0.9"), true));
        assert!(!compatible.contains(&version("4.1.0"), true));

        let less = SpecifierSet::parse("<4.0.0").expect("less");
        assert!(!less.contains(&version("4.0.0rc1"), true));
        assert!(less.contains(&version("3.9.0"), true));

        let greater = SpecifierSet::parse(">4.0.0").expect("greater");
        assert!(!greater.contains(&version("4.0.0.post1"), true));
        assert!(greater.contains(&version("4.0.1"), true));

        let not_equal = SpecifierSet::parse("!=4.0.0").expect("not equal");
        assert!(!not_equal.contains(&version("4.0"), true));
        assert!(not_equal.contains(&version("4.0.1"), true));

        let arbitrary = SpecifierSet::parse("===4.0.0rc8").expect("arbitrary");
        assert!(arbitrary.contains(&version("4.0.0rc8"), true));
        assert!(!arbitrary.contains(&version("4.0.0"), true));
    }

    #[test]
    fn rejects_bad_specifiers() {
        for text in ["", "4.0.0", ">=4.0.0,,<5", "~=4", ">=4.0.*", "==4.0.0rc1.*"] {
            assert!(
                SpecifierSet::parse(text).is_err(),
                "{text:?} should be rejected"
            );
        }
    }

    #[test]
    fn prerelease_policy_is_explicit() {
        let specifier = SpecifierSet::parse(">=4.0.0").expect("specifier");
        assert!(!specifier.contains(&version("4.0.1rc1"), false));
        assert!(specifier.contains(&version("4.0.1rc1"), true));
    }
}
