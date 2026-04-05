pub mod lexer;
#[cfg(any(feature = "tree-sitter-legacy", test))]
pub mod parse;
pub mod parser;
pub mod tokens;

/// Error type for the public parse API.
#[derive(Debug)]
pub enum ParseError {
    InvalidSyntax {
        message: String,
        line: usize,
        column: usize,
    },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::InvalidSyntax {
                message,
                line,
                column,
            } => {
                write!(f, "Syntax error at {}:{}: {}", line, column, message)
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse .flutmax source into AST (hand-written lexer + recursive descent parser).
pub fn parse(source: &str) -> Result<flutmax_ast::Program, ParseError> {
    parser::FlutmaxParser::parse(source).map_err(|e| ParseError::InvalidSyntax {
        message: format!("{}", e),
        line: match &e {
            parser::ParseError::Syntax { line, .. } => *line,
            parser::ParseError::Lex(le) => le.line,
        },
        column: match &e {
            parser::ParseError::Syntax { column, .. } => *column,
            parser::ParseError::Lex(le) => le.column,
        },
    })
}

/// Legacy: parse using tree-sitter (for comparison/migration testing).
#[cfg(any(feature = "tree-sitter-legacy", test))]
pub fn parse_legacy(source: &str) -> Result<flutmax_ast::Program, parse::ParseError> {
    parse::parse(source)
}

/// Parse using the hand-written parser directly (returns parser-specific error).
pub fn parse_new(source: &str) -> Result<flutmax_ast::Program, parser::ParseError> {
    parser::FlutmaxParser::parse(source)
}

/// Parse with error recovery: returns a (possibly partial) AST and all errors.
/// The parser skips to the next `;` on error and continues, collecting all diagnostics.
pub fn parse_new_with_errors(
    source: &str,
) -> Result<(flutmax_ast::Program, Vec<parser::ParseError>), parser::ParseError> {
    parser::FlutmaxParser::parse_with_errors(source)
}
