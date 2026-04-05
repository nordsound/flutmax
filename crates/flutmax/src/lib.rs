//! # flutmax
//!
//! Transpiler between `.flutmax` text and Max/MSP `.maxpat` patches.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! let maxpat = flutmax::compile("wire osc = cycle~(440);\nout audio: signal = osc;").unwrap();
//! let source = flutmax::decompile(&maxpat).unwrap();
//! ```

// Re-export sub-crates for advanced usage
pub use flutmax_ast as ast;
pub use flutmax_parser as parser;
pub use flutmax_codegen as codegen;
pub use flutmax_decompile as decompile;
pub use flutmax_sema as sema;
pub use flutmax_objdb as objdb;

/// Compile `.flutmax` source to `.maxpat` JSON string.
///
/// # Example
/// ```no_run
/// let maxpat = flutmax::compile("wire osc = cycle~(440);\nout audio: signal = osc;").unwrap();
/// ```
pub fn compile(source: &str) -> Result<String, String> {
    let ast = flutmax_parser::parse(source).map_err(|e| e.to_string())?;
    let graph = flutmax_codegen::build_graph(&ast).map_err(|e| format!("{:?}", e))?;
    flutmax_codegen::generate(&graph).map_err(|e| format!("{:?}", e))
}

/// Decompile `.maxpat` JSON string to `.flutmax` source.
///
/// # Example
/// ```rust,no_run
/// let source = flutmax::decompile("{}").unwrap();
/// ```
pub fn decompile(maxpat_json: &str) -> Result<String, String> {
    flutmax_decompile::decompile(maxpat_json).map_err(|e| format!("{:?}", e))
}

/// Parse `.flutmax` source and return AST as JSON string.
///
/// Useful for language bindings (Python, WASM) that can't access Rust types directly.
pub fn parse_to_json(source: &str) -> Result<String, String> {
    let ast = flutmax_parser::parse(source).map_err(|e| e.to_string())?;
    ast_to_json(&ast)
}

/// Decompile with multi-file support (subpatchers, codebox, UI data).
pub fn decompile_multi(maxpat_json: &str, name: &str) -> Result<flutmax_decompile::DecompileResult, String> {
    flutmax_decompile::decompile_multi(maxpat_json, name).map_err(|e| format!("{:?}", e))
}

/// Convert an AST Program to a JSON string.
fn ast_to_json(program: &flutmax_ast::Program) -> Result<String, String> {
    // Manual serialization since Program doesn't derive Serialize
    let mut obj = serde_json::Map::new();

    // in_decls
    let in_decls: Vec<serde_json::Value> = program.in_decls.iter().map(|d| {
        serde_json::json!({
            "index": d.index,
            "name": d.name,
            "port_type": format!("{:?}", d.port_type),
        })
    }).collect();
    obj.insert("in_decls".into(), serde_json::Value::Array(in_decls));

    // out_decls
    let out_decls: Vec<serde_json::Value> = program.out_decls.iter().map(|d| {
        serde_json::json!({
            "index": d.index,
            "name": d.name,
            "port_type": format!("{:?}", d.port_type),
        })
    }).collect();
    obj.insert("out_decls".into(), serde_json::Value::Array(out_decls));

    // wires
    let wires: Vec<serde_json::Value> = program.wires.iter().map(|w| {
        serde_json::json!({
            "name": w.name,
            "expr": format!("{:?}", w.value),
            "attrs": w.attrs.iter().map(|a| {
                serde_json::json!({"key": a.key, "value": format!("{:?}", a.value)})
            }).collect::<Vec<_>>(),
        })
    }).collect();
    obj.insert("wires".into(), serde_json::Value::Array(wires));

    // out_assignments
    let out_assigns: Vec<serde_json::Value> = program.out_assignments.iter().map(|a| {
        serde_json::json!({
            "index": a.index,
            "value": format!("{:?}", a.value),
        })
    }).collect();
    obj.insert("out_assignments".into(), serde_json::Value::Array(out_assigns));

    serde_json::to_string_pretty(&serde_json::Value::Object(obj))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile() {
        let result = compile("wire osc = cycle~(440);\nout audio: signal = osc;");
        assert!(result.is_ok(), "compile failed: {:?}", result.err());
        let json = result.unwrap();
        assert!(json.contains("cycle~"));
    }

    #[test]
    fn test_compile_error() {
        let result = compile("wire osc = ;");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_to_json() {
        let result = parse_to_json("in freq: float;\nwire osc = cycle~(freq);\nout audio: signal = osc;");
        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.contains("freq"));
        assert!(json.contains("cycle~"));
    }

    #[test]
    fn test_roundtrip() {
        let source = "in freq: float;\nwire osc = cycle~(freq);\nout audio: signal = osc;\n";
        let maxpat = compile(source).unwrap();
        let decompiled = decompile(&maxpat).unwrap();
        assert!(decompiled.contains("cycle~"));
    }
}
