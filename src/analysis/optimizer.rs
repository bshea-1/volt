use crate::syntax::ast::*;

/// Ahead-of-time AST optimizer performing constant folding and algebraic simplification.
pub struct AstOptimizer;

impl AstOptimizer {
    pub fn optimize_component(mut comp: ComponentDecl) -> ComponentDecl {
        // Optimize signal initializers
        for sig in &mut comp.signals {
            sig.init_expr = Self::optimize_expr(&sig.init_expr);
        }

        // Optimize computed expressions
        for comp_decl in &mut comp.computeds {
            comp_decl.expr = Self::optimize_expr(&comp_decl.expr);
        }

        // Optimize method bodies
        for method in &mut comp.methods {
            method.body = method
                .body
                .iter()
                .map(Self::optimize_stmt)
                .collect();
        }

        // Optimize render template
        if let Some(ref mut render) = comp.render {
            render.root = Self::optimize_template_node(&render.root);
        }

        comp
    }

    pub fn optimize_stmt(stmt: &Stmt) -> Stmt {
        match stmt {
            Stmt::Let {
                name,
                type_ann,
                is_mut,
                init,
                span,
            } => Stmt::Let {
                name: name.clone(),
                type_ann: type_ann.clone(),
                is_mut: *is_mut,
                init: Self::optimize_expr(init),
                span: *span,
            },
            Stmt::Assign {
                target,
                value,
                span,
            } => Stmt::Assign {
                target: target.clone(),
                value: Self::optimize_expr(value),
                span: *span,
            },
            Stmt::AssignOp {
                target,
                op,
                value,
                span,
            } => Stmt::AssignOp {
                target: target.clone(),
                op: *op,
                value: Self::optimize_expr(value),
                span: *span,
            },
            Stmt::Expr(expr) => Stmt::Expr(Self::optimize_expr(expr)),
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                span,
            } => Stmt::If {
                cond: Self::optimize_expr(cond),
                then_branch: then_branch.iter().map(Self::optimize_stmt).collect(),
                else_branch: else_branch
                    .as_ref()
                    .map(|b| b.iter().map(Self::optimize_stmt).collect()),
                span: *span,
            },
            Stmt::Return(expr, span) => {
                Stmt::Return(expr.as_ref().map(Self::optimize_expr), *span)
            }
        }
    }

    pub fn optimize_template_node(node: &TemplateNode) -> TemplateNode {
        match node {
            TemplateNode::Element(elem) => {
                let mut optimized_elem = elem.clone();
                optimized_elem.children = elem
                    .children
                    .iter()
                    .map(Self::optimize_template_node)
                    .collect();
                TemplateNode::Element(optimized_elem)
            }
            TemplateNode::Text(t, span) => TemplateNode::Text(t.clone(), *span),
            TemplateNode::DynamicExpr(expr, span) => {
                TemplateNode::DynamicExpr(Self::optimize_expr(expr), *span)
            }
        }
    }

    pub fn optimize_expr(expr: &Expr) -> Expr {
        match expr {
            Expr::Binary {
                op,
                left,
                right,
                span,
            } => {
                let opt_left = Self::optimize_expr(left);
                let opt_right = Self::optimize_expr(right);

                // Constant folding for integers
                if let (Expr::IntLit(a, _), Expr::IntLit(b, _)) = (&opt_left, &opt_right) {
                    match op {
                        BinaryOp::Add => return Expr::IntLit(a.wrapping_add(*b), *span),
                        BinaryOp::Sub => return Expr::IntLit(a.wrapping_sub(*b), *span),
                        BinaryOp::Mul => return Expr::IntLit(a.wrapping_mul(*b), *span),
                        BinaryOp::Div if *b != 0 => return Expr::IntLit(a.wrapping_div(*b), *span),
                        BinaryOp::Mod if *b != 0 => return Expr::IntLit(a.wrapping_rem(*b), *span),
                        BinaryOp::Eq => return Expr::BoolLit(a == b, *span),
                        BinaryOp::NotEq => return Expr::BoolLit(a != b, *span),
                        BinaryOp::Lt => return Expr::BoolLit(a < b, *span),
                        BinaryOp::LtEq => return Expr::BoolLit(a <= b, *span),
                        BinaryOp::Gt => return Expr::BoolLit(a > b, *span),
                        BinaryOp::GtEq => return Expr::BoolLit(a >= b, *span),
                        _ => {}
                    }
                }

                // Constant folding for booleans
                if let (Expr::BoolLit(a, _), Expr::BoolLit(b, _)) = (&opt_left, &opt_right) {
                    match op {
                        BinaryOp::And => return Expr::BoolLit(*a && *b, *span),
                        BinaryOp::Or => return Expr::BoolLit(*a || *b, *span),
                        BinaryOp::Eq => return Expr::BoolLit(a == b, *span),
                        BinaryOp::NotEq => return Expr::BoolLit(a != b, *span),
                        _ => {}
                    }
                }

                // Algebraic identities
                match op {
                    // x + 0 => x, 0 + x => x
                    BinaryOp::Add => {
                        if let Expr::IntLit(0, _) = opt_right {
                            return opt_left;
                        }
                        if let Expr::IntLit(0, _) = opt_left {
                            return opt_right;
                        }
                    }
                    // x - 0 => x
                    BinaryOp::Sub => {
                        if let Expr::IntLit(0, _) = opt_right {
                            return opt_left;
                        }
                    }
                    // x * 1 => x, 1 * x => x
                    BinaryOp::Mul => {
                        if let Expr::IntLit(1, _) = opt_right {
                            return opt_left;
                        }
                        if let Expr::IntLit(1, _) = opt_left {
                            return opt_right;
                        }
                        // x * 0 => 0, 0 * x => 0
                        if let Expr::IntLit(0, s) = opt_right {
                            return Expr::IntLit(0, s);
                        }
                        if let Expr::IntLit(0, s) = opt_left {
                            return Expr::IntLit(0, s);
                        }
                    }
                    // x / 1 => x
                    BinaryOp::Div => {
                        if let Expr::IntLit(1, _) = opt_right {
                            return opt_left;
                        }
                    }
                    // x && true => x, true && x => x
                    BinaryOp::And => {
                        if let Expr::BoolLit(true, _) = opt_right {
                            return opt_left;
                        }
                        if let Expr::BoolLit(true, _) = opt_left {
                            return opt_right;
                        }
                        // x && false => false, false && x => false
                        if let Expr::BoolLit(false, s) = opt_right {
                            return Expr::BoolLit(false, s);
                        }
                        if let Expr::BoolLit(false, s) = opt_left {
                            return Expr::BoolLit(false, s);
                        }
                    }
                    // x || false => x, false || x => x
                    BinaryOp::Or => {
                        if let Expr::BoolLit(false, _) = opt_right {
                            return opt_left;
                        }
                        if let Expr::BoolLit(false, _) = opt_left {
                            return opt_right;
                        }
                        // x || true => true, true || x => true
                        if let Expr::BoolLit(true, s) = opt_right {
                            return Expr::BoolLit(true, s);
                        }
                        if let Expr::BoolLit(true, s) = opt_left {
                            return Expr::BoolLit(true, s);
                        }
                    }
                    _ => {}
                }

                Expr::Binary {
                    op: *op,
                    left: Box::new(opt_left),
                    right: Box::new(opt_right),
                    span: *span,
                }
            }
            Expr::Unary { op, expr, span } => {
                let opt_expr = Self::optimize_expr(expr);
                match (op, &opt_expr) {
                    (UnaryOp::Neg, Expr::IntLit(val, s)) => Expr::IntLit(-val, *s),
                    (UnaryOp::Not, Expr::BoolLit(val, s)) => Expr::BoolLit(!val, *s),
                    _ => Expr::Unary {
                        op: *op,
                        expr: Box::new(opt_expr),
                        span: *span,
                    },
                }
            }
            Expr::Call {
                callee,
                args,
                span,
            } => Expr::Call {
                callee: callee.clone(),
                args: args.iter().map(Self::optimize_expr).collect(),
                span: *span,
            },
            Expr::MemberAccess {
                object,
                member,
                span,
            } => Expr::MemberAccess {
                object: Box::new(Self::optimize_expr(object)),
                member: member.clone(),
                span: *span,
            },
            other => other.clone(),
        }
    }
}
