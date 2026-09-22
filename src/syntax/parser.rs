use crate::syntax::ast::*;
use crate::syntax::token::{Span, Token, TokenKind};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        if self.pos < self.tokens.len() {
            &self.tokens[self.pos]
        } else {
            self.tokens.last().unwrap()
        }
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn advance(&mut self) -> Token {
        let tok = self.peek().clone();
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, expected: TokenKind) -> Result<Token, String> {
        let tok = self.peek();
        if tok.kind == expected {
            Ok(self.advance())
        } else {
            Err(format!(
                "Expected '{}', but found '{}' at {}:{}",
                expected, tok.kind, tok.span.line, tok.span.col
            ))
        }
    }

    fn expect_ident(&mut self) -> Result<(String, Span), String> {
        let tok = self.advance();
        if let TokenKind::Ident(name) = tok.kind {
            Ok((name, tok.span))
        } else {
            Err(format!(
                "Expected identifier, but found '{}' at {}:{}",
                tok.kind, tok.span.line, tok.span.col
            ))
        }
    }

    pub fn parse_program(&mut self) -> Result<Program, String> {
        let mut items = Vec::new();
        while self.peek_kind() != &TokenKind::Eof {
            match self.peek_kind() {
                TokenKind::Extern => {
                    items.push(TopLevelItem::ExternBlock(self.parse_extern_block()?));
                }
                TokenKind::Export | TokenKind::Component => {
                    items.push(TopLevelItem::Component(self.parse_component()?));
                }
                other => {
                    let span = self.peek().span;
                    return Err(format!(
                        "Unexpected token '{}' at top level at {}:{}",
                        other, span.line, span.col
                    ));
                }
            }
        }
        Ok(Program { items })
    }

    fn parse_extern_block(&mut self) -> Result<ExternBlock, String> {
        let start_span = self.expect(TokenKind::Extern)?.span;
        let abi = match self.advance().kind {
            TokenKind::StringLit(s) => s,
            other => return Err(format!("Expected ABI string after extern, found '{}'", other)),
        };

        self.expect(TokenKind::OpenBrace)?;
        let mut functions = Vec::new();

        while self.peek_kind() != &TokenKind::CloseBrace && self.peek_kind() != &TokenKind::Eof {
            let fn_span = self.expect(TokenKind::Fn)?.span;
            let (name, _) = self.expect_ident()?;
            self.expect(TokenKind::OpenParen)?;
            let mut params = Vec::new();

            while self.peek_kind() != &TokenKind::CloseParen && self.peek_kind() != &TokenKind::Eof {
                let (param_name, _) = self.expect_ident()?;
                self.expect(TokenKind::Colon)?;
                let param_type = self.parse_type()?;
                params.push((param_name, param_type));

                if self.peek_kind() == &TokenKind::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
            self.expect(TokenKind::CloseParen)?;

            let return_type = if self.peek_kind() == &TokenKind::Arrow {
                self.advance();
                Some(self.parse_type()?)
            } else {
                None
            };

            self.expect(TokenKind::Semicolon)?;

            functions.push(ExternFnDecl {
                name,
                params,
                return_type,
                span: fn_span,
            });
        }

        let end_span = self.expect(TokenKind::CloseBrace)?.span;
        let span = Span::new(start_span.start, end_span.end, start_span.line, start_span.col);

        Ok(ExternBlock { abi, functions, span })
    }

    fn parse_component(&mut self) -> Result<ComponentDecl, String> {
        let start_span = self.peek().span;
        let is_exported = if self.peek_kind() == &TokenKind::Export {
            self.advance();
            true
        } else {
            false
        };

        self.expect(TokenKind::Component)?;
        let (name, _) = self.expect_ident()?;
        self.expect(TokenKind::OpenBrace)?;

        let mut signals = Vec::new();
        let mut computeds = Vec::new();
        let mut methods = Vec::new();
        let mut render = None;

        while self.peek_kind() != &TokenKind::CloseBrace && self.peek_kind() != &TokenKind::Eof {
            match self.peek_kind() {
                TokenKind::Signal => {
                    signals.push(self.parse_signal()?);
                }
                TokenKind::Computed => {
                    computeds.push(self.parse_computed()?);
                }
                TokenKind::Fn => {
                    methods.push(self.parse_fn()?);
                }
                TokenKind::Render => {
                    if render.is_some() {
                        return Err("Component can only have one render block".into());
                    }
                    render = Some(self.parse_render()?);
                }
                other => {
                    let span = self.peek().span;
                    return Err(format!(
                        "Unexpected token '{}' in component body at {}:{}",
                        other, span.line, span.col
                    ));
                }
            }
        }

        let end_span = self.expect(TokenKind::CloseBrace)?.span;
        let span = Span::new(start_span.start, end_span.end, start_span.line, start_span.col);

        Ok(ComponentDecl {
            name,
            is_exported,
            signals,
            computeds,
            methods,
            render,
            span,
        })
    }

    fn parse_signal(&mut self) -> Result<SignalDecl, String> {
        let start_span = self.expect(TokenKind::Signal)?.span;
        let (name, _) = self.expect_ident()?;
        self.expect(TokenKind::Colon)?;
        let type_ann = self.parse_type()?;
        self.expect(TokenKind::Eq)?;
        let init_expr = self.parse_expr(0)?;
        let end_span = self.expect(TokenKind::Semicolon)?.span;

        let span = Span::new(start_span.start, end_span.end, start_span.line, start_span.col);
        Ok(SignalDecl {
            name,
            type_ann,
            init_expr,
            span,
        })
    }

    fn parse_computed(&mut self) -> Result<ComputedDecl, String> {
        let start_span = self.expect(TokenKind::Computed)?.span;
        let (name, _) = self.expect_ident()?;
        self.expect(TokenKind::Colon)?;
        let type_ann = self.parse_type()?;
        self.expect(TokenKind::Eq)?;
        let expr = self.parse_expr(0)?;
        let end_span = self.expect(TokenKind::Semicolon)?.span;

        let span = Span::new(start_span.start, end_span.end, start_span.line, start_span.col);
        Ok(ComputedDecl {
            name,
            type_ann,
            expr,
            span,
        })
    }

    fn parse_fn(&mut self) -> Result<FnDecl, String> {
        let start_span = self.expect(TokenKind::Fn)?.span;
        let (name, _) = self.expect_ident()?;
        self.expect(TokenKind::OpenParen)?;
        let mut params = Vec::new();

        while self.peek_kind() != &TokenKind::CloseParen && self.peek_kind() != &TokenKind::Eof {
            let (param_name, _) = self.expect_ident()?;
            self.expect(TokenKind::Colon)?;
            let param_type = self.parse_type()?;
            params.push((param_name, param_type));

            if self.peek_kind() == &TokenKind::Comma {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(TokenKind::CloseParen)?;

        let return_type = if self.peek_kind() == &TokenKind::Arrow {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        self.expect(TokenKind::OpenBrace)?;
        let mut body = Vec::new();

        while self.peek_kind() != &TokenKind::CloseBrace && self.peek_kind() != &TokenKind::Eof {
            body.push(self.parse_stmt()?);
        }

        let end_span = self.expect(TokenKind::CloseBrace)?.span;
        let span = Span::new(start_span.start, end_span.end, start_span.line, start_span.col);

        Ok(FnDecl {
            name,
            params,
            return_type,
            body,
            span,
        })
    }

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        match self.peek_kind() {
            TokenKind::Let => {
                let start_span = self.advance().span;
                let is_mut = if self.peek_kind() == &TokenKind::Mut {
                    self.advance();
                    true
                } else {
                    false
                };
                let (name, _) = self.expect_ident()?;
                let type_ann = if self.peek_kind() == &TokenKind::Colon {
                    self.advance();
                    Some(self.parse_type()?)
                } else {
                    None
                };
                self.expect(TokenKind::Eq)?;
                let init = self.parse_expr(0)?;
                let end_span = self.expect(TokenKind::Semicolon)?.span;
                let span = Span::new(start_span.start, end_span.end, start_span.line, start_span.col);
                Ok(Stmt::Let {
                    name,
                    type_ann,
                    is_mut,
                    init,
                    span,
                })
            }
            TokenKind::If => {
                let start_span = self.advance().span;
                let cond = self.parse_expr(0)?;
                self.expect(TokenKind::OpenBrace)?;
                let mut then_branch = Vec::new();
                while self.peek_kind() != &TokenKind::CloseBrace && self.peek_kind() != &TokenKind::Eof {
                    then_branch.push(self.parse_stmt()?);
                }
                let mut end_span = self.expect(TokenKind::CloseBrace)?.span;

                let else_branch = if self.peek_kind() == &TokenKind::Else {
                    self.advance();
                    self.expect(TokenKind::OpenBrace)?;
                    let mut else_stmts = Vec::new();
                    while self.peek_kind() != &TokenKind::CloseBrace && self.peek_kind() != &TokenKind::Eof {
                        else_stmts.push(self.parse_stmt()?);
                    }
                    end_span = self.expect(TokenKind::CloseBrace)?.span;
                    Some(else_stmts)
                } else {
                    None
                };

                let span = Span::new(start_span.start, end_span.end, start_span.line, start_span.col);
                Ok(Stmt::If {
                    cond,
                    then_branch,
                    else_branch,
                    span,
                })
            }
            TokenKind::Return => {
                let start_span = self.advance().span;
                let expr = if self.peek_kind() != &TokenKind::Semicolon {
                    Some(self.parse_expr(0)?)
                } else {
                    None
                };
                let end_span = self.expect(TokenKind::Semicolon)?.span;
                let span = Span::new(start_span.start, end_span.end, start_span.line, start_span.col);
                Ok(Stmt::Return(expr, span))
            }
            TokenKind::Ident(_) => {
                // Check if this is an assignment: ident = expr; or ident += expr;
                if self.pos + 1 < self.tokens.len() {
                    let next_kind = &self.tokens[self.pos + 1].kind;
                    match next_kind {
                        TokenKind::Eq => {
                            let (target, start_span) = self.expect_ident()?;
                            self.advance(); // Eq
                            let value = self.parse_expr(0)?;
                            let end_span = self.expect(TokenKind::Semicolon)?.span;
                            let span = Span::new(start_span.start, end_span.end, start_span.line, start_span.col);
                            return Ok(Stmt::Assign { target, value, span });
                        }
                        TokenKind::PlusEq | TokenKind::MinusEq | TokenKind::StarEq | TokenKind::SlashEq => {
                            let (target, start_span) = self.expect_ident()?;
                            let op_tok = self.advance();
                            let op = match op_tok.kind {
                                TokenKind::PlusEq => AssignOperator::AddEq,
                                TokenKind::MinusEq => AssignOperator::SubEq,
                                TokenKind::StarEq => AssignOperator::MulEq,
                                TokenKind::SlashEq => AssignOperator::DivEq,
                                _ => unreachable!(),
                            };
                            let value = self.parse_expr(0)?;
                            let end_span = self.expect(TokenKind::Semicolon)?.span;
                            let span = Span::new(start_span.start, end_span.end, start_span.line, start_span.col);
                            return Ok(Stmt::AssignOp {
                                target,
                                op,
                                value,
                                span,
                            });
                        }
                        _ => {}
                    }
                }
                // Otherwise it's an expression statement
                let expr = self.parse_expr(0)?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Stmt::Expr(expr))
            }
            _ => {
                let expr = self.parse_expr(0)?;
                self.expect(TokenKind::Semicolon)?;
                Ok(Stmt::Expr(expr))
            }
        }
    }

    fn parse_render(&mut self) -> Result<RenderBlock, String> {
        let start_span = self.expect(TokenKind::Render)?.span;
        self.expect(TokenKind::OpenBrace)?;
        let root = self.parse_template_node()?;
        let end_span = self.expect(TokenKind::CloseBrace)?.span;
        let span = Span::new(start_span.start, end_span.end, start_span.line, start_span.col);
        Ok(RenderBlock { root, span })
    }

    fn parse_template_node(&mut self) -> Result<TemplateNode, String> {
        match self.peek_kind() {
            TokenKind::TagOpen(_) => {
                let elem = self.parse_element()?;
                Ok(TemplateNode::Element(elem))
            }
            TokenKind::StringLit(s) => {
                let text = s.clone();
                let span = self.advance().span;
                Ok(TemplateNode::Text(text, span))
            }
            TokenKind::OpenBrace => {
                let start_span = self.advance().span;
                let expr = self.parse_expr(0)?;
                let end_span = self.expect(TokenKind::CloseBrace)?.span;
                let span = Span::new(start_span.start, end_span.end, start_span.line, start_span.col);
                Ok(TemplateNode::DynamicExpr(expr, span))
            }
            other => {
                let span = self.peek().span;
                Err(format!(
                    "Unexpected token '{}' in template at {}:{}",
                    other, span.line, span.col
                ))
            }
        }
    }

    fn parse_element(&mut self) -> Result<ElementNode, String> {
        let tok = self.advance();
        let (tag, start_span) = match tok.kind {
            TokenKind::TagOpen(t) => (t, tok.span),
            _ => unreachable!(),
        };

        let mut attributes = Vec::new();
        let mut events = Vec::new();

        // Parse attributes and events inside the tag
        while self.peek_kind() != &TokenKind::TagEnd
            && self.peek_kind() != &TokenKind::TagSelfClose
            && self.peek_kind() != &TokenKind::Eof
        {
            match self.peek_kind() {
                TokenKind::AtEvent(ev_name) => {
                    let ev_name = ev_name.clone();
                    let ev_span = self.advance().span;
                    self.expect(TokenKind::Eq)?;

                    // Handler can be an ident or {ident}
                    let handler = if self.peek_kind() == &TokenKind::OpenBrace {
                        self.advance();
                        let (h, _) = self.expect_ident()?;
                        self.expect(TokenKind::CloseBrace)?;
                        h
                    } else {
                        let (h, _) = self.expect_ident()?;
                        h
                    };

                    events.push(EventBinding {
                        event_name: ev_name,
                        handler,
                        span: ev_span,
                    });
                }
                TokenKind::Ident(attr_name) => {
                    let attr_name = attr_name.clone();
                    let attr_span = self.advance().span;
                    self.expect(TokenKind::Eq)?;

                    let val = match self.peek_kind() {
                        TokenKind::StringLit(s) => {
                            let val_str = s.clone();
                            self.advance();
                            AttributeValue::Static(val_str)
                        }
                        TokenKind::OpenBrace => {
                            self.advance();
                            let expr = self.parse_expr(0)?;
                            self.expect(TokenKind::CloseBrace)?;
                            AttributeValue::Dynamic(expr)
                        }
                        other => {
                            let span = self.peek().span;
                            return Err(format!(
                                "Expected attribute value string or {{expr}}, found '{}' at {}:{}",
                                other, span.line, span.col
                            ));
                        }
                    };

                    attributes.push(Attribute {
                        name: attr_name,
                        value: val,
                        span: attr_span,
                    });
                }
                other => {
                    let span = self.peek().span;
                    return Err(format!(
                        "Unexpected token in tag '<{}': '{}' at {}:{}",
                        tag, other, span.line, span.col
                    ));
                }
            }
        }

        if self.peek_kind() == &TokenKind::TagSelfClose {
            let end_span = self.advance().span;
            let span = Span::new(start_span.start, end_span.end, start_span.line, start_span.col);
            return Ok(ElementNode {
                tag,
                attributes,
                events,
                children: Vec::new(),
                span,
            });
        }

        self.expect(TokenKind::TagEnd)?;

        // Parse children
        let mut children = Vec::new();
        while self.peek_kind() != &TokenKind::Eof {
            if let TokenKind::TagClose(ref close_tag) = self.peek_kind() {
                if close_tag == &tag {
                    break;
                } else {
                    return Err(format!(
                        "Mismatched closing tag: expected '</{}>', found '</{}>'",
                        tag, close_tag
                    ));
                }
            }
            children.push(self.parse_template_node()?);
        }

        let end_span = self.expect(TokenKind::TagClose(tag.clone()))?.span;
        let span = Span::new(start_span.start, end_span.end, start_span.line, start_span.col);

        Ok(ElementNode {
            tag,
            attributes,
            events,
            children,
            span,
        })
    }

    fn parse_type(&mut self) -> Result<Type, String> {
        let (name, _span) = self.expect_ident()?;
        match name.as_str() {
            "i32" => Ok(Type::I32),
            "i64" => Ok(Type::I64),
            "u32" => Ok(Type::U32),
            "u64" => Ok(Type::U64),
            "f32" => Ok(Type::F32),
            "f64" => Ok(Type::F64),
            "bool" => Ok(Type::Bool),
            "string" => Ok(Type::String),
            "void" => Ok(Type::Void),
            "Option" => {
                self.expect(TokenKind::Lt)?;
                let inner = self.parse_type()?;
                self.expect(TokenKind::Gt)?;
                Ok(Type::Option(Box::new(inner)))
            }
            "Result" => {
                self.expect(TokenKind::Lt)?;
                let ok = self.parse_type()?;
                self.expect(TokenKind::Comma)?;
                let err = self.parse_type()?;
                self.expect(TokenKind::Gt)?;
                Ok(Type::Result(Box::new(ok), Box::new(err)))
            }
            _ => Ok(Type::Custom(name)),
        }
    }

    // Pratt parser for binary expressions
    fn parse_expr(&mut self, min_prec: u8) -> Result<Expr, String> {
        let mut left = self.parse_primary()?;

        while let Some((op, prec)) = self.current_binary_op() {
            if prec < min_prec {
                break;
            }
            self.advance(); // consume op
            let right = self.parse_expr(prec + 1)?;
            let span = Span::new(
                left.span().start,
                right.span().end,
                left.span().line,
                left.span().col,
            );
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }

        Ok(left)
    }

    fn current_binary_op(&self) -> Option<(BinaryOp, u8)> {
        match self.peek_kind() {
            TokenKind::OrOr => Some((BinaryOp::Or, 1)),
            TokenKind::AndAnd => Some((BinaryOp::And, 2)),
            TokenKind::EqEq => Some((BinaryOp::Eq, 3)),
            TokenKind::NotEq => Some((BinaryOp::NotEq, 3)),
            TokenKind::Lt => Some((BinaryOp::Lt, 4)),
            TokenKind::LtEq => Some((BinaryOp::LtEq, 4)),
            TokenKind::Gt => Some((BinaryOp::Gt, 4)),
            TokenKind::GtEq => Some((BinaryOp::GtEq, 4)),
            TokenKind::Plus => Some((BinaryOp::Add, 5)),
            TokenKind::Minus => Some((BinaryOp::Sub, 5)),
            TokenKind::Star => Some((BinaryOp::Mul, 6)),
            TokenKind::Slash => Some((BinaryOp::Div, 6)),
            TokenKind::Percent => Some((BinaryOp::Mod, 6)),
            _ => None,
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.peek_kind().clone() {
            TokenKind::IntLit(n) => {
                let span = self.advance().span;
                Ok(Expr::IntLit(n, span))
            }
            TokenKind::FloatLit(n) => {
                let span = self.advance().span;
                Ok(Expr::FloatLit(n, span))
            }
            TokenKind::StringLit(s) => {
                let span = self.advance().span;
                Ok(Expr::StringLit(s, span))
            }
            TokenKind::True => {
                let span = self.advance().span;
                Ok(Expr::BoolLit(true, span))
            }
            TokenKind::False => {
                let span = self.advance().span;
                Ok(Expr::BoolLit(false, span))
            }
            TokenKind::Minus => {
                let op_span = self.advance().span;
                let expr = self.parse_primary()?;
                let span = Span::new(op_span.start, expr.span().end, op_span.line, op_span.col);
                Ok(Expr::Unary {
                    op: UnaryOp::Neg,
                    expr: Box::new(expr),
                    span,
                })
            }
            TokenKind::Not => {
                let op_span = self.advance().span;
                let expr = self.parse_primary()?;
                let span = Span::new(op_span.start, expr.span().end, op_span.line, op_span.col);
                Ok(Expr::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                    span,
                })
            }
            TokenKind::OpenParen => {
                self.advance();
                let expr = self.parse_expr(0)?;
                self.expect(TokenKind::CloseParen)?;
                Ok(expr)
            }
            TokenKind::Ident(name) => {
                let span = self.advance().span;
                if self.peek_kind() == &TokenKind::OpenParen {
                    // Function call: callee(args...)
                    self.advance();
                    let mut args = Vec::new();
                    while self.peek_kind() != &TokenKind::CloseParen && self.peek_kind() != &TokenKind::Eof {
                        args.push(self.parse_expr(0)?);
                        if self.peek_kind() == &TokenKind::Comma {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    let end_span = self.expect(TokenKind::CloseParen)?.span;
                    let call_span = Span::new(span.start, end_span.end, span.line, span.col);
                    Ok(Expr::Call {
                        callee: name,
                        args,
                        span: call_span,
                    })
                } else if self.peek_kind() == &TokenKind::Dot {
                    // Member access: ident.member
                    self.advance();
                    let (member, end_span) = self.expect_ident()?;
                    let acc_span = Span::new(span.start, end_span.end, span.line, span.col);
                    Ok(Expr::MemberAccess {
                        object: Box::new(Expr::Ident(name, span)),
                        member,
                        span: acc_span,
                    })
                } else {
                    Ok(Expr::Ident(name, span))
                }
            }
            other => {
                let span = self.peek().span;
                Err(format!(
                    "Unexpected token '{}' in expression at {}:{}",
                    other, span.line, span.col
                ))
            }
        }
    }
}
