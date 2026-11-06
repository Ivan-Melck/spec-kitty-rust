//! CPython 3.11.15 / UCD14 `isdigit` members outside the existing Nd authority.
//!
//! Derived from the pinned oracle over all 0..=0x10ffff, not Rust Unicode data.
//! 128 code points, 20 ranges; big-endian u32 member SHA256:
//! 4cf72a4576c5bc5c1c8f8dd173d2b91547b95ad53eb2f8750caea22718b17f1b
//! Source probe: /tmp/sk-cf56-parent-source-console.py, receipt .frNxQt.
//! Decimal members remain solely in python_decimal_data, not duplicated here.

pub(crate) const EXTRA_DIGIT_RANGES: &[(u32, u32)] = &[
    (178, 179),
    (185, 185),
    (4969, 4977),
    (6618, 6618),
    (8304, 8304),
    (8308, 8313),
    (8320, 8329),
    (9312, 9320),
    (9332, 9340),
    (9352, 9360),
    (9450, 9450),
    (9461, 9469),
    (9471, 9471),
    (10102, 10110),
    (10112, 10120),
    (10122, 10130),
    (68160, 68163),
    (69216, 69224),
    (69714, 69722),
    (127232, 127242),
];

#[cfg(test)]
mod tests {
    #[test]
    fn pinned_digit_members_keep_decimal_and_nondecimal_roles_distinct() {
        use crate::python_text::{is_decimal, is_digit};
        assert_eq!((0..=0x10ffff).filter(|code| is_decimal(*code)).count(), 660);
        assert_eq!((0..=0x10ffff).filter(|code| is_digit(*code)).count(), 788);
        assert!(is_digit(0xb2));
        assert!(!is_decimal(0xb2));
        assert!(!is_digit(0x2163));
        assert!(!is_digit(0xd800));
    }
}
