use sk_doctrine::text_budget::{budget_without_warning_line, enforce_token_budget};
use sk_kernel::PythonString;
fn text(s: &str) -> PythonString {
    PythonString::from_scalars(s)
}

#[test]
fn descriptor_defaults_keep_source_substitution_enabled() {
    let section = sk_doctrine::text_budget::RenderedSection {
        section_id: text("s"),
        header: text("H"),
        body: text("body"),
        selector: text("directive:X"),
        ..Default::default()
    };
    assert!(section.substitutable, "Source descriptor default is true");
    let outcome = sk_doctrine::text_budget::apply_token_budget(&[section], 1, None);
    assert_eq!(outcome.notes.len(), 1);
}

#[test]
fn shared_exact_splitlines_preserves_surrogates_and_scalar_behavior() {
    for boundary in [
        0x0a, 0x0b, 0x0c, 0x0d, 0x1c, 0x1d, 0x1e, 0x85, 0x2028, 0x2029,
    ] {
        let units = [0xd800, 0, boundary, 0xdfff, boundary];
        assert_eq!(
            sk_kernel::python_text::splitlines_codepoints(&units),
            vec![&units[..2], &units[3..4]]
        );
    }
    let crlf = [0xd800, 0, 0x0d, 0x0a, 0xdfff, 0x0d, 0x0a];
    assert_eq!(
        sk_kernel::python_text::splitlines_codepoints(&crlf),
        vec![&crlf[..2], &crlf[4..5]]
    );
    for scalar in [
        "",
        "a\r\nb\u{1c}\u{1d}c\u{2028}",
        "x\u{1f}y",
        "\n",
        "\n\n",
        "a\tb",
    ] {
        let units = text(scalar);
        let exact: Vec<Vec<u32>> =
            sk_kernel::python_text::splitlines_codepoints(units.codepoints())
                .iter()
                .map(|line| line.to_vec())
                .collect();
        let finite: Vec<Vec<u32>> = sk_kernel::python_text::splitlines(scalar)
            .iter()
            .map(|line| line.chars().map(u32::from).collect())
            .collect();
        assert_eq!(exact, finite);
    }
}

#[test]
fn minimum_representable_budget_still_substitutes_without_native_overflow() {
    let profile = text("H1\nAAA\n\nH2\nBBB");
    let result = enforce_token_budget(
        &profile,
        &text("implement"),
        &profile,
        &text(""),
        &text(""),
        i128::MIN,
        None,
    );
    let result = result.to_scalar_string().unwrap();
    assert!(result.contains("2 sections substituted"));
    assert!(result.contains(&format!("budget={}", i128::MIN)));
}

#[test]
fn critical_header_without_lf_is_not_a_substitutable_body() {
    for input in [
        "### Terminology Canon",
        "\n\n### Terminology Canon",
        "###\u{00a0}Unknown",
    ] {
        let block = text(input);
        let result = enforce_token_budget(
            &block,
            &text("implement"),
            &text(""),
            &block,
            &text(""),
            1,
            None,
        );
        assert_eq!(
            result, block,
            "Source partition has a heading and empty body: {input:?}"
        );
    }
}

#[test]
fn finite_remainder_boundary_does_not_panic() {
    let error = budget_without_warning_line(1, i128::MIN)
        .expect_err("exact remainder is outside the finite projection");
    assert_eq!(error.budget, i128::MIN);
    assert_eq!(error.count, 1);
    assert!(error.reserved_chars > 2);
    assert_eq!(budget_without_warning_line(0, i128::MIN), Ok(i128::MIN));
    assert_eq!(
        budget_without_warning_line(1, i128::MIN + 1000).unwrap(),
        i128::MIN + 1000
            - (2 + sk_doctrine::text_budget::warning_line(1, i128::MIN + 1000)
                .codepoints()
                .len()) as i128
    );
}
