pub mod validate;

use flutmax_codegen::{build_graph, build_graph_with_objdb, generate, generate_with_options, generate_with_ui, BuildError, CodeFiles, CodegenError, GenerateOptions, UiData};
use flutmax_objdb::ObjectDb;
use flutmax_parser::parse;
use flutmax_sema::registry::AbstractionRegistry;
use flutmax_sema::type_check::{type_check, type_check_with_registry, TypeError};

/// Compilation error type that wraps all pipeline errors.
#[derive(Debug)]
pub enum CompileError {
    Parse(flutmax_parser::ParseError),
    Type(Vec<TypeError>),
    BuildGraph(BuildError),
    Codegen(CodegenError),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Parse(e) => write!(f, "parse error: {}", e),
            CompileError::Type(errors) => {
                let msg = errors.iter().map(|e| format!("{}", e)).collect::<Vec<_>>().join("\n");
                write!(f, "{}", msg)
            }
            CompileError::BuildGraph(e) => write!(f, "graph build error: {}", e),
            CompileError::Codegen(e) => write!(f, "codegen error: {}", e),
        }
    }
}

impl std::error::Error for CompileError {}

impl From<flutmax_parser::ParseError> for CompileError {
    fn from(e: flutmax_parser::ParseError) -> Self {
        CompileError::Parse(e)
    }
}

impl From<BuildError> for CompileError {
    fn from(e: BuildError) -> Self {
        CompileError::BuildGraph(e)
    }
}

impl From<CodegenError> for CompileError {
    fn from(e: CodegenError) -> Self {
        CompileError::Codegen(e)
    }
}

/// Compile a .flutmax source string into a .maxpat JSON string.
pub fn compile(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let ast = parse(source)?;
    let type_errors = type_check(&ast);
    if !type_errors.is_empty() {
        let msg = type_errors.iter().map(|e| format!("{}", e)).collect::<Vec<_>>().join("\n");
        return Err(msg.into());
    }
    let graph = build_graph(&ast)?;
    let json = generate(&graph)?;
    Ok(json)
}

/// Compile a .flutmax source string with an AbstractionRegistry.
///
/// When a registry is provided, it resolves Abstraction inlet/outlet counts
/// by referencing `in`/`out` declarations from other `.flutmax` files.
pub fn compile_with_registry(
    source: &str,
    registry: Option<&AbstractionRegistry>,
) -> Result<String, Box<dyn std::error::Error>> {
    compile_with_registry_and_code_files(source, registry, None)
}

/// Compile with AbstractionRegistry and CodeFiles for codebox support.
pub fn compile_with_registry_and_code_files(
    source: &str,
    registry: Option<&AbstractionRegistry>,
    code_files: Option<&CodeFiles>,
) -> Result<String, Box<dyn std::error::Error>> {
    compile_full(source, registry, code_files, None)
}

/// Compile with all options: registry, code_files, and objdb.
pub fn compile_full(
    source: &str,
    registry: Option<&AbstractionRegistry>,
    code_files: Option<&CodeFiles>,
    objdb: Option<&ObjectDb>,
) -> Result<String, Box<dyn std::error::Error>> {
    let ast = parse(source)?;
    let type_errors = type_check_with_registry(&ast, registry);
    if !type_errors.is_empty() {
        let msg = type_errors.iter().map(|e| format!("{}", e)).collect::<Vec<_>>().join("\n");
        return Err(msg.into());
    }
    let graph = build_graph_with_objdb(&ast, registry, code_files, objdb)?;
    let json = generate(&graph)?;
    Ok(json)
}

/// Compile a .flutmax source as an RNBO patcher (classnamespace: "rnbo").
pub fn compile_rnbo(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let ast = parse(source)?;
    let type_errors = type_check(&ast);
    if !type_errors.is_empty() {
        let msg = type_errors.iter().map(|e| format!("{}", e)).collect::<Vec<_>>().join("\n");
        return Err(msg.into());
    }
    let graph = build_graph(&ast)?;
    let opts = GenerateOptions { classnamespace: "rnbo".to_string() };
    let json = generate_with_options(&graph, &opts)?;
    Ok(json)
}

/// Compile with all options including UI data from .uiflutmax sidecar file.
pub fn compile_full_with_ui(
    source: &str,
    registry: Option<&AbstractionRegistry>,
    code_files: Option<&CodeFiles>,
    objdb: Option<&ObjectDb>,
    ui_data: Option<&UiData>,
) -> Result<String, Box<dyn std::error::Error>> {
    let ast = parse(source)?;
    let type_errors = type_check_with_registry(&ast, registry);
    if !type_errors.is_empty() {
        let msg = type_errors.iter().map(|e| format!("{}", e)).collect::<Vec<_>>().join("\n");
        return Err(msg.into());
    }
    let graph = build_graph_with_objdb(&ast, registry, code_files, objdb)?;
    let json = generate_with_ui(&graph, &GenerateOptions::default(), ui_data)?;
    Ok(json)
}

/// Compile a .flutmax source as a gen~ patcher (classnamespace: "dsp.gen").
///
/// Inside gen~, all objects operate at signal rate, so
/// E001 (Signal->Control connection) and E005 (output type mismatch) are suppressed.
/// E002 (undefined reference) and E003 (duplicate definition) are still detected.
pub fn compile_gen(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let ast = parse(source)?;
    let type_errors: Vec<_> = type_check(&ast)
        .into_iter()
        .filter(|e| e.code != "E001" && e.code != "E005")
        .collect();
    if !type_errors.is_empty() {
        let msg = type_errors.iter().map(|e| format!("{}", e)).collect::<Vec<_>>().join("\n");
        return Err(msg.into());
    }
    let graph = build_graph(&ast)?;
    let opts = GenerateOptions { classnamespace: "dsp.gen".to_string() };
    let json = generate_with_options(&graph, &opts)?;
    Ok(json)
}
