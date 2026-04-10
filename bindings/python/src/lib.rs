use pyo3::prelude::*;
use std::fs;
use std::path::Path;

/// Compile .flutmax source string to .maxpat JSON string.
#[pyfunction]
fn compile(source: &str) -> PyResult<String> {
    flutmax::compile(source)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e))
}

/// Decompile .maxpat JSON string to .flutmax source string.
#[pyfunction]
fn decompile(maxpat_json: &str) -> PyResult<String> {
    flutmax::decompile(maxpat_json)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e))
}

/// Parse .flutmax source and return AST as JSON string.
#[pyfunction]
fn parse(source: &str) -> PyResult<String> {
    flutmax::parse_to_json(source)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PySyntaxError, _>(e))
}

/// Compile a .flutmax file to a .maxpat file.
///
/// Reads `input_path`, compiles, and writes to `output_path`.
#[pyfunction]
fn compile_file(input_path: &str, output_path: &str) -> PyResult<()> {
    let source = fs::read_to_string(input_path)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyIOError, _>(format!("{}: {}", input_path, e)))?;
    let maxpat = flutmax::compile(&source)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e))?;
    fs::write(output_path, &maxpat)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyIOError, _>(format!("{}: {}", output_path, e)))?;
    Ok(())
}

/// Decompile a .maxpat file to a .flutmax file.
///
/// Reads `input_path`, decompiles, and writes to `output_path`.
#[pyfunction]
fn decompile_file(input_path: &str, output_path: &str) -> PyResult<()> {
    let json = fs::read_to_string(input_path)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyIOError, _>(format!("{}: {}", input_path, e)))?;
    let source = flutmax::decompile(&json)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e))?;

    // Create parent directories if needed
    if let Some(parent) = Path::new(output_path).parent() {
        fs::create_dir_all(parent)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyIOError, _>(format!("{}: {}", output_path, e)))?;
    }

    fs::write(output_path, &source)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyIOError, _>(format!("{}: {}", output_path, e)))?;
    Ok(())
}

/// flutmax Python module
#[pymodule]
fn flutmax_py(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(compile, m)?)?;
    m.add_function(wrap_pyfunction!(decompile, m)?)?;
    m.add_function(wrap_pyfunction!(parse, m)?)?;
    m.add_function(wrap_pyfunction!(compile_file, m)?)?;
    m.add_function(wrap_pyfunction!(decompile_file, m)?)?;
    m.add("__version__", "0.1.2")?;
    Ok(())
}
