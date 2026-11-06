//! Source-first control for the `requirement_refs` token repr.
//!
//! The expected values below are the reference's own output for
//! `specify_cli.requirement_mapping.grammar.tokenize_refs` on the pinned oracle
//! (CPython 3.11.15, Source `9aed3a80`), captured before this crate's
//! formatter was changed to the canonical Kernel `python_repr`:
//!
//! ```text
//! ['FR-001 FR-002', 'FR-003,FR-004'] -> ['FR-001', 'FR-002', 'FR-003', 'FR-004']
//! ["O'Brien"]                        -> ["O'Brien"]
//! ['say "hi"']                       -> ['say', '"hi"']
//! ['a\nb']                           -> ['a', 'b']
//! ['café']                           -> ['café']
//! [5]                                -> ['<NON_STRING:5>']
//! [None]                             -> ['<NON_STRING:None>']
//! [True]                             -> ['<NON_STRING:True>']
//! [[1, 'x']]                         -> ["<NON_STRING:[1, 'x']>"]
//! [{'k': "v's"}]                     -> ['<NON_STRING:{\'k\': "v\'s"}>']
//! ```
//!
//! The string cases are the ones the review flagged as a divergent raw
//! `'…'` repr; the container cases pin the `str()`/`repr()` interleaving.

use serde_json::json;
use sk_runtime::tokenize_refs;

#[test]
fn token_repr_matches_the_reference_for_strings() {
    assert_eq!(
        tokenize_refs(&json!(["FR-001 FR-002", "FR-003,FR-004"])),
        vec!["FR-001", "FR-002", "FR-003", "FR-004"]
    );
    assert_eq!(tokenize_refs(&json!(["O'Brien"])), vec!["O'Brien"]);
    assert_eq!(tokenize_refs(&json!(["say \"hi\""])), vec!["say", "\"hi\""]);
    assert_eq!(tokenize_refs(&json!(["a\nb"])), vec!["a", "b"]);
    assert_eq!(tokenize_refs(&json!(["café"])), vec!["café"]);
}

#[test]
fn token_repr_matches_the_reference_for_non_strings() {
    assert_eq!(tokenize_refs(&json!([5])), vec!["<NON_STRING:5>"]);
    assert_eq!(tokenize_refs(&json!([null])), vec!["<NON_STRING:None>"]);
    assert_eq!(tokenize_refs(&json!([true])), vec!["<NON_STRING:True>"]);
    assert_eq!(
        tokenize_refs(&json!([[1, "x"]])),
        vec!["<NON_STRING:[1, 'x']>"]
    );
    assert_eq!(
        tokenize_refs(&json!([{"k": "v's"}])),
        vec!["<NON_STRING:{'k': \"v's\"}>"]
    );
}

#[test]
fn nested_string_repr_matches_the_reference() {
    // Only a *non-string* item reaches `repr`; a top-level string is split as a
    // scalar, which is the reference's own behaviour. A string needing both quote
    // kinds is repr'd by the canonical Kernel formatter inside a container:
    // reference `['<NON_STRING:[\'it\\\'s "q"\']>']`.
    assert_eq!(
        tokenize_refs(&json!([["it's \"q\""]])),
        vec!["<NON_STRING:['it\\'s \"q\"']>"]
    );
    assert_eq!(
        tokenize_refs(&json!([{"k": "it's"}])),
        vec!["<NON_STRING:{'k': \"it's\"}>"]
    );
    // A top-level string is split on whitespace, never quoted.
    assert_eq!(
        tokenize_refs(&json!(["it's \"quoted\""])),
        vec!["it's", "\"quoted\""]
    );
}
