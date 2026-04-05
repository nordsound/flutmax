/// Hand-written lexer for flutmax source code.
///
/// Converts a source string into a stream of `Token`s that the recursive-descent
/// parser consumes. Key design decisions:
///
/// - Identifiers: `[a-zA-Z_][a-zA-Z0-9_]*(-[a-zA-Z0-9_]+)*` (hyphens allowed, e.g. `drunk-walk`)
/// - Dot is always emitted as a separate `Dot` token
/// - Tilde `~` is always emitted as a separate `Tilde` token
/// - The parser reassembles dotted names (`jit.gl.render`) and tilde names (`cycle~`)
/// - `.attr(` is recognized as a single `DotAttrLParen` token
/// - Operator chars (`?*+/%!<>=&|^-`) form `Operator` tokens when not part of numbers
/// - Keywords are checked after identifier scanning
use crate::tokens::{Token, TokenType};

pub struct Lexer<'a> {
    source: &'a [u8],
    pos: usize,
    line: usize,
    col: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source: source.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    /// Tokenize the entire source, returning a Vec of tokens (ending with Eof).
    pub fn tokenize(source: &str) -> Result<Vec<Token>, LexError> {
        let mut lexer = Lexer::new(source);
        let mut tokens = Vec::new();
        loop {
            let tok = lexer.next_token()?;
            let is_eof = tok.token_type == TokenType::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    /// Tokenize the entire source, including comment tokens (for semantic highlighting).
    /// Comments are emitted as `TokenType::Comment` tokens instead of being skipped.
    pub fn tokenize_with_comments(source: &str) -> Result<Vec<Token>, LexError> {
        let mut lexer = Lexer::new(source);
        let mut tokens = Vec::new();
        loop {
            let tok = lexer.next_token_with_comments()?;
            let is_eof = tok.token_type == TokenType::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.source.get(self.pos + offset).copied()
    }

    fn advance(&mut self) -> Option<u8> {
        let ch = self.source.get(self.pos).copied()?;
        self.pos += 1;
        if ch == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            // Skip whitespace
            while let Some(ch) = self.peek() {
                if ch == b' ' || ch == b'\t' || ch == b'\n' || ch == b'\r' {
                    self.advance();
                } else {
                    break;
                }
            }
            // Skip line comments
            if self.peek() == Some(b'/') && self.peek_at(1) == Some(b'/') {
                // Consume until end of line
                while let Some(ch) = self.peek() {
                    if ch == b'\n' {
                        break;
                    }
                    self.advance();
                }
                // Continue to skip more whitespace/comments
                continue;
            }
            break;
        }
    }

    /// Skip whitespace only (not comments). Returns `Some(Comment token)` if a
    /// comment was found, or `None` if no comment follows whitespace.
    fn skip_whitespace_and_maybe_comment(&mut self) -> Option<Token> {
        // Skip whitespace
        while let Some(ch) = self.peek() {
            if ch == b' ' || ch == b'\t' || ch == b'\n' || ch == b'\r' {
                self.advance();
            } else {
                break;
            }
        }
        // Check for line comment — emit as token instead of skipping
        if self.peek() == Some(b'/') && self.peek_at(1) == Some(b'/') {
            let line = self.line;
            let col = self.col;
            let start = self.pos;
            // Consume until end of line
            while let Some(ch) = self.peek() {
                if ch == b'\n' {
                    break;
                }
                self.advance();
            }
            let text = std::str::from_utf8(&self.source[start..self.pos]).unwrap();
            return Some(Token::new(TokenType::Comment, text, line, col));
        }
        None
    }

    /// Like `next_token` but emits `Comment` tokens instead of skipping them.
    fn next_token_with_comments(&mut self) -> Result<Token, LexError> {
        if let Some(comment_tok) = self.skip_whitespace_and_maybe_comment() {
            return Ok(comment_tok);
        }

        let line = self.line;
        let col = self.col;

        let ch = match self.peek() {
            Some(ch) => ch,
            None => return Ok(Token::new(TokenType::Eof, "", line, col)),
        };

        // Delegate to the same matching logic as next_token
        self.lex_token_char(ch, line, col)
    }

    fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_whitespace_and_comments();

        let line = self.line;
        let col = self.col;

        let ch = match self.peek() {
            Some(ch) => ch,
            None => return Ok(Token::new(TokenType::Eof, "", line, col)),
        };

        self.lex_token_char(ch, line, col)
    }

    /// Core token matching logic shared by `next_token` and `next_token_with_comments`.
    fn lex_token_char(&mut self, ch: u8, line: usize, col: usize) -> Result<Token, LexError> {
        match ch {
            b'(' => {
                self.advance();
                Ok(Token::new(TokenType::LParen, "(", line, col))
            }
            b')' => {
                self.advance();
                Ok(Token::new(TokenType::RParen, ")", line, col))
            }
            b'[' => {
                self.advance();
                Ok(Token::new(TokenType::LBracket, "[", line, col))
            }
            b']' => {
                self.advance();
                Ok(Token::new(TokenType::RBracket, "]", line, col))
            }
            b',' => {
                self.advance();
                Ok(Token::new(TokenType::Comma, ",", line, col))
            }
            b';' => {
                self.advance();
                Ok(Token::new(TokenType::Semicolon, ";", line, col))
            }
            b':' => {
                self.advance();
                Ok(Token::new(TokenType::Colon, ":", line, col))
            }
            b'~' => {
                self.advance();
                Ok(Token::new(TokenType::Tilde, "~", line, col))
            }
            b'.' => {
                // Check for `.attr(` special token
                if self.matches_ahead(b".attr(") {
                    for _ in 0..6 {
                        self.advance();
                    }
                    Ok(Token::new(TokenType::DotAttrLParen, ".attr(", line, col))
                } else {
                    self.advance();
                    Ok(Token::new(TokenType::Dot, ".", line, col))
                }
            }
            b'=' => {
                // Could be `=`, `==`, or longer operator
                // If followed by `=` or another operator char, treat as operator
                if self.peek_at(1) == Some(b'=') || self.is_operator_char_at(1) {
                    self.lex_operator(line, col)
                } else {
                    self.advance();
                    Ok(Token::new(TokenType::Eq, "=", line, col))
                }
            }
            b'"' => self.lex_string(line, col),
            b'-' => {
                // Negative number or operator
                // It's a negative number if followed by a digit
                // BUT only if the previous significant token is not an identifier/number/rparen
                // (to handle `sub(1, -2)` vs operator `-`)
                // For simplicity: if `-` followed by digit, lex as number
                if self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
                    self.lex_number(line, col)
                } else {
                    self.lex_operator(line, col)
                }
            }
            _ if ch.is_ascii_digit() => self.lex_number(line, col),
            _ if is_ident_start(ch) => self.lex_identifier(line, col),
            _ if is_operator_char(ch) => self.lex_operator(line, col),
            _ => Err(LexError {
                message: format!("Unexpected character '{}'", ch as char),
                line,
                column: col,
            }),
        }
    }

    fn matches_ahead(&self, pattern: &[u8]) -> bool {
        if self.pos + pattern.len() > self.source.len() {
            return false;
        }
        &self.source[self.pos..self.pos + pattern.len()] == pattern
    }

    fn is_operator_char_at(&self, offset: usize) -> bool {
        self.peek_at(offset).is_some_and(is_operator_char)
    }

    /// Lex an identifier: `[a-zA-Z_][a-zA-Z0-9_]*(-[a-zA-Z0-9_]+)*`
    /// Then check if it's a keyword.
    fn lex_identifier(&mut self, line: usize, col: usize) -> Result<Token, LexError> {
        let start = self.pos;
        // First char: [a-zA-Z_]
        self.advance();
        // Continue: [a-zA-Z0-9_]
        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric() || ch == b'_' {
                self.advance();
            } else {
                break;
            }
        }
        // Hyphenated segments: `-[a-zA-Z0-9_]+`
        // A hyphen followed by alphanumeric (not a digit alone, to avoid `-7`)
        while self.peek() == Some(b'-') {
            // Look ahead: next char after `-` must be a letter or digit that forms
            // part of the identifier, not a standalone negative number
            if let Some(next) = self.peek_at(1) {
                if next.is_ascii_alphanumeric() || next == b'_' {
                    self.advance(); // consume `-`
                                    // consume segment
                    while let Some(ch) = self.peek() {
                        if ch.is_ascii_alphanumeric() || ch == b'_' {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        let text = std::str::from_utf8(&self.source[start..self.pos]).unwrap();
        let token_type = keyword_or_ident(text);
        Ok(Token::new(token_type, text, line, col))
    }

    /// Lex a number: integer, float, scientific notation, trailing dot, negative.
    fn lex_number(&mut self, line: usize, col: usize) -> Result<Token, LexError> {
        let start = self.pos;

        // Optional leading `-`
        if self.peek() == Some(b'-') {
            self.advance();
        }

        // Digits
        self.consume_digits();

        // Optional decimal part
        if self.peek() == Some(b'.') {
            // Peek ahead: if the next char after `.` is a digit, or nothing follows
            // (trailing dot like `100.`), consume the dot.
            // But NOT if it's `.attr(` or `.in[` etc.
            let after_dot = self.peek_at(1);
            let consume_dot = match after_dot {
                Some(d) if d.is_ascii_digit() => true,
                // Trailing dot: `100.` — only if not followed by identifier start
                // (which would be member access like `100.something`)
                Some(d) if is_ident_start(d) => false,
                _ => true, // end of input, space, comma, paren, etc.
            };
            if consume_dot {
                self.advance(); // consume `.`
                self.consume_digits(); // may be empty for trailing dot
            }
        }

        // Optional scientific notation
        if let Some(ch) = self.peek() {
            if ch == b'e' || ch == b'E' {
                self.advance(); // consume `e`/`E`
                                // Optional sign
                if let Some(sign) = self.peek() {
                    if sign == b'+' || sign == b'-' {
                        self.advance();
                    }
                }
                self.consume_digits();
            }
        }

        let text = std::str::from_utf8(&self.source[start..self.pos]).unwrap();
        Ok(Token::new(TokenType::NumberLit, text, line, col))
    }

    fn consume_digits(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                self.advance();
            } else {
                break;
            }
        }
    }

    /// Lex a string literal: `"..."` with escape sequences.
    fn lex_string(&mut self, line: usize, col: usize) -> Result<Token, LexError> {
        let start = self.pos;
        self.advance(); // consume opening `"`

        loop {
            match self.peek() {
                Some(b'"') => {
                    self.advance(); // consume closing `"`
                    break;
                }
                Some(b'\\') => {
                    self.advance(); // consume `\`
                    self.advance(); // consume escaped char
                }
                Some(_) => {
                    self.advance();
                }
                None => {
                    return Err(LexError {
                        message: "Unterminated string literal".to_string(),
                        line,
                        column: col,
                    });
                }
            }
        }

        let text = std::str::from_utf8(&self.source[start..self.pos]).unwrap();
        Ok(Token::new(TokenType::StringLit, text, line, col))
    }

    /// Lex an operator: `[*+/%!<>=&|^?-]+`
    /// Also consumes `=` characters when part of multi-char operators like `==`, `!=`, `<=`, `>=`.
    fn lex_operator(&mut self, line: usize, col: usize) -> Result<Token, LexError> {
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if is_operator_char(ch) || ch == b'=' {
                self.advance();
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.source[start..self.pos]).unwrap();
        Ok(Token::new(TokenType::Operator, text, line, col))
    }
}

fn is_ident_start(ch: u8) -> bool {
    ch.is_ascii_alphabetic() || ch == b'_'
}

fn is_operator_char(ch: u8) -> bool {
    matches!(
        ch,
        b'*' | b'+' | b'/' | b'%' | b'!' | b'<' | b'>' | b'&' | b'|' | b'^' | b'?'
    )
}

fn keyword_or_ident(text: &str) -> TokenType {
    match text {
        "wire" => TokenType::Wire,
        "in" => TokenType::In,
        "out" => TokenType::Out,
        "state" => TokenType::State,
        "msg" => TokenType::Msg,
        "feedback" => TokenType::Feedback,
        "signal" => TokenType::Signal,
        "float" => TokenType::Float,
        "int" => TokenType::Int,
        "bang" => TokenType::Bang,
        "list" => TokenType::List,
        "symbol" => TokenType::Symbol,
        _ => TokenType::Identifier,
    }
}

#[derive(Debug)]
pub struct LexError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Lex error at {}:{}: {}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for LexError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::TokenType::*;

    fn types(source: &str) -> Vec<TokenType> {
        Lexer::tokenize(source)
            .unwrap()
            .into_iter()
            .map(|t| t.token_type)
            .collect()
    }

    fn lexemes(source: &str) -> Vec<String> {
        Lexer::tokenize(source)
            .unwrap()
            .into_iter()
            .map(|t| t.lexeme)
            .collect()
    }

    #[test]
    fn test_simple_wire() {
        let toks = types("wire osc = cycle~(440);");
        assert_eq!(
            toks,
            vec![
                Wire, Identifier, // osc
                Eq, Identifier, // cycle
                Tilde, LParen, NumberLit, // 440
                RParen, Semicolon, Eof,
            ]
        );
    }

    #[test]
    fn test_in_decl() {
        let toks = types("in 0 (freq): signal;");
        assert_eq!(
            toks,
            vec![In, NumberLit, LParen, Identifier, RParen, Colon, Signal, Semicolon, Eof]
        );
    }

    #[test]
    fn test_dotted_identifier() {
        // `jit.gl.render(440)` → jit Dot gl Dot render LParen 440 RParen
        let lex = lexemes("jit.gl.render(440)");
        assert_eq!(
            lex,
            vec!["jit", ".", "gl", ".", "render", "(", "440", ")", ""]
        );
    }

    #[test]
    fn test_port_access() {
        // `w_1.in[0]` → w_1 Dot in LBracket 0 RBracket
        let toks = types("w_1.in[0]");
        assert_eq!(
            toks,
            vec![Identifier, Dot, In, LBracket, NumberLit, RBracket, Eof]
        );
    }

    #[test]
    fn test_output_port_access() {
        let toks = types("w_1.out[1]");
        assert_eq!(
            toks,
            vec![Identifier, Dot, Out, LBracket, NumberLit, RBracket, Eof]
        );
    }

    #[test]
    fn test_numbers() {
        // Integer
        let lex = lexemes("42");
        assert_eq!(lex, vec!["42", ""]);

        // Float
        let lex = lexemes("3.14");
        assert_eq!(lex, vec!["3.14", ""]);

        // Negative
        let lex = lexemes("-7");
        assert_eq!(lex, vec!["-7", ""]);

        // Trailing dot
        let lex = lexemes("100.");
        assert_eq!(lex, vec!["100.", ""]);

        // Scientific notation
        let lex = lexemes("1e-6");
        assert_eq!(lex, vec!["1e-6", ""]);

        // Float + scientific
        let lex = lexemes("3.14E+5");
        assert_eq!(lex, vec!["3.14E+5", ""]);
    }

    #[test]
    fn test_string() {
        let toks = Lexer::tokenize(r#""hello world""#).unwrap();
        assert_eq!(toks.len(), 2); // string + eof
        assert_eq!(toks[0].token_type, StringLit);
        assert_eq!(toks[0].lexeme, r#""hello world""#);
    }

    #[test]
    fn test_string_with_escapes() {
        let toks = Lexer::tokenize(r#""hello \"world\"""#).unwrap();
        assert_eq!(toks[0].token_type, StringLit);
        assert_eq!(toks[0].lexeme, r#""hello \"world\"""#);
    }

    #[test]
    fn test_operator_names() {
        let toks = types("?(a, b)");
        assert_eq!(
            toks,
            vec![Operator, LParen, Identifier, Comma, Identifier, RParen, Eof]
        );

        let lex = lexemes("*(x, y)");
        assert_eq!(lex[0], "*");
    }

    #[test]
    fn test_comment_skipped() {
        let toks = types("// comment\nwire x = 1;");
        assert_eq!(toks, vec![Wire, Identifier, Eq, NumberLit, Semicolon, Eof]);
    }

    #[test]
    fn test_hyphenated_identifier() {
        let lex = lexemes("drunk-walk");
        assert_eq!(lex, vec!["drunk-walk", ""]);
    }

    #[test]
    fn test_dot_attr_lparen() {
        let toks = types(".attr(minimum: 0)");
        assert_eq!(
            toks,
            vec![DotAttrLParen, Identifier, Colon, NumberLit, RParen, Eof]
        );
    }

    #[test]
    fn test_negative_float() {
        let lex = lexemes("-3.14");
        assert_eq!(lex, vec!["-3.14", ""]);
    }

    #[test]
    fn test_line_column_tracking() {
        let toks = Lexer::tokenize("wire x\n  = 1;").unwrap();
        // `wire` at (1,1)
        assert_eq!((toks[0].line, toks[0].column), (1, 1));
        // `x` at (1,6)
        assert_eq!((toks[1].line, toks[1].column), (1, 6));
        // `=` at (2,3)
        assert_eq!((toks[2].line, toks[2].column), (2, 3));
        // `1` at (2,5)
        assert_eq!((toks[3].line, toks[3].column), (2, 5));
    }

    #[test]
    fn test_empty_source() {
        let toks = types("");
        assert_eq!(toks, vec![Eof]);
    }

    #[test]
    fn test_out_assignment_tokens() {
        let toks = types("out[0] = osc;");
        assert_eq!(
            toks,
            vec![Out, LBracket, NumberLit, RBracket, Eq, Identifier, Semicolon, Eof]
        );
    }

    #[test]
    fn test_operator_eq_disambiguation() {
        // `==` should be a single operator token, not Eq Eq
        let lex = lexemes("==(a, b)");
        assert_eq!(lex[0], "==");
        assert_eq!(
            types("==(a, b)"),
            vec![Operator, LParen, Identifier, Comma, Identifier, RParen, Eof]
        );
    }

    #[test]
    fn test_dotted_segment_with_digit() {
        // `jit.3m` — dotted segment starting with digit
        // The lexer emits separate tokens: jit Dot 3 ...
        // But `3m` won't be a single identifier token — `3` is a number.
        // The parser handles reassembly with digit-starting segments.
        let lex = lexemes("jit.3m");
        // `3m` is tricky: `3` as number, then `m` as identifier
        // Actually the lexer sees `3` as digit → NumberLit, then `m` as Identifier
        assert_eq!(lex, vec!["jit", ".", "3", "m", ""]);
    }

    #[test]
    fn test_msg_tokens() {
        let toks = types(r#"msg click = "bang";"#);
        assert_eq!(toks, vec![Msg, Identifier, Eq, StringLit, Semicolon, Eof]);
    }

    #[test]
    fn test_feedback_tokens() {
        let toks = types("feedback fb: signal;");
        assert_eq!(
            toks,
            vec![Feedback, Identifier, Colon, Signal, Semicolon, Eof]
        );
    }

    #[test]
    fn test_state_tokens() {
        let toks = types("state counter: int = 0;");
        assert_eq!(
            toks,
            vec![State, Identifier, Colon, Int, Eq, NumberLit, Semicolon, Eof]
        );
    }

    #[test]
    fn test_string_with_url() {
        // String containing `//` should not be treated as comment
        let toks = Lexer::tokenize(r#""http://example.com""#).unwrap();
        assert_eq!(toks.len(), 2);
        assert_eq!(toks[0].token_type, StringLit);
        assert_eq!(toks[0].lexeme, r#""http://example.com""#);
    }

    #[test]
    fn test_complex_expr() {
        // `mul~(osc, 0.5)` — tilde identifier with float arg
        let lex = lexemes("mul~(osc, 0.5)");
        assert_eq!(lex, vec!["mul", "~", "(", "osc", ",", "0.5", ")", ""]);
    }
}
