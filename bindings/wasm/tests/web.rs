use wasm_bindgen_test::*;
wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn test_compile() {
    let source = "wire osc = cycle~(440);\nout audio: signal = osc;\n";
    let result = flutmax_wasm::compile(source);
    assert!(result.is_ok(), "compile failed: {:?}", result.err());
    let json = result.unwrap();
    assert!(json.contains("cycle~"));
}

#[wasm_bindgen_test]
fn test_compile_error() {
    let result = flutmax_wasm::compile("wire osc = ;");
    assert!(result.is_err());
}

#[wasm_bindgen_test]
fn test_parse() {
    let source = "in freq: float;\nwire osc = cycle~(freq);\nout audio: signal = osc;\n";
    let result = flutmax_wasm::parse(source);
    assert!(result.is_ok());
    let json = result.unwrap();
    assert!(json.contains("wires"));
}

#[wasm_bindgen_test]
fn test_version() {
    assert_eq!(flutmax_wasm::version(), "0.1.0");
}
