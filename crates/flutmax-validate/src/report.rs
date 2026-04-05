use std::path::Path;

use flutmax_objdb::ObjectDb;

use crate::json;
use crate::static_check;
use crate::structure;

/// Severity level of a validation finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Error => write!(f, "error"),
            Severity::Warning => write!(f, "warning"),
        }
    }
}

/// A single validation finding.
#[derive(Debug, Clone)]
pub struct ValidationError {
    pub severity: Severity,
    /// Which validation layer produced this error: "json", "structure", "static"
    pub layer: &'static str,
    pub message: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}: {}", self.layer, self.severity, self.message)
    }
}

/// Aggregated validation report for a single .maxpat file.
#[derive(Debug)]
pub struct ValidationReport {
    /// The file name or path that was validated.
    pub file: String,
    /// All validation findings.
    pub errors: Vec<ValidationError>,
    /// Number of boxes checked during structure validation.
    pub boxes_checked: usize,
    /// Number of patchlines checked during structure validation.
    pub lines_checked: usize,
}

impl ValidationReport {
    /// Returns true if any error-severity finding exists.
    pub fn has_errors(&self) -> bool {
        self.errors.iter().any(|e| e.severity == Severity::Error)
    }

    /// Count of error-severity findings.
    pub fn error_count(&self) -> usize {
        self.errors
            .iter()
            .filter(|e| e.severity == Severity::Error)
            .count()
    }

    /// Count of warning-severity findings.
    pub fn warning_count(&self) -> usize {
        self.errors
            .iter()
            .filter(|e| e.severity == Severity::Warning)
            .count()
    }
}

impl std::fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Validation: {}", self.file)?;
        writeln!(
            f,
            "  {} error(s), {} warning(s)",
            self.error_count(),
            self.warning_count()
        )?;
        writeln!(
            f,
            "  Checked: {} boxes, {} lines",
            self.boxes_checked, self.lines_checked
        )?;
        for err in &self.errors {
            writeln!(f, "  {}", err)?;
        }
        Ok(())
    }
}

/// Run all Layer 1 validations on a .maxpat file.
///
/// Chains: JSON parsing -> structure validation -> report.
/// If JSON parsing fails, structure validation is skipped.
pub fn validate(path: &Path) -> ValidationReport {
    validate_with_objdb(path, None)
}

/// Run all Layer 1 validations on a .maxpat JSON string.
///
/// Chains: JSON parsing -> structure validation -> report.
/// If JSON parsing fails, structure validation is skipped.
pub fn validate_str(content: &str, filename: &str) -> ValidationReport {
    validate_str_with_objdb(content, filename, None)
}

/// Run all Layer 1 validations on a .maxpat file with an optional full ObjectDb.
///
/// Chains: JSON parsing -> structure validation -> static analysis with objdb -> report.
pub fn validate_with_objdb(path: &Path, objdb: Option<&ObjectDb>) -> ValidationReport {
    let file = path.display().to_string();

    // Step 1: JSON validation
    let json_value = match json::validate_json(path) {
        Ok(v) => v,
        Err(e) => {
            return ValidationReport {
                file,
                errors: vec![ValidationError {
                    severity: Severity::Error,
                    layer: "json",
                    message: e.message,
                }],
                boxes_checked: 0,
                lines_checked: 0,
            };
        }
    };

    build_report_from_json_with_objdb(&json_value, file, objdb)
}

/// Run all Layer 1 validations on a .maxpat JSON string with an optional full ObjectDb.
///
/// Chains: JSON parsing -> structure validation -> static analysis with objdb -> report.
pub fn validate_str_with_objdb(
    content: &str,
    filename: &str,
    objdb: Option<&ObjectDb>,
) -> ValidationReport {
    let file = filename.to_string();

    // Step 1: JSON validation
    let json_value = match json::validate_json_str(content) {
        Ok(v) => v,
        Err(e) => {
            return ValidationReport {
                file,
                errors: vec![ValidationError {
                    severity: Severity::Error,
                    layer: "json",
                    message: e.message,
                }],
                boxes_checked: 0,
                lines_checked: 0,
            };
        }
    };

    build_report_from_json_with_objdb(&json_value, file, objdb)
}

