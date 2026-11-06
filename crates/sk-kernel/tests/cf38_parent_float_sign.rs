//! Source Ruamel-safe float magnitude/sign construction, not a parser fork.
use sk_kernel::python_yaml::{Dialect, decode_graph};
#[test]
fn yaml12_negative_infinity_applies_the_sign_exactly_once() {
    for (token, negative) in [("-.inf", true), ("+.inf", false), (".inf", false)] {
        let graph = decode_graph(&format!("value: {token}\n"), Dialect::RuamelSafe12).unwrap();
        let number = graph
            .root_view()
            .lookup("value")
            .unwrap()
            .as_float()
            .unwrap();
        assert!(number.is_infinite());
        assert_eq!(number.is_sign_negative(), negative, "{token}");
    }
    let graph = decode_graph("value: -0.0\n", Dialect::RuamelSafe12).unwrap();
    assert!(
        graph
            .root_view()
            .lookup("value")
            .unwrap()
            .as_float()
            .unwrap()
            .is_sign_negative()
    );
}
