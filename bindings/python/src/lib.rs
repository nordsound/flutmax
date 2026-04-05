use pyo3::prelude::*;

/// Compile .flutmax source to .maxpat JSON string.
#[pyfunction]
fn compile(source: &str) -> PyResult<String> {
    flutmax::compile(source)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e))
}

/// Decompile .maxpat JSON string to .flutmax source.
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

/// flutmax Python module
#[pymodule]
fn flutmax_py(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(compile, m)?)?;
    m.add_function(wrap_pyfunction!(decompile, m)?)?;
    m.add_function(wrap_pyfunction!(parse, m)?)?;
    m.add("__version__", "0.1.0")?;
    Ok(())
}
