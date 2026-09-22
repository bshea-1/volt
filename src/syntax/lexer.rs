use crate::syntax::token::{Span, Token, TokenKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LexerMode {
    Normal,
    TemplateContent,
    TemplateTag,
}

pub struct Lexer<'a> {
    input: &'a str,
    chars: Vec<(usize, char)>,
    pos: usize,
    line: usize,
    col: usize,
    mode_stack: Vec<LexerMode>,
    brace_depths: Vec<usize>, // tracks brace nesting inside Normal mode when pushed from template
    just_saw_render: bool,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        let chars: Vec<(usize, char)> = input.char_indices().collect();
        Self {
            input,
            chars,
            pos: 0,
            line: 1,
            col: 1,
            mode_stack: vec![LexerMode::Normal],
            brace_depths: Vec::new(),
            just_saw_render: false,
        }
    }

    fn current_mode(&self) -> LexerMode {
        *self.mode_stack.last().unwrap_or(&LexerMode::Normal)
    }

    fn peek(&self) -> Option<char> {
        if self.pos < self.chars.len() {
            Some(self.chars[self.pos].1)
        } else {
            None
        }
    }

    fn peek_offset(&self, offset: usize) -> Option<char> {
        let idx = self.pos + offset;
        if idx < self.chars.len() {
            Some(self.chars[idx].1)
        } else {
            None
        }
    }

    fn current_byte_idx(&self) -> usize {
        if self.pos < self.chars.len() {
            self.chars[self.pos].0
        } else {
            self.input.len()
        }
    }

    fn advance(&mut self) -> Option<char> {
        if self.pos < self.chars.len() {
            let (_, c) = self.chars[self.pos];
            self.pos += 1;
            if c == '\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
            Some(c)
        } else {
            None
        }
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            // skip whitespace
            while let Some(c) = self.peek() {
                if c.is_whitespace() {
                    self.advance();
                } else {
                    break;
                }
            }

            // check for comments
            if self.peek() == Some('/') && self.peek_offset(1) == Some('/') {
                // Line comment
                self.advance(); // '/'
                self.advance(); // '/'
                while let Some(c) = self.peek() {
                    self.advance();
                    if c == '\n' {
                        break;
                    }
                }
            } else if self.peek() == Some('/') && self.peek_offset(1) == Some('*') {
                // Block comment
                self.advance(); // '/'
                self.advance(); // '*'
                while let Some(c) = self.peek() {
                    if c == '*' && self.peek_offset(1) == Some('/') {
                        self.advance(); // '*'
                        self.advance(); // '/'
                        break;
                    }
                    self.advance();
                }
            } else {
                break;
            }
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token()?;
            let is_eof = tok.kind == TokenKind::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    pub fn next_token(&mut self) -> Result<Token, String> {
        self.skip_whitespace_and_comments();

        let start_byte = self.current_byte_idx();
        let start_line = self.line;
        let start_col = self.col;

        let c = match self.peek() {
            Some(ch) => ch,
            None => {
                let span = Span::new(start_byte, start_byte, start_line, start_col);
                return Ok(Token::new(TokenKind::Eof, span));
            }
        };

        match self.current_mode() {
            LexerMode::Normal => self.next_normal_token(c, start_byte, start_line, start_col),
            LexerMode::TemplateTag => self.next_template_tag_token(c, start_byte, start_line, start_col),
            LexerMode::TemplateContent => self.next_template_content_token(c, start_byte, start_line, start_col),
        }
    }

    fn next_normal_token(
        &mut self,
        c: char,
        start_byte: usize,
        start_line: usize,
        start_col: usize,
    ) -> Result<Token, String> {
        let kind = match c {
            '{' => {
                self.advance();
                if self.just_saw_render {
                    self.just_saw_render = false;
                    self.mode_stack.push(LexerMode::TemplateContent);
                } else if !self.brace_depths.is_empty() {
                    let last = self.brace_depths.last_mut().unwrap();
                    *last += 1;
                }
                TokenKind::OpenBrace
            }
            '}' => {
                self.advance();
                if !self.brace_depths.is_empty() {
                    let last = self.brace_depths.last_mut().unwrap();
                    if *last == 1 {
                        self.brace_depths.pop();
                        self.mode_stack.pop(); // return to template mode
                    } else {
                        *last -= 1;
                    }
                }
                TokenKind::CloseBrace
            }
            '(' => { self.advance(); TokenKind::OpenParen }
            ')' => { self.advance(); TokenKind::CloseParen }
            '[' => { self.advance(); TokenKind::OpenBracket }
            ']' => { self.advance(); TokenKind::CloseBracket }
            ':' => { self.advance(); TokenKind::Colon }
            ';' => { self.advance(); TokenKind::Semicolon }
            ',' => { self.advance(); TokenKind::Comma }
            '.' => { self.advance(); TokenKind::Dot }
            '+' => {
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::PlusEq
                } else {
                    TokenKind::Plus
                }
            }
            '-' => {
                self.advance();
                if self.peek() == Some('>') {
                    self.advance();
                    TokenKind::Arrow
                } else if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::MinusEq
                } else {
                    TokenKind::Minus
                }
            }
            '*' => {
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::StarEq
                } else {
                    TokenKind::Star
                }
            }
            '/' => {
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::SlashEq
                } else {
                    TokenKind::Slash
                }
            }
            '%' => { self.advance(); TokenKind::Percent }
            '=' => {
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::EqEq
                } else {
                    TokenKind::Eq
                }
            }
            '!' => {
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::NotEq
                } else {
                    TokenKind::Not
                }
            }
            '<' => {
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::LtEq
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::GtEq
                } else {
                    TokenKind::Gt
                }
            }
            '&' => {
                self.advance();
                if self.peek() == Some('&') {
                    self.advance();
                    TokenKind::AndAnd
                } else {
                    return Err(format!("Unexpected character '&' at {}:{}", start_line, start_col));
                }
            }
            '|' => {
                self.advance();
                if self.peek() == Some('|') {
                    self.advance();
                    TokenKind::OrOr
                } else {
                    return Err(format!("Unexpected character '|' at {}:{}", start_line, start_col));
                }
            }
            '"' => self.lex_string_literal(start_line, start_col)?,
            _ if c.is_ascii_digit() => self.lex_number()?,
            _ if c.is_alphabetic() || c == '_' => self.lex_ident_or_keyword(),
            _ => return Err(format!("Unexpected character '{}' at {}:{}", c, start_line, start_col)),
        };

        let end_byte = self.current_byte_idx();
        let span = Span::new(start_byte, end_byte, start_line, start_col);
        Ok(Token::new(kind, span))
    }

    fn next_template_tag_token(
        &mut self,
        c: char,
        start_byte: usize,
        start_line: usize,
        start_col: usize,
    ) -> Result<Token, String> {
        let kind = match c {
            '/' if self.peek_offset(1) == Some('>') => {
                self.advance(); // '/'
                self.advance(); // '>'
                self.mode_stack.pop(); // back to TemplateContent
                TokenKind::TagSelfClose
            }
            '>' => {
                self.advance();
                self.mode_stack.pop(); // back to TemplateContent
                TokenKind::TagEnd
            }
            '=' => {
                self.advance();
                TokenKind::Eq
            }
            '@' => {
                self.advance(); // '@'
                let mut name = String::new();
                while let Some(ch) = self.peek() {
                    if ch.is_alphanumeric() || ch == '-' || ch == '_' {
                        name.push(ch);
                        self.advance();
                    } else {
                        break;
                    }
                }
                TokenKind::AtEvent(name)
            }
            '"' => self.lex_string_literal(start_line, start_col)?,
            '{' => {
                self.advance();
                self.mode_stack.push(LexerMode::Normal);
                self.brace_depths.push(1);
                TokenKind::OpenBrace
            }
            _ if c.is_alphabetic() || c == '_' || c == '-' => {
                let mut ident = String::new();
                while let Some(ch) = self.peek() {
                    if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                        ident.push(ch);
                        self.advance();
                    } else {
                        break;
                    }
                }
                TokenKind::Ident(ident)
            }
            _ => return Err(format!("Unexpected character '{}' in template tag at {}:{}", c, start_line, start_col)),
        };

        let end_byte = self.current_byte_idx();
        let span = Span::new(start_byte, end_byte, start_line, start_col);
        Ok(Token::new(kind, span))
    }

    fn next_template_content_token(
        &mut self,
        c: char,
        start_byte: usize,
        start_line: usize,
        start_col: usize,
    ) -> Result<Token, String> {
        if c == '<' && self.peek_offset(1) == Some('/') {
            // Closing tag: </tag>
            self.advance(); // '<'
            self.advance(); // '/'
            self.skip_whitespace_and_comments();
            let mut tag = String::new();
            while let Some(ch) = self.peek() {
                if ch.is_alphanumeric() || ch == '-' || ch == '_' {
                    tag.push(ch);
                    self.advance();
                } else {
                    break;
                }
            }
            self.skip_whitespace_and_comments();
            if self.peek() == Some('>') {
                self.advance(); // '>'
            } else {
                return Err(format!("Expected '>' to close tag </{}> at {}:{}", tag, self.line, self.col));
            }
            let end_byte = self.current_byte_idx();
            let span = Span::new(start_byte, end_byte, start_line, start_col);
            return Ok(Token::new(TokenKind::TagClose(tag), span));
        }

        if c == '<' {
            // Opening tag: <tag
            self.advance(); // '<'
            let mut tag = String::new();
            while let Some(ch) = self.peek() {
                if ch.is_alphanumeric() || ch == '-' || ch == '_' {
                    tag.push(ch);
                    self.advance();
                } else {
                    break;
                }
            }
            self.mode_stack.push(LexerMode::TemplateTag);
            let end_byte = self.current_byte_idx();
            let span = Span::new(start_byte, end_byte, start_line, start_col);
            return Ok(Token::new(TokenKind::TagOpen(tag), span));
        }

        if c == '"' {
            let kind = self.lex_string_literal(start_line, start_col)?;
            let end_byte = self.current_byte_idx();
            let span = Span::new(start_byte, end_byte, start_line, start_col);
            return Ok(Token::new(kind, span));
        }

        if c == '{' {
            self.advance();
            self.mode_stack.push(LexerMode::Normal);
            self.brace_depths.push(1);
            let end_byte = self.current_byte_idx();
            let span = Span::new(start_byte, end_byte, start_line, start_col);
            return Ok(Token::new(TokenKind::OpenBrace, span));
        }

        if c == '}' {
            // End of render block!
            self.advance();
            self.mode_stack.pop(); // back to Normal
            let end_byte = self.current_byte_idx();
            let span = Span::new(start_byte, end_byte, start_line, start_col);
            return Ok(Token::new(TokenKind::CloseBrace, span));
        }

        // Unquoted text content inside element
        let mut text = String::new();
        while let Some(ch) = self.peek() {
            if ch == '<' || ch == '{' || ch == '}' || ch == '"' {
                break;
            }
            text.push(ch);
            self.advance();
        }

        let trimmed = text.trim();
        if !trimmed.is_empty() {
            let end_byte = self.current_byte_idx();
            let span = Span::new(start_byte, end_byte, start_line, start_col);
            return Ok(Token::new(TokenKind::StringLit(trimmed.to_string()), span));
        }

        // If it was just whitespace that ended at a delimiter, recurse
        self.next_token()
    }

    fn lex_string_literal(&mut self, start_line: usize, start_col: usize) -> Result<TokenKind, String> {
        self.advance(); // open quote '"'
        let mut s = String::new();
        while let Some(ch) = self.peek() {
            if ch == '"' {
                self.advance(); // close quote
                return Ok(TokenKind::StringLit(s));
            } else if ch == '\\' {
                self.advance();
                match self.advance() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
                    Some('\\') => s.push('\\'),
                    Some('"') => s.push('"'),
                    Some(other) => s.push(other),
                    None => return Err(format!("Unterminated escape in string at {}:{}", start_line, start_col)),
                }
            } else {
                s.push(ch);
                self.advance();
            }
        }
        Err(format!("Unterminated string literal at {}:{}", start_line, start_col))
    }

    fn lex_number(&mut self) -> Result<TokenKind, String> {
        let mut num_str = String::new();
        let mut is_float = false;

        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                num_str.push(ch);
                self.advance();
            } else if ch == '.' && !is_float && self.peek_offset(1).map_or(false, |c| c.is_ascii_digit()) {
                is_float = true;
                num_str.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        if is_float {
            let f: f64 = num_str.parse().map_err(|e| format!("Invalid float: {}", e))?;
            Ok(TokenKind::FloatLit(f))
        } else {
            let n: i64 = num_str.parse().map_err(|e| format!("Invalid integer: {}", e))?;
            Ok(TokenKind::IntLit(n))
        }
    }

    fn lex_ident_or_keyword(&mut self) -> TokenKind {
        let mut ident = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_alphanumeric() || ch == '_' {
                ident.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        match ident.as_str() {
            "component" => TokenKind::Component,
            "export" => TokenKind::Export,
            "signal" => TokenKind::Signal,
            "computed" => TokenKind::Computed,
            "fn" => TokenKind::Fn,
            "render" => {
                self.just_saw_render = true;
                TokenKind::Render
            }
            "extern" => TokenKind::Extern,
            "let" => TokenKind::Let,
            "mut" => TokenKind::Mut,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "return" => TokenKind::Return,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            _ => TokenKind::Ident(ident),
        }
    }
}
