use wasm_bindgen::prelude::*;

/// Compile .flutmax source to .maxpat JSON string.
#[wasm_bindgen]
pub fn compile(source: &str) -> Result<String, JsValue> {
    flutmax::compile(source).map_err(|e| JsValue::from_str(&e))
}

/// Decompile .maxpat JSON string to .flutmax source.
#[wasm_bindgen]
pub fn decompile(maxpat_json: &str) -> Result<String, JsValue> {
    flutmax::decompile(maxpat_json).map_err(|e| JsValue::from_str(&e))
}

/// Parse .flutmax source and return AST as JSON string.
#[wasm_bindgen]
pub fn parse(source: &str) -> Result<String, JsValue> {
    flutmax::parse_to_json(source).map_err(|e| JsValue::from_str(&e))
}

/// Get the flutmax version.
#[wasm_bindgen]
pub fn version() -> String {
    "0.1.0".to_string()
}
