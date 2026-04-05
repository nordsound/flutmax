/// Token types for the flutmax lexer.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenType {
    // Keywords
    Wire,
    In,
    Out,
    State,
    Msg,
    Feedback,
    Signal,
    Float,
    Int,
    Bang,
    List,
    Symbol,

    // Delimiters
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Semicolon,
    Colon,
    Dot,
    Eq,
    Tilde,

    // Identifiers & Literals
    /// Plain identifier: `foo`, `drunk-walk`, `node_a`
    Identifier,
    /// Operator name used as Max/gen~ object name: `?`, `*`, `+`, `-`, `/`, `%`, `==`, etc.
    Operator,
    /// Numeric literal: `42`, `3.14`, `-7`, `1e-6`, `100.`
    NumberLit,
    /// String literal: `"hello"`
    StringLit,

    // Special
    /// `.attr(` — recognized as a single token for simplicity
    DotAttrLParen,

    /// Line comment: `// ...`
    Comment,

    // Misc
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub token_type: TokenType,
    pub lexeme: String,
    pub line: usize,
    pub column: usize,
}

impl Token {
    pub fn new(
        token_type: TokenType,
        lexeme: impl Into<String>,
        line: usize,
        column: usize,
    ) -> Self {
        Self {
            token_type,
            lexeme: lexeme.into(),
            line,
            column,
        }
    }
}
