use volt_core::analysis::ReactivityAnalyzer;
use volt_core::codegen::WasmGcCompiler;
use volt_core::syntax;
use wasmparser::{CompositeInnerType, Operator, Parser, Payload};

#[test]
fn test_counter_reactivity_and_struct_layout() {
    let source = r#"
export component Counter {
  signal count: i32 = 0;
  signal step: i32 = 1;

  computed doubled: i32 = count * 2;

  fn increment() {
    count += step;
  }

  fn reset() {
    count = 0;
  }

  render {
    <div class="counter-card">
      <h2>"Volt Reactive Counter"</h2>
      <p>"Current Value: " {count} " (Doubled: " {doubled} ")" </p>
      <div class="actions">
        <button @click=increment>"Increment"</button>
        <button @click=reset>"Reset"</button>
      </div>
    </div>
  }
}
"#;

    let program = syntax::parse(source).expect("Syntax parse failed");
    let comp = match &program.items[0] {
        syntax::TopLevelItem::Component(c) => c,
        _ => panic!("Expected component"),
    };

    let plan = ReactivityAnalyzer::analyze_component(comp).expect("Reactivity analysis failed");

    // Verify analysis masks
    assert_eq!(plan.signals.len(), 2);
    assert_eq!(plan.signals[0].name, "count");
    assert_eq!(plan.signals[0].mask, 0b01);
    assert_eq!(plan.signals[1].name, "step");
    assert_eq!(plan.signals[1].mask, 0b10);

    assert_eq!(plan.computeds.len(), 1);
    assert_eq!(plan.computeds[0].name, "doubled");
    assert_eq!(plan.computeds[0].mask, 0b01); // depends on count (bit 0)

    assert_eq!(plan.dynamic_text_slots, 2);
    let dyn_nodes: Vec<_> = plan
        .elements
        .iter()
        .filter_map(|e| match &e.kind {
            volt_core::analysis::PlannedNodeKind::DynamicText(slot) => Some((e, slot)),
            _ => None,
        })
        .collect();
    assert_eq!(dyn_nodes.len(), 2);
    assert_eq!(dyn_nodes[0].1.state_slot, 0);
    assert_eq!(dyn_nodes[0].1.trigger_mask, 0b01);
    assert_eq!(dyn_nodes[1].1.state_slot, 1);
    assert_eq!(dyn_nodes[1].1.trigger_mask, 0b01);

    // Compile to Wasm bytes
    let mut compiler = WasmGcCompiler::new(plan.clone(), Vec::new());
    let wasm_bytes = compiler.compile().expect("Wasm compilation failed");

    // Parse with wasmparser and verify Wasm structures
    let mut struct_type_found = false;
    let mut function_bodies = Vec::new();

    for payload in Parser::new(0).parse_all(&wasm_bytes) {
        match payload.expect("Invalid Wasm payload") {
            Payload::TypeSection(reader) => {
                for ty in reader.into_iter() {
                    let rec_group = ty.expect("Failed to read RecGroup");
                    for sub_type in rec_group.types() {
                        if let CompositeInnerType::Struct(struct_type) = &sub_type.composite_type.inner {
                            struct_type_found = true;
                            // 1 (dirty_mask) + 2 (signals) + 1 (computed) + 2 (dynamic_slots) = 6 fields
                            assert_eq!(struct_type.fields.len(), 6);
                            for field in struct_type.fields.iter() {
                                assert!(field.mutable);
                            }
                            // Fields 0..4: i32 (dirty_mask, count, step, doubled)
                            for i in 0..4 {
                                assert_eq!(
                                    struct_type.fields[i].element_type,
                                    wasmparser::StorageType::Val(wasmparser::ValType::I32),
                                    "Field {} must be i32", i
                                );
                            }
                            // Fields 4..6: externref (dynamic text slots)
                            for i in 4..6 {
                                assert_eq!(
                                    struct_type.fields[i].element_type,
                                    wasmparser::StorageType::Val(wasmparser::ValType::Ref(wasmparser::RefType::EXTERNREF)),
                                    "Dynamic slot field {} must be externref", i
                                );
                            }
                        }
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                let mut ops = Vec::new();
                let mut reader = body.get_operators_reader().expect("Operators reader");
                while !reader.eof() {
                    ops.push(reader.read().expect("Operator read"));
                }
                function_bodies.push(ops);
            }
            _ => {}
        }
    }

    assert!(struct_type_found, "ComponentState struct type must be present in TypeSection");

    // Function indices in CodeSection:
    // func 0: mount
    // func 1: render_dynamic_text (direct setTextNumber calls)
    // func 2: increment (method 0)
    // func 3: reset (method 1)
    // func 4: get_count (getter sig 0)
    // func 5: get_step (getter sig 1)
    // func 6: get_doubled (getter comp 0)
    assert_eq!(function_bodies.len(), 7);

    // 1. Verify Mount function (func 0)
    let mount_ops = &function_bodies[0];
    // Must initialize dirty_mask to -1 (0xFFFFFFFF as i32)
    let has_dirty_mask_init = mount_ops.iter().any(|op| matches!(op, Operator::I32Const { value: -1 }));
    assert!(has_dirty_mask_init, "Mount must initialize field 0 dirty_mask to -1");

    // Must initialize dynamic slots with RefNull
    let has_ref_null = mount_ops.iter().any(|op| matches!(op, Operator::RefNull { .. }));
    assert!(has_ref_null, "Mount must initialize dynamic slot fields to ref.null extern");

    // Must clear dirty_mask to 0 before mount ends
    let has_dirty_mask_clear = mount_ops.windows(3).any(|w| {
        matches!(
            (&w[0], &w[1], &w[2]),
            (
                Operator::LocalGet { local_index: 1 },
                Operator::I32Const { value: 0 },
                Operator::StructSet { field_index: 0, .. }
            )
        )
    });
    assert!(has_dirty_mask_clear, "Mount must clear dirty_mask to 0 at completion");

    // Must attach events via delegated data-vt-<event> attributes (Import 3) instead of dom.addEventListener (Import 5)
    let calls_add_event_listener = mount_ops.iter().any(|op| matches!(op, Operator::Call { function_index: 5 }));
    assert!(!calls_add_event_listener, "Mount must use delegated data-vt-<event> attributes instead of dom.addEventListener");

    // 2. Verify render_dynamic_text function (func 1)
    let render_dyn_ops = &function_bodies[1];
    // Dynamic text slot must be guarded by dirty_mask & trigger_mask (1)
    let has_dyn_slot_guard = render_dyn_ops.windows(5).any(|w| {
        matches!(
            (&w[0], &w[1], &w[2], &w[3], &w[4]),
            (
                Operator::LocalGet { local_index: 0 },
                Operator::StructGet { field_index: 0, .. },
                Operator::I32Const { value: 1 },
                Operator::I32And,
                Operator::If { .. }
            )
        )
    });
    assert!(has_dyn_slot_guard, "render_dynamic_text must guard slots with (dirty_mask & trigger_mask) != 0");

    // Must invoke dom.setTextNumber (Import 6)
    let calls_set_text_number = render_dyn_ops.iter().any(|op| matches!(op, Operator::Call { function_index: 6 }));
    assert!(calls_set_text_number, "render_dynamic_text must call dom.setTextNumber (import index 6)");

    // 3. Verify increment method (func 2)
    let increment_ops = &function_bodies[2];
    // Signal mutation must mark bit in dirty_mask: dirty_mask |= 1
    let marks_dirty_bit = increment_ops.windows(6).any(|w| {
        matches!(
            (&w[0], &w[1], &w[2], &w[3], &w[4], &w[5]),
            (
                Operator::LocalGet { local_index: 0 },
                Operator::LocalGet { local_index: 0 },
                Operator::StructGet { field_index: 0, .. },
                Operator::I32Const { value: 1 },
                Operator::I32Or,
                Operator::StructSet { field_index: 0, .. }
            )
        )
    });
    assert!(marks_dirty_bit, "increment must mark dirty_mask with (1 << sig_id)");

    // Computed 'doubled' recomputation must be guarded by (dirty_mask & comp.mask)
    let guards_computed = increment_ops.windows(5).any(|w| {
        matches!(
            (&w[0], &w[1], &w[2], &w[3], &w[4]),
            (
                Operator::LocalGet { local_index: 0 },
                Operator::StructGet { field_index: 0, .. },
                Operator::I32Const { value: 1 },
                Operator::I32And,
                Operator::If { .. }
            )
        )
    });
    assert!(guards_computed, "increment must guard computed recalculation with (dirty_mask & comp.mask) != 0");

    // render_dynamic_text call must be guarded by dirty_mask != 0
    let guards_render_call = increment_ops.windows(4).any(|w| {
        matches!(
            (&w[0], &w[1], &w[2], &w[3]),
            (
                Operator::LocalGet { local_index: 0 },
                Operator::StructGet { field_index: 0, .. },
                Operator::If { .. },
                Operator::Call { .. }
            )
        )
    });
    assert!(guards_render_call, "increment must guard render_dynamic_text call with dirty_mask != 0");

    // Method must clear dirty_mask to 0 at end
    let clears_dirty_mask = increment_ops.windows(3).any(|w| {
        matches!(
            (&w[0], &w[1], &w[2]),
            (
                Operator::LocalGet { local_index: 0 },
                Operator::I32Const { value: 0 },
                Operator::StructSet { field_index: 0, .. }
            )
        )
    });
    assert!(clears_dirty_mask, "increment must clear dirty_mask to 0 at exit");

    // 4. Verify Getters
    // get_count (func 4) must read field 1
    let get_count_ops = &function_bodies[4];
    assert!(get_count_ops.iter().any(|op| matches!(op, Operator::StructGet { field_index: 1, .. })));

    // get_step (func 5) must read field 2
    let get_step_ops = &function_bodies[5];
    assert!(get_step_ops.iter().any(|op| matches!(op, Operator::StructGet { field_index: 2, .. })));

    // get_doubled (func 6) must read field 3
    let get_doubled_ops = &function_bodies[6];
    assert!(get_doubled_ops.iter().any(|op| matches!(op, Operator::StructGet { field_index: 3, .. })));
}

#[test]
fn test_multi_signal_independent_guards() {
    let source = r#"
export component MultiSignalApp {
  signal a: i32 = 10;
  signal b: i32 = 20;

  computed a_comp: i32 = a + 5;
  computed b_comp: i32 = b + 10;

  fn mutate_b_only() {
    b += 1;
  }

  render {
    <div>
      <p>"Slot A: " {a_comp}</p>
      <p>"Slot B: " {b_comp}</p>
    </div>
  }
}
"#;

    let program = syntax::parse(source).expect("Syntax parse failed");
    let comp = match &program.items[0] {
        syntax::TopLevelItem::Component(c) => c,
        _ => panic!("Expected component"),
    };

    let plan = ReactivityAnalyzer::analyze_component(comp).expect("Reactivity analysis failed");

    assert_eq!(plan.signals[0].name, "a");
    assert_eq!(plan.signals[0].mask, 0b01);
    assert_eq!(plan.signals[1].name, "b");
    assert_eq!(plan.signals[1].mask, 0b10);

    assert_eq!(plan.computeds[0].name, "a_comp");
    assert_eq!(plan.computeds[0].mask, 0b01);
    assert_eq!(plan.computeds[1].name, "b_comp");
    assert_eq!(plan.computeds[1].mask, 0b10);

    let mut compiler = WasmGcCompiler::new(plan.clone(), Vec::new());
    let wasm_bytes = compiler.compile().expect("Wasm compilation failed");

    // Parse the mutate_b_only method
    let mut function_bodies = Vec::new();
    for payload in Parser::new(0).parse_all(&wasm_bytes) {
        if let Payload::CodeSectionEntry(body) = payload.expect("Payload") {
            let mut ops = Vec::new();
            let mut reader = body.get_operators_reader().expect("Reader");
            while !reader.eof() {
                ops.push(reader.read().expect("Op"));
            }
            function_bodies.push(ops);
        }
    }

    // mutate_b_only is method 0 (func index 2)
    let mutate_ops = &function_bodies[2];

    // Check that dirty bit marked is 2 (1 << 1 for signal b)
    let marks_b_dirty = mutate_ops.windows(4).any(|w| {
        matches!(
            (&w[0], &w[1], &w[2], &w[3]),
            (
                Operator::StructGet { field_index: 0, .. },
                Operator::I32Const { value: 2 },
                Operator::I32Or,
                Operator::StructSet { field_index: 0, .. }
            )
        )
    });
    assert!(marks_b_dirty, "mutate_b_only must mark bit 2 (1 << 1) for signal b");

    // Check that a_comp (mask 1) is guarded by I32Const { value: 1 } and b_comp (mask 2) by I32Const { value: 2 }
    let guards_a = mutate_ops.windows(3).any(|w| {
        matches!(
            (&w[0], &w[1], &w[2]),
            (Operator::StructGet { field_index: 0, .. }, Operator::I32Const { value: 1 }, Operator::I32And)
        )
    });
    let guards_b = mutate_ops.windows(3).any(|w| {
        matches!(
            (&w[0], &w[1], &w[2]),
            (Operator::StructGet { field_index: 0, .. }, Operator::I32Const { value: 2 }, Operator::I32And)
        )
    });
    assert!(guards_a, "Method must test a_comp mask 1");
    assert!(guards_b, "Method must test b_comp mask 2");
}

#[test]
fn test_constant_folding_and_algebraic_simplification() {
    let source = r#"
export component ConstFoldApp {
  signal base: i32 = 100 - 50;
  computed folded_math: i32 = 10 * 2 + 5;
  computed identity_math: i32 = base + 0;

  fn do_nothing() {
    base = base * 1;
  }

  render {
    <div>
      <p>"Folded: " {folded_math}</p>
    </div>
  }
}
"#;

    let program = syntax::parse(source).expect("Syntax parse failed");
    let comp = match &program.items[0] {
        syntax::TopLevelItem::Component(c) => c,
        _ => panic!("Expected component"),
    };

    let plan = ReactivityAnalyzer::analyze_component(comp).expect("Reactivity analysis failed");

    // Verify signal init_expr folded to IntLit(50)
    assert_eq!(plan.signals[0].name, "base");
    assert!(matches!(plan.signals[0].init_expr, syntax::ast::Expr::IntLit(50, _)));

    // Verify folded_math folded to IntLit(25)
    assert_eq!(plan.computeds[0].name, "folded_math");
    assert!(matches!(plan.computeds[0].expr, syntax::ast::Expr::IntLit(25, _)));
    assert_eq!(plan.computeds[0].mask, 0); // constant computed has no dependencies

    // Verify identity_math folded from base + 0 to Ident("base")
    assert_eq!(plan.computeds[1].name, "identity_math");
    assert!(matches!(&plan.computeds[1].expr, syntax::ast::Expr::Ident(name, _) if name == "base"));
    assert_eq!(plan.computeds[1].mask, 0b1);

    // Compile to Wasm bytes and check bytecode
    let mut compiler = WasmGcCompiler::new(plan, Vec::new());
    let wasm_bytes = compiler.compile().expect("Wasm compilation failed");

    let mut function_bodies = Vec::new();
    for payload in Parser::new(0).parse_all(&wasm_bytes) {
        if let Payload::CodeSectionEntry(body) = payload.expect("Payload") {
            let mut ops = Vec::new();
            let mut reader = body.get_operators_reader().expect("Reader");
            while !reader.eof() {
                ops.push(reader.read().expect("Op"));
            }
            function_bodies.push(ops);
        }
    }

    // Mount is func 0
    let mount_ops = &function_bodies[0];
    // Must initialize signal with 50 directly (no I32Sub)
    assert!(mount_ops.iter().any(|op| matches!(op, Operator::I32Const { value: 50 })));
    assert!(!mount_ops.iter().any(|op| matches!(op, Operator::I32Sub)));

    // Initial computation for folded_math must emit I32Const 25 directly (no I32Mul, no I32Add)
    assert!(mount_ops.iter().any(|op| matches!(op, Operator::I32Const { value: 25 })));
    assert!(!mount_ops.iter().any(|op| matches!(op, Operator::I32Mul)));
}

