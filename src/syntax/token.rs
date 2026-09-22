use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub col: usize,
}

impl Span {
    pub fn new(start: usize, end: usize, line: usize, col: usize) -> Self {
        Self { start, end, line, col }
    }

    pub fn dummy() -> Self {
        Self { start: 0, end: 0, line: 1, col: 1 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Keywords
    Component,
    Export,
    Signal,
    Computed,
    Fn,
    Render,
    Extern,
    Let,
    Mut,
    If,
    Else,
    Return,
    True,
    False,

    // Literals & Identifiers
    Ident(String),
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),

    // Template specific
    AtEvent(String),       // e.g. "@click", "@input"
    TagOpen(String),       // e.g. "<div", "<button"
    TagClose(String),      // e.g. "</div>", "</button>"
    TagEnd,                // ">"
    TagSelfClose,          // "/>"

    // Operators
    Plus,                  // "+"
    Minus,                 // "-"
    Star,                  // "*"
    Slash,                 // "/"
    Percent,               // "%"
    Eq,                    // "="
    PlusEq,                // "+="
    MinusEq,               // "-="
    StarEq,                // "*="
    SlashEq,               // "/="
    EqEq,                  // "=="
    NotEq,                 // "!="
    Lt,                    // "<"
    LtEq,                  // "<="
    Gt,                    // ">"
    GtEq,                  // ">="
    AndAnd,                // "&&"
    OrOr,                  // "||"
    Not,                   // "!"

    // Delimiters
    OpenBrace,             // "{"
    CloseBrace,            // "}"
    OpenParen,             // "("
    CloseParen,            // ")"
    OpenBracket,           // "["
    CloseBracket,          // "]"
    Colon,                 // ":"
    Semicolon,             // ";"
    Comma,                 // ","
    Arrow,                 // "->"
    Dot,                   // "."

    Eof,
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenKind::Component => write!(f, "component"),
            TokenKind::Export => write!(f, "export"),
            TokenKind::Signal => write!(f, "signal"),
            TokenKind::Computed => write!(f, "computed"),
            TokenKind::Fn => write!(f, "fn"),
            TokenKind::Render => write!(f, "render"),
            TokenKind::Extern => write!(f, "extern"),
            TokenKind::Let => write!(f, "let"),
            TokenKind::Mut => write!(f, "mut"),
            TokenKind::If => write!(f, "if"),
            TokenKind::Else => write!(f, "else"),
            TokenKind::Return => write!(f, "return"),
            TokenKind::True => write!(f, "true"),
            TokenKind::False => write!(f, "false"),
            TokenKind::Ident(s) => write!(f, "{}", s),
            TokenKind::IntLit(n) => write!(f, "{}", n),
            TokenKind::FloatLit(n) => write!(f, "{}", n),
            TokenKind::StringLit(s) => write!(f, "\"{}\"", s),
            TokenKind::AtEvent(ev) => write!(f, "{}", ev),
            TokenKind::TagOpen(t) => write!(f, "<{}", t),
            TokenKind::TagClose(t) => write!(f, "</{}>", t),
            TokenKind::TagEnd => write!(f, ">"),
            TokenKind::TagSelfClose => write!(f, "/>"),
            TokenKind::Plus => write!(f, "+"),
            TokenKind::Minus => write!(f, "-"),
            TokenKind::Star => write!(f, "*"),
            TokenKind::Slash => write!(f, "/"),
            TokenKind::Percent => write!(f, "%"),
            TokenKind::Eq => write!(f, "="),
            TokenKind::PlusEq => write!(f, "+="),
            TokenKind::MinusEq => write!(f, "-="),
            TokenKind::StarEq => write!(f, "*="),
            TokenKind::SlashEq => write!(f, "/="),
            TokenKind::EqEq => write!(f, "=="),
            TokenKind::NotEq => write!(f, "!="),
            TokenKind::Lt => write!(f, "<"),
            TokenKind::LtEq => write!(f, "<="),
            TokenKind::Gt => write!(f, ">"),
            TokenKind::GtEq => write!(f, ">="),
            TokenKind::AndAnd => write!(f, "&&"),
            TokenKind::OrOr => write!(f, "||"),
            TokenKind::Not => write!(f, "!"),
            TokenKind::OpenBrace => write!(f, "{{"),
            TokenKind::CloseBrace => write!(f, "}}"),
            TokenKind::OpenParen => write!(f, "("),
            TokenKind::CloseParen => write!(f, ")"),
            TokenKind::OpenBracket => write!(f, "["),
            TokenKind::CloseBracket => write!(f, "]"),
            TokenKind::Colon => write!(f, ":"),
            TokenKind::Semicolon => write!(f, ";"),
            TokenKind::Comma => write!(f, ","),
            TokenKind::Arrow => write!(f, "->"),
            TokenKind::Dot => write!(f, "."),
            TokenKind::Eof => write!(f, "<EOF>"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}
