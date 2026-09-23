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

#[derive(Debug, Clone)]
pub struct DynamicTextSlot {
    pub state_slot: usize,
    pub trigger_mask: u32,
    pub expr: Expr,
}

#[derive(Debug, Clone)]
pub enum PlannedNodeKind {
    Element {
        tag: String,
        static_attrs: Vec<(String, String)>,
        events: Vec<(String, String)>,
        static_text: Option<String>,
    },
    StaticText(String),
    DynamicText(DynamicTextSlot),
}

#[derive(Debug, Clone)]
pub struct PlannedElement {
    pub id: usize,
    pub parent_id: usize, // 0 is root passed into mount
    pub kind: PlannedNodeKind,
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
        let opt_comp = crate::analysis::optimizer::AstOptimizer::optimize_component(comp.clone());
        let comp = &opt_comp;

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

                let has_dynamic = elem.children.iter().any(|c| matches!(c, TemplateNode::DynamicExpr(..)));
                let has_child_elements = elem.children.iter().any(|c| matches!(c, TemplateNode::Element(..)));

                if !has_dynamic && !has_child_elements {
                    // Leaf element with only static text (e.g. <h2>"Title"</h2> or <button>"Increment"</button>)
                    let mut combined = String::new();
                    for child in &elem.children {
                        if let TemplateNode::Text(txt, _) = child {
                            combined.push_str(txt);
                        }
                    }
                    let static_text = if combined.is_empty() { None } else { Some(combined) };
                    elements.push(PlannedElement {
                        id: elem_id,
                        parent_id,
                        kind: PlannedNodeKind::Element {
                            tag: elem.tag.clone(),
                            static_attrs,
                            events,
                            static_text,
                        },
                    });
                } else {
                    // Element with child elements, dynamic expressions, or mixed content
                    elements.push(PlannedElement {
                        id: elem_id,
                        parent_id,
                        kind: PlannedNodeKind::Element {
                            tag: elem.tag.clone(),
                            static_attrs,
                            events,
                            static_text: None,
                        },
                    });

                    // Plan children in exact source order
                    for child in &elem.children {
                        match child {
                            TemplateNode::Element(child_elem) => {
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
                            TemplateNode::Text(txt, _) => {
                                let text_id = *next_elem_id;
                                *next_elem_id += 1;
                                elements.push(PlannedElement {
                                    id: text_id,
                                    parent_id: elem_id,
                                    kind: PlannedNodeKind::StaticText(txt.clone()),
                                });
                            }
                            TemplateNode::DynamicExpr(expr, _) => {
                                let dyn_id = *next_elem_id;
                                *next_elem_id += 1;
                                let slot = *next_slot_id;
                                *next_slot_id += 1;
                                let mask = Self::calculate_mask_for_expr(expr, signals, computeds);
                                elements.push(PlannedElement {
                                    id: dyn_id,
                                    parent_id: elem_id,
                                    kind: PlannedNodeKind::DynamicText(DynamicTextSlot {
                                        state_slot: slot,
                                        trigger_mask: mask,
                                        expr: expr.clone(),
                                    }),
                                });
                            }
                        }
                    }
                }

                Ok(())
            }
            TemplateNode::Text(txt, _) => {
                let text_id = *next_elem_id;
                *next_elem_id += 1;
                elements.push(PlannedElement {
                    id: text_id,
                    parent_id,
                    kind: PlannedNodeKind::StaticText(txt.clone()),
                });
                Ok(())
            }
            TemplateNode::DynamicExpr(expr, _) => {
                let dyn_id = *next_elem_id;
                *next_elem_id += 1;
                let slot = *next_slot_id;
                *next_slot_id += 1;
                let mask = Self::calculate_mask_for_expr(expr, signals, computeds);
                elements.push(PlannedElement {
                    id: dyn_id,
                    parent_id,
                    kind: PlannedNodeKind::DynamicText(DynamicTextSlot {
                        state_slot: slot,
                        trigger_mask: mask,
                        expr: expr.clone(),
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
