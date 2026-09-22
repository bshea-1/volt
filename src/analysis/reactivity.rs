use crate::syntax::ast::*;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct SignalInfo {
    pub id: usize,
    pub mask: u32,
    pub name: String,
    pub type_ann: Type,
    pub init_expr: Expr,
}

#[derive(Debug, Clone)]
pub struct ComputedInfo {
    pub id: usize,
    pub name: String,
    pub type_ann: Type,
    pub expr: Expr,
    pub mask: u32, // Combined mask of root signals it depends on
}

#[derive(Debug, Clone, PartialEq)]
pub enum TextPart {
    Lit(String),
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub struct DynamicTextPlan {
    pub state_slot: usize,
    pub target_elem_id: usize,
    pub trigger_mask: u32,
    pub parts: Vec<TextPart>,
}

#[derive(Debug, Clone)]
pub struct PlannedElement {
    pub id: usize,
    pub tag: String,
    pub parent_id: usize, // 0 is root passed into mount
    pub static_attrs: Vec<(String, String)>,
    pub events: Vec<(String, String)>, // (event_name, handler_name)
    pub static_text: Option<String>,
    pub dynamic_text: Option<DynamicTextPlan>,
}

#[derive(Debug, Clone)]
pub struct ComponentPlan {
    pub name: String,
    pub is_exported: bool,
    pub signals: Vec<SignalInfo>,
    pub computeds: Vec<ComputedInfo>,
    pub methods: Vec<FnDecl>,
    pub elements: Vec<PlannedElement>,
    pub dynamic_text_slots: usize,
    pub signal_name_to_id: HashMap<String, usize>,
    pub computed_name_to_id: HashMap<String, usize>,
}

pub struct ReactivityAnalyzer;

impl ReactivityAnalyzer {
    pub fn analyze_component(comp: &ComponentDecl) -> Result<ComponentPlan, String> {
        let mut signals = Vec::new();
        let mut signal_name_to_id = HashMap::new();

        for (id, sig) in comp.signals.iter().enumerate() {
            let mask = 1u32.checked_shl(id as u32).ok_or_else(|| {
                format!("Too many signals in component {} (max 32)", comp.name)
            })?;
            signal_name_to_id.insert(sig.name.clone(), id);
            signals.push(SignalInfo {
                id,
                mask,
                name: sig.name.clone(),
                type_ann: sig.type_ann.clone(),
                init_expr: sig.init_expr.clone(),
            });
        }

        // Analyze computeds and determine signal dependency masks
        let mut computeds = Vec::new();
        let mut computed_name_to_id = HashMap::new();

        for (id, comp_decl) in comp.computeds.iter().enumerate() {
            computed_name_to_id.insert(comp_decl.name.clone(), id);
        }

        for (id, comp_decl) in comp.computeds.iter().enumerate() {
            let mask = Self::calculate_expr_mask(
                &comp_decl.expr,
                &signal_name_to_id,
                &computed_name_to_id,
                &comp.computeds,
                &mut HashSet::new(),
            )?;
            computeds.push(ComputedInfo {
                id,
                name: comp_decl.name.clone(),
                type_ann: comp_decl.type_ann.clone(),
                expr: comp_decl.expr.clone(),
                mask,
            });
        }

        // Plan elements and dynamic slot allocations
        let mut elements = Vec::new();
        let mut dynamic_text_slots = 0;

        if let Some(ref render) = comp.render {
            let mut next_elem_id = 1;
            Self::plan_template_node(
                &render.root,
                0, // Root parent
                &mut next_elem_id,
                &mut dynamic_text_slots,
                &signal_name_to_id,
                &computeds,
                &mut elements,
            )?;
        }

        Ok(ComponentPlan {
            name: comp.name.clone(),
            is_exported: comp.is_exported,
            signals,
            computeds,
            methods: comp.methods.clone(),
            elements,
            dynamic_text_slots,
            signal_name_to_id,
            computed_name_to_id,
        })
    }

    fn calculate_expr_mask(
        expr: &Expr,
        signals: &HashMap<String, usize>,
        computeds: &HashMap<String, usize>,
        all_computeds: &[ComputedDecl],
        visited: &mut HashSet<String>,
    ) -> Result<u32, String> {
        match expr {
            Expr::Ident(name, _) => {
                if let Some(&sig_id) = signals.get(name) {
                    Ok(1u32 << sig_id)
                } else if let Some(&comp_id) = computeds.get(name) {
                    if visited.contains(name) {
                        return Err(format!("Cyclic dependency detected in computed '{}'", name));
                    }
                    visited.insert(name.clone());
                    let comp_decl = &all_computeds[comp_id];
                    let mask = Self::calculate_expr_mask(
                        &comp_decl.expr,
                        signals,
                        computeds,
                        all_computeds,
                        visited,
                    )?;
                    visited.remove(name);
                    Ok(mask)
                } else {
                    Ok(0) // Constant or parameter
                }
            }
            Expr::Binary { left, right, .. } => {
                let m1 = Self::calculate_expr_mask(left, signals, computeds, all_computeds, visited)?;
                let m2 = Self::calculate_expr_mask(right, signals, computeds, all_computeds, visited)?;
                Ok(m1 | m2)
            }
            Expr::Unary { expr, .. } => {
                Self::calculate_expr_mask(expr, signals, computeds, all_computeds, visited)
            }
            Expr::Call { args, .. } => {
                let mut mask = 0;
                for arg in args {
                    mask |= Self::calculate_expr_mask(arg, signals, computeds, all_computeds, visited)?;
                }
                Ok(mask)
            }
            Expr::MemberAccess { object, .. } => {
                Self::calculate_expr_mask(object, signals, computeds, all_computeds, visited)
            }
            Expr::IntLit(..) | Expr::FloatLit(..) | Expr::StringLit(..) | Expr::BoolLit(..) => Ok(0),
        }
    }

