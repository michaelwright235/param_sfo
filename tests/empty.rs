use param_sfo::{ParamSFO, param_sfo};

#[test]
fn empty() {
    let sfo = param_sfo! {}.unwrap();
    let bytes = sfo.to_bytes().unwrap();
    let empty = ParamSFO::from_bytes(bytes).unwrap();
    assert_eq!(empty.entries().len(), 0)
}
