use sk_kernel::python_json::{parse, python_json_le};

#[test]
fn direct_nan_relational_comparison_does_not_use_container_identity() {
    let left = parse("NaN").unwrap();
    let right = parse("NaN").unwrap();
    assert!(!python_json_le(&left, &right).unwrap());
    assert!(python_json_le(&parse("[NaN]").unwrap(), &parse("[NaN]").unwrap()).unwrap());
    assert!(python_json_le(&parse("Infinity").unwrap(), &parse("Infinity").unwrap()).unwrap());
    assert!(python_json_le(&parse("true").unwrap(), &parse("1").unwrap()).unwrap());
    assert!(python_json_le(&parse("null").unwrap(), &parse("null").unwrap()).is_err());
}
