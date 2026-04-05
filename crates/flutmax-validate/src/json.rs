use serde_json::Value;
use std::path::Path;

/// JSON parsing error with optional location information.
#[derive(Debug, Clone)]
pub struct JsonError {
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.line, self.column) {
            (Some(line), Some(col)) => write!(f, "{}:{}: {}", line, col, self.message),
            (Some(line), None) => write!(f, "{}: {}", line, self.message),
            _ => write!(f, "{}", self.message),
        }
    }
}

impl std::error::Error for JsonError {}

/// Validate a file as JSON. Reads the file and parses it.
///
/// Returns the parsed JSON value on success, or a `JsonError` with
/// line/column information on failure.
pub fn validate_json(path: &Path) -> Result<Value, JsonError> {
    let content = std::fs::read_to_string(path).map_err(|e| JsonError {
        message: format!("Failed to read file: {}", e),
        line: None,
        column: None,
    })?;
    validate_json_str(&content)
}

/// Validate a string as JSON.
///
/// Returns the parsed JSON value on success, or a `JsonError` with
/// line/column information on failure.
pub fn validate_json_str(content: &str) -> Result<Value, JsonError> {
    if content.trim().is_empty() {
        return Err(JsonError {
            message: "Empty file".to_string(),
            line: None,
            column: None,
        });
    }

    serde_json::from_str(content).map_err(|e| {
        let line = if e.line() > 0 { Some(e.line()) } else { None };
        let column = if e.column() > 0 {
            Some(e.column())
        } else {
            None
        };
        JsonError {
            message: e.to_string(),
            line,
            column,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_json() {
        let json = r#"{"patcher": {"boxes": []}}"#;
        let result = validate_json_str(json);
        assert!(result.is_ok());
    }

    #[test]
    fn invalid_json_missing_comma() {
        let json = r#"{
    "a": 1
    "b": 2
}"#;
        let result = validate_json_str(json);
        assert!(result.is_err());
        let err = result.unwrap_err();
        // serde_json reports line/column for parse errors
        assert!(err.line.is_some(), "expected line number in error");
        assert!(err.message.contains("expected"));
    }

    #[test]
    fn empty_file() {
        let result = validate_json_str("");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.message, "Empty file");
    }

    #[test]
    fn whitespace_only_file() {
        let result = validate_json_str("   \n\n  ");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.message, "Empty file");
    }

    #[test]
    fn valid_complex_json() {
        let json = r#"{
            "patcher": {
                "fileversion": 1,
                "boxes": [
                    {"box": {"id": "obj-1", "maxclass": "newobj"}}
                ],
                "lines": []
            }
        }"#;
        let result = validate_json_str(json);
        assert!(result.is_ok());
    }

    #[test]
    fn invalid_json_trailing_comma() {
        let json = r#"{"a": 1,}"#;
        let result = validate_json_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn validate_json_file_not_found() {
        let result = validate_json(Path::new("/nonexistent/file.maxpat"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("Failed to read file"));
        assert!(err.line.is_none());
    }

    #[test]
    fn json_error_display_with_location() {
        let err = JsonError {
            message: "unexpected token".to_string(),
            line: Some(3),
            column: Some(5),
        };
        assert_eq!(format!("{}", err), "3:5: unexpected token");
    }

    #[test]
    fn json_error_display_without_location() {
        let err = JsonError {
            message: "Empty file".to_string(),
            line: None,
            column: None,
        };
        assert_eq!(format!("{}", err), "Empty file");
    }
}