/// Build a ValidationReport from a successfully parsed JSON value with an optional ObjectDb.
fn build_report_from_json_with_objdb(
    json_value: &serde_json::Value,
    file: String,
    objdb: Option<&ObjectDb>,
) -> ValidationReport {
    let boxes_checked = structure::count_boxes(json_value);
    let lines_checked = structure::count_lines(json_value);

    // Step 2: Structure validation
    let structure_errors = structure::validate_structure(json_value);

    let mut errors: Vec<ValidationError> = structure_errors
        .into_iter()
        .map(|se| ValidationError {
            severity: Severity::Error,
            layer: "structure",
            message: format!("{}: {}", se.path, se.message),
        })
        .collect();

    // Step 3: Static analysis (objdb-based)
    let static_errors = static_check::validate_static_with_objdb(json_value, objdb);
    for se in static_errors {
        let severity = match se.error_type {
            static_check::StaticErrorType::UnknownObject => Severity::Warning,
            _ => Severity::Error,
        };
        errors.push(ValidationError {
            severity,
            layer: "static",
            message: format!("{}: {}", se.box_id, se.message),
        });
    }

    ValidationReport {
        file,
        errors,
        boxes_checked,
        lines_checked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_maxpat_json() -> &'static str {
        r#"{
            "patcher": {
                "fileversion": 1,
                "appversion": {"major": 8, "minor": 6},
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "newobj",
                            "text": "cycle~ 440",
                            "numinlets": 2,
                            "numoutlets": 1,
                            "patching_rect": [100.0, 200.0, 80.0, 22.0]
                        }
                    },
                    {
                        "box": {
                            "id": "obj-2",
                            "maxclass": "newobj",
                            "text": "dac~",
                            "numinlets": 2,
                            "numoutlets": 0,
                            "patching_rect": [100.0, 300.0, 40.0, 22.0]
                        }
                    }
                ],
                "lines": [
                    {
                        "patchline": {
                            "source": ["obj-1", 0],
                            "destination": ["obj-2", 0]
                        }
                    }
                ]
            }
        }"#
    }

    #[test]
    fn valid_file_no_errors() {
        let report = validate_str(valid_maxpat_json(), "test.maxpat");
        assert!(!report.has_errors());
        assert_eq!(report.error_count(), 0);
        assert_eq!(report.warning_count(), 0);
        assert_eq!(report.boxes_checked, 2);
        assert_eq!(report.lines_checked, 1);
        assert_eq!(report.file, "test.maxpat");
    }

    #[test]
    fn invalid_json_returns_one_error() {
        let report = validate_str("{invalid json", "bad.maxpat");
        assert!(report.has_errors());
        assert_eq!(report.error_count(), 1);
        assert_eq!(report.errors[0].layer, "json");
        assert_eq!(report.errors[0].severity, Severity::Error);
        // When JSON fails, structure validation is skipped
        assert_eq!(report.boxes_checked, 0);
        assert_eq!(report.lines_checked, 0);
    }

    #[test]
    fn empty_string_returns_json_error() {
        let report = validate_str("", "empty.maxpat");
        assert!(report.has_errors());
        assert_eq!(report.error_count(), 1);
        assert_eq!(report.errors[0].layer, "json");
    }

    #[test]
    fn multiple_structure_errors() {
        // Missing fileversion, boxes have problems
        let json = r#"{
            "patcher": {
                "boxes": [
                    {"box": {"maxclass": "button", "numinlets": 1, "numoutlets": 1, "patching_rect": [0,0,20,20]}},
                    {"box": {"id": "obj-2", "numinlets": 1, "numoutlets": 1, "patching_rect": [0,0,20,20]}}
                ],
                "lines": []
            }
        }"#;
        let report = validate_str(json, "multi_error.maxpat");
        assert!(report.has_errors());
        // Missing fileversion + box[0] missing id + box[1] missing maxclass = at least 3 errors
        assert!(
            report.error_count() >= 3,
            "Expected at least 3 errors, got {}",
            report.error_count()
        );
        assert_eq!(report.boxes_checked, 2);
    }

    #[test]
    fn json_skips_structure_validation() {
        // If JSON is invalid, structure errors should NOT appear
        let report = validate_str("not json at all", "broken.maxpat");
        assert_eq!(report.error_count(), 1);
        assert_eq!(report.errors[0].layer, "json");
    }

    #[test]
    fn report_display() {
        let report = validate_str(valid_maxpat_json(), "display_test.maxpat");
        let display = format!("{}", report);
        assert!(display.contains("display_test.maxpat"));
        assert!(display.contains("0 error(s)"));
    }

    #[test]
    fn validate_file_not_found() {
        let report = validate(Path::new("/nonexistent/path/test.maxpat"));
        assert!(report.has_errors());
        assert_eq!(report.error_count(), 1);
        assert_eq!(report.errors[0].layer, "json");
    }

    #[test]
    fn severity_display() {
        assert_eq!(format!("{}", Severity::Error), "error");
        assert_eq!(format!("{}", Severity::Warning), "warning");
    }

    #[test]
    fn validation_error_display() {
        let err = ValidationError {
            severity: Severity::Error,
            layer: "structure",
            message: "Missing field".to_string(),
        };
        let display = format!("{}", err);
        assert_eq!(display, "[structure] error: Missing field");
    }

    #[test]
    fn report_with_warnings_only() {
        // A valid JSON with valid structure should have no errors and no warnings
        let report = validate_str(valid_maxpat_json(), "clean.maxpat");
        assert!(!report.has_errors());
        assert_eq!(report.error_count(), 0);
        assert_eq!(report.warning_count(), 0);
    }

    #[test]
    fn valid_empty_patcher() {
        let json = r#"{
            "patcher": {
                "fileversion": 1,
                "boxes": [],
                "lines": []
            }
        }"#;
        let report = validate_str(json, "empty_patcher.maxpat");
        assert!(!report.has_errors());
        assert_eq!(report.boxes_checked, 0);
        assert_eq!(report.lines_checked, 0);
    }
}