    fn plan_template_node(
        node: &TemplateNode,
        parent_id: usize,
        next_elem_id: &mut usize,
        next_slot_id: &mut usize,
        signals: &HashMap<String, usize>,
        computeds: &[ComputedInfo],
        elements: &mut Vec<PlannedElement>,
    ) -> Result<(), String> {
        match node {
            TemplateNode::Element(elem) => {
                let elem_id = *next_elem_id;
                *next_elem_id += 1;

                let mut static_attrs = Vec::new();
                for attr in &elem.attributes {
                    match &attr.value {
                        AttributeValue::Static(val) => {
                            static_attrs.push((attr.name.clone(), val.clone()));
                        }
                        AttributeValue::Dynamic(_) => {
                            // Can support dynamic attributes in the future
                        }
                    }
                }

                let mut events = Vec::new();
                for ev in &elem.events {
                    events.push((ev.event_name.clone(), ev.handler.clone()));
                }

                // Check children: separate static text vs dynamic expressions vs child elements
                let mut child_elements = Vec::new();
                let mut text_parts = Vec::new();
                let mut is_dynamic = false;
                let mut trigger_mask = 0u32;

                for child in &elem.children {
                    match child {
                        TemplateNode::Element(child_elem) => {
                            child_elements.push(child_elem);
                        }
                        TemplateNode::Text(txt, _) => {
                            text_parts.push(TextPart::Lit(txt.clone()));
                        }
                        TemplateNode::DynamicExpr(expr, _) => {
                            is_dynamic = true;
                            let expr_mask = Self::calculate_mask_for_expr(expr, signals, computeds);
                            trigger_mask |= expr_mask;
                            text_parts.push(TextPart::Expr(expr.clone()));
                        }
                    }
                }

                let (static_text, dynamic_text) = if is_dynamic {
                    let slot = *next_slot_id;
                    *next_slot_id += 1;
                    (
                        None,
                        Some(DynamicTextPlan {
                            state_slot: slot,
                            target_elem_id: elem_id,
                            trigger_mask,
                            parts: text_parts,
                        }),
                    )
                } else if !text_parts.is_empty() {
                    let mut combined = String::new();
                    for part in text_parts {
                        if let TextPart::Lit(s) = part {
                            combined.push_str(&s);
                        }
                    }
                    (Some(combined), None)
                } else {
                    (None, None)
                };

                elements.push(PlannedElement {
                    id: elem_id,
                    tag: elem.tag.clone(),
                    parent_id,
                    static_attrs,
                    events,
                    static_text,
                    dynamic_text,
                });

                // Recursively plan child elements
                for child_elem in child_elements {
                    Self::plan_template_node(
                        &TemplateNode::Element(child_elem.clone()),
                        elem_id,
                        next_elem_id,
                        next_slot_id,
                        signals,
                        computeds,
                        elements,
                    )?;
                }

                Ok(())
            }
            TemplateNode::Text(txt, _) => {
                // Top level text or non-element text
                let elem_id = *next_elem_id;
                *next_elem_id += 1;
                elements.push(PlannedElement {
                    id: elem_id,
                    tag: "span".into(),
                    parent_id,
                    static_attrs: Vec::new(),
                    events: Vec::new(),
                    static_text: Some(txt.clone()),
                    dynamic_text: None,
                });
                Ok(())
            }
            TemplateNode::DynamicExpr(expr, _) => {
                let elem_id = *next_elem_id;
                *next_elem_id += 1;
                let slot = *next_slot_id;
                *next_slot_id += 1;
                let mask = Self::calculate_mask_for_expr(expr, signals, computeds);
                elements.push(PlannedElement {
                    id: elem_id,
                    tag: "span".into(),
                    parent_id,
                    static_attrs: Vec::new(),
                    events: Vec::new(),
                    static_text: None,
                    dynamic_text: Some(DynamicTextPlan {
                        state_slot: slot,
                        target_elem_id: elem_id,
                        trigger_mask: mask,
                        parts: vec![TextPart::Expr(expr.clone())],
                    }),
                });
                Ok(())
            }
        }
    }

    fn calculate_mask_for_expr(
        expr: &Expr,
        signals: &HashMap<String, usize>,
        computeds: &[ComputedInfo],
    ) -> u32 {
        match expr {
            Expr::Ident(name, _) => {
                if let Some(&sig_id) = signals.get(name) {
                    1u32 << sig_id
                } else if let Some(comp) = computeds.iter().find(|c| &c.name == name) {
                    comp.mask
                } else {
                    0
                }
            }
            Expr::Binary { left, right, .. } => {
                Self::calculate_mask_for_expr(left, signals, computeds)
                    | Self::calculate_mask_for_expr(right, signals, computeds)
            }
            Expr::Unary { expr, .. } => Self::calculate_mask_for_expr(expr, signals, computeds),
            Expr::Call { args, .. } => {
                let mut mask = 0;
                for arg in args {
                    mask |= Self::calculate_mask_for_expr(arg, signals, computeds);
                }
                mask
            }
            Expr::MemberAccess { object, .. } => {
                Self::calculate_mask_for_expr(object, signals, computeds)
            }
            Expr::IntLit(..) | Expr::FloatLit(..) | Expr::StringLit(..) | Expr::BoolLit(..) => 0,
        }
    }
}
