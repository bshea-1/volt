use crate::analysis::reactivity::{ComponentPlan, PlannedNodeKind};
use crate::codegen::memory_layout::{StringTable, DATA_START};
use crate::syntax::ast::*;
use wasm_encoder::*;

pub struct WasmGcCompiler {
    plan: ComponentPlan,
    extern_blocks: Vec<ExternBlock>,
    string_table: StringTable,
}

impl WasmGcCompiler {
    pub const DIRTY_MASK_FIELD_IDX: u32 = 0;

    pub const DOM_CREATE_ELEMENT_IMPORT_IDX: u32 = 0;
    pub const DOM_CREATE_TEXT_NODE_IMPORT_IDX: u32 = 1;
    pub const DOM_SET_TEXT_CONTENT_IMPORT_IDX: u32 = 2;
    pub const DOM_SET_ATTRIBUTE_IMPORT_IDX: u32 = 3;
    pub const DOM_APPEND_CHILD_IMPORT_IDX: u32 = 4;
    pub const DOM_ADD_EVENT_LISTENER_IMPORT_IDX: u32 = 5;
    pub const DOM_SET_TEXT_NUMBER_IMPORT_IDX: u32 = 6;

    pub fn new(plan: ComponentPlan, extern_blocks: Vec<ExternBlock>) -> Self {
        Self {
            plan,
            extern_blocks,
            string_table: StringTable::new(),
        }
    }

    #[inline]
    pub fn signal_field_idx(&self, sig_id: usize) -> u32 {
        1 + sig_id as u32
    }

    #[inline]
    pub fn computed_field_idx(&self, comp_id: usize) -> u32 {
        1 + self.plan.signals.len() as u32 + comp_id as u32
    }

    #[inline]
    pub fn dynamic_slot_field_idx(&self, state_slot: usize) -> u32 {
        1 + self.plan.signals.len() as u32 + self.plan.computeds.len() as u32 + state_slot as u32
    }

    #[inline]
    pub fn total_struct_fields(&self) -> usize {
        1 + self.plan.signals.len() + self.plan.computeds.len() + self.plan.dynamic_text_slots
    }


    pub fn compile(&mut self) -> Result<Vec<u8>, String> {
        let mut module = Module::new();

        // 1. Populate string table with all static strings
        self.collect_strings();

        // 2. Type Section
        let mut types = TypeSection::new();
        // Type indices:
        // 0: sig_dom_create (i32, i32) -> (externref)
        types.ty().function(
            vec![ValType::I32, ValType::I32],
            vec![ValType::Ref(RefType::EXTERNREF)],
        );
        // 1: sig_dom_set_text (externref, i32, i32) -> ()
        types.ty().function(
            vec![ValType::Ref(RefType::EXTERNREF), ValType::I32, ValType::I32],
            vec![],
        );
        // 2: sig_dom_set_attr (externref, i32, i32, i32, i32) -> ()
        types.ty().function(
            vec![
                ValType::Ref(RefType::EXTERNREF),
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I32,
            ],
            vec![],
        );
        // 3: sig_dom_append (externref, externref) -> ()
        types.ty().function(
            vec![
                ValType::Ref(RefType::EXTERNREF),
                ValType::Ref(RefType::EXTERNREF),
            ],
            vec![],
        );
        // 4: sig_dom_add_event (externref, i32, i32, i32) -> ()
        types.ty().function(
            vec![
                ValType::Ref(RefType::EXTERNREF),
                ValType::I32,
                ValType::I32,
                ValType::I32,
            ],
            vec![],
        );
        // 5: sig_dom_set_text_num (externref, i32) -> ()
        types.ty().function(
            vec![ValType::Ref(RefType::EXTERNREF), ValType::I32],
            vec![],
        );
        // 6: sig_mount (externref) -> ()
        types.ty().function(vec![ValType::Ref(RefType::EXTERNREF)], vec![]);
        // 7: sig_void () -> ()
        types.ty().function(vec![], vec![]);
        // 8: sig_getter () -> (i32)
        types.ty().function(vec![], vec![ValType::I32]);
        // 9: sig_extern_i32_i32_void (i32, i32) -> ()
        types.ty().function(vec![ValType::I32, ValType::I32], vec![]);
        // 10: sig_extern_i32_i32_i32 (i32, i32) -> (i32)
        types.ty().function(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
        // 11: sig_extern_i32_void (i32) -> ()
        types.ty().function(vec![ValType::I32], vec![]);

        // GC Struct Type: ComponentState
        // Field 0: dirty_mask (i32)
        // Fields 1..1+num_signals: signals (i32)
        // Fields 1+num_signals..1+num_signals+num_computeds: computeds (i32)
        // Dynamic slots: dynamic_text_slots (externref)
        let total_fields = self.total_struct_fields();
        let num_i32_fields = 1 + self.plan.signals.len() + self.plan.computeds.len();
        let mut struct_fields = Vec::with_capacity(total_fields);
        for i in 0..total_fields {
            let val_type = if i < num_i32_fields {
                ValType::I32
            } else {
                ValType::Ref(RefType::EXTERNREF)
            };
            struct_fields.push(FieldType {
                element_type: StorageType::Val(val_type),
                mutable: true,
            });
        }
        let struct_type = StructType {
            fields: struct_fields.into_boxed_slice(),
        };
        let struct_type_idx = 12;
        types.ty().subtype(&SubType {
            is_final: true,
            supertype_idx: None,
            composite_type: CompositeType {
                shared: false,
                inner: CompositeInnerType::Struct(struct_type),
                describes: None,
                descriptor: None,
            },
        });

        module.section(&types);

        // 3. Import Section
        let mut imports = ImportSection::new();
        // 0: dom.createElement: type 0 (i32, i32) -> externref
        imports.import("dom", "createElement", EntityType::Function(0));
        // 1: dom.createTextNode: type 0 (i32, i32) -> externref
        imports.import("dom", "createTextNode", EntityType::Function(0));
        // 2: dom.setTextContent: type 1 (externref, i32, i32) -> ()
        imports.import("dom", "setTextContent", EntityType::Function(1));
        // 3: dom.setAttribute: type 2 (externref, i32, i32, i32, i32) -> ()
        imports.import("dom", "setAttribute", EntityType::Function(2));
        // 4: dom.appendChild: type 3 (externref, externref) -> ()
        imports.import("dom", "appendChild", EntityType::Function(3));
        // 5: dom.addEventListener: type 4 (externref, i32, i32, i32) -> ()
        imports.import("dom", "addEventListener", EntityType::Function(4));
        // 6: dom.setTextNumber: type 5 (externref, i32) -> ()
        imports.import("dom", "setTextNumber", EntityType::Function(5));

        let mut num_imports = 7u32;
        let mut extern_func_indices = std::collections::HashMap::new();

        // Add any foreign functions from extern blocks
        for block in &self.extern_blocks {
            for func in &block.functions {
                let type_idx = if func.params.len() == 2 && func.return_type.is_some() {
                    10 // (i32, i32) -> i32
                } else if func.params.len() == 2 {
                    9 // (i32, i32) -> ()
                } else if func.params.len() == 1 && func.return_type.is_none() {
                    11 // (i32) -> ()
                } else if func.return_type.is_some() {
                    8 // () -> i32
                } else {
                    7 // () -> ()
                };
                imports.import(&block.abi, &func.name, EntityType::Function(type_idx));
                extern_func_indices.insert(func.name.clone(), num_imports);
                num_imports += 1;
            }
        }
        module.section(&imports);

        // 4. Function Section
        // Defined functions:
        // func (num_imports): mount (type 6: [externref] -> [])
        // func (num_imports + 1): render_dynamic_text (type 7: [] -> [])
        // func (num_imports + 2..): methods (type 7: [] -> [])
        // func (after methods): getters for signals/computeds (type 8: [] -> [i32])
        let mut functions = FunctionSection::new();
        functions.function(6); // mount
        functions.function(7); // render_dynamic_text

        let method_start_func_idx = num_imports + 2;
        for _ in &self.plan.methods {
            functions.function(7); // methods have sig_void
        }

        let getter_start_func_idx = method_start_func_idx + self.plan.methods.len() as u32;
        for _ in &self.plan.signals {
            functions.function(8); // getter () -> i32
        }
        for _ in &self.plan.computeds {
            functions.function(8); // getter () -> i32
        }

        module.section(&functions);

        // 5. Table Section (for event listener funcrefs)
        let mut tables = TableSection::new();
        let num_methods = self.plan.methods.len() as u64;
        tables.table(TableType {
            element_type: RefType::FUNCREF,
            minimum: num_methods.max(1),
            maximum: Some(num_methods.max(1)),
            shared: false,
            table64: false,
        });
        module.section(&tables);

        // 6. Memory Section
        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: 1,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&memories);

        // 7. Global Section (Component State GC Struct Reference)
        let mut globals = GlobalSection::new();
        globals.global(
            GlobalType {
                val_type: ValType::Ref(RefType {
                    nullable: true,
                    heap_type: HeapType::Concrete(struct_type_idx),
                }),
                mutable: true,
                shared: false,
            },
            &ConstExpr::ref_null(HeapType::Concrete(struct_type_idx)),
        );
        module.section(&globals);

        // 8. Export Section
        let mut exports = ExportSection::new();
        exports.export("memory", ExportKind::Memory, 0);
        exports.export("__table", ExportKind::Table, 0);
        exports.export("mount", ExportKind::Func, num_imports); // func 6 is mount

        // Export methods
        for (i, method) in self.plan.methods.iter().enumerate() {
            let func_idx = method_start_func_idx + i as u32;
            exports.export(&method.name, ExportKind::Func, func_idx);
        }

        // Export getters
        let mut cur_getter_idx = getter_start_func_idx;
        for sig in &self.plan.signals {
            exports.export(&format!("get_{}", sig.name), ExportKind::Func, cur_getter_idx);
            cur_getter_idx += 1;
        }
        for comp in &self.plan.computeds {
            exports.export(&format!("get_{}", comp.name), ExportKind::Func, cur_getter_idx);
            cur_getter_idx += 1;
        }

        module.section(&exports);

        // 9. Element Section (Populates Table with method funcrefs)
        if num_methods > 0 {
            let mut elements = ElementSection::new();
            let mut method_func_indices = Vec::new();
            for i in 0..num_methods {
                method_func_indices.push(method_start_func_idx + i as u32);
            }
            elements.active(
                Some(0),
                &ConstExpr::i32_const(0),
                Elements::Functions(std::borrow::Cow::Borrowed(&method_func_indices)),
            );
            module.section(&elements);
        }

        // 10. Code Section
        let mut codes = CodeSection::new();

        // mount(root_id: i32)
        codes.function(&self.emit_mount_function(
            struct_type_idx,
            num_imports,
            method_start_func_idx,
            &extern_func_indices,
        )?);

        // render_dynamic_text()
        codes.function(&self.emit_render_dynamic_text_function(
            struct_type_idx,
            num_imports,
            &extern_func_indices,
        )?);

        // Methods: increment, reset, onMount, etc.
        for (method_idx, method) in self.plan.methods.iter().enumerate() {
            codes.function(&self.emit_method_function(
                method_idx,
                method,
                struct_type_idx,
                num_imports,
                &extern_func_indices,
            )?);
        }

        // Getters: get_count, get_step, get_doubled
        for sig in &self.plan.signals {
            codes.function(&self.emit_getter_function(self.signal_field_idx(sig.id), struct_type_idx));
        }
        for comp in &self.plan.computeds {
            codes.function(&self.emit_getter_function(self.computed_field_idx(comp.id), struct_type_idx));
        }

        module.section(&codes);

        // 11. Data Section (String Table)
        let mut data = DataSection::new();
        data.active(
            0,
            &ConstExpr::i32_const(DATA_START as i32),
            self.string_table.data().iter().copied(),
        );
        module.section(&data);

        Ok(module.finish())
    }

    fn collect_strings(&mut self) {
        for elem in &self.plan.elements {
            match &elem.kind {
                PlannedNodeKind::Element {
                    tag,
                    static_attrs,
                    events,
                    static_text,
                } => {
                    self.string_table.insert(tag);
                    for (attr_name, attr_val) in static_attrs {
                        self.string_table.insert(attr_name);
                        self.string_table.insert(attr_val);
                    }
                    for (ev_name, _) in events {
                        let attr_name = format!("data-vt-{}", ev_name);
                        self.string_table.insert(&attr_name);
                    }
                    if let Some(ref text) = static_text {
                        self.string_table.insert(text);
                    }
                }
                PlannedNodeKind::StaticText(text) => {
                    self.string_table.insert(text);
                }
                PlannedNodeKind::DynamicText(_) => {
                    self.string_table.insert("");
                }
            }
        }
        for i in 0..self.plan.methods.len() {
            let idx_str = i.to_string();
            self.string_table.insert(&idx_str);
        }
        for method in &self.plan.methods {
            for stmt in &method.body {
                Self::collect_strings_from_stmt(stmt, &mut self.string_table);
            }
        }
    }

    fn collect_strings_from_stmt(stmt: &Stmt, string_table: &mut StringTable) {
        match stmt {
            Stmt::Assign { value, .. } | Stmt::AssignOp { value, .. } => {
                Self::collect_strings_from_expr(value, string_table);
            }
            Stmt::Expr(expr) => {
                Self::collect_strings_from_expr(expr, string_table);
            }
            _ => {}
        }
    }

    fn collect_strings_from_expr(expr: &Expr, string_table: &mut StringTable) {
        match expr {
            Expr::StringLit(s, _) => {
                string_table.insert(s);
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    Self::collect_strings_from_expr(arg, string_table);
                }
            }
            Expr::Binary { left, right, .. } => {
                Self::collect_strings_from_expr(left, string_table);
                Self::collect_strings_from_expr(right, string_table);
            }
            Expr::Unary { expr, .. } => {
                Self::collect_strings_from_expr(expr, string_table);
            }
            _ => {}
        }
    }

    fn emit_mount_function(
        &self,
        struct_type_idx: u32,
        num_imports: u32,
        method_start_func_idx: u32,
        extern_func_indices: &std::collections::HashMap<String, u32>,
    ) -> Result<Function, String> {
        // Locals:
        // param 0: root (externref)
        // local 1: state struct ref
        // local 2..: local vars for created DOM element nodes (externref)
        let num_elements = self.plan.elements.len();
        let locals = vec![
            (
                1,
                ValType::Ref(RefType {
                    nullable: true,
                    heap_type: HeapType::Concrete(struct_type_idx),
                }),
            ),
            (num_elements as u32, ValType::Ref(RefType::EXTERNREF)),
        ];

        let mut func = Function::new(locals);

        // 1. Allocate ComponentState GC struct
        // Field 0: Initial dirty_mask = -1 (0xFFFFFFFF) so initial render triggers all slots
        func.instruction(&Instruction::I32Const(-1));

        // Push initial values for all signals
        for sig in &self.plan.signals {
            self.emit_const_i32_expr(&sig.init_expr, &mut func);
        }
        // Push initial values for computeds (0 initially)
        for _ in &self.plan.computeds {
            func.instruction(&Instruction::I32Const(0));
        }
        // Push initial values for dynamic text node slots (ref.null extern initially)
        for _ in 0..self.plan.dynamic_text_slots {
            func.instruction(&Instruction::RefNull(RefType::EXTERNREF.heap_type));
        }

        func.instruction(&Instruction::StructNew(struct_type_idx));
        func.instruction(&Instruction::LocalSet(1)); // local 1 = state
        func.instruction(&Instruction::LocalGet(1));
        func.instruction(&Instruction::GlobalSet(0)); // global 0 = state

        // 2. Instantiate DOM elements
        for (i, elem) in self.plan.elements.iter().enumerate() {
            let elem_local = 2 + i as u32;

            match &elem.kind {
                PlannedNodeKind::Element {
                    tag,
                    static_attrs,
                    events,
                    static_text,
                } => {
                    let tag_entry = self.string_table.get(tag).unwrap();
                    // dom.createElement(tagPtr, tagLen) -> nodeId
                    func.instruction(&Instruction::I32Const(tag_entry.offset as i32));
                    func.instruction(&Instruction::I32Const(tag_entry.len as i32));
                    func.instruction(&Instruction::Call(Self::DOM_CREATE_ELEMENT_IMPORT_IDX));
                    func.instruction(&Instruction::LocalSet(elem_local));

                    // Set static attributes
                    for (attr_name, attr_val) in static_attrs {
                        let name_entry = self.string_table.get(attr_name).unwrap();
                        let val_entry = self.string_table.get(attr_val).unwrap();

                        func.instruction(&Instruction::LocalGet(elem_local));
                        func.instruction(&Instruction::I32Const(name_entry.offset as i32));
                        func.instruction(&Instruction::I32Const(name_entry.len as i32));
                        func.instruction(&Instruction::I32Const(val_entry.offset as i32));
                        func.instruction(&Instruction::I32Const(val_entry.len as i32));
                        func.instruction(&Instruction::Call(Self::DOM_SET_ATTRIBUTE_IMPORT_IDX));
                    }

                    // Set static text content if present (for leaf elements)
                    if let Some(ref text) = static_text {
                        let text_entry = self.string_table.get(text).unwrap();
                        func.instruction(&Instruction::LocalGet(elem_local));
                        func.instruction(&Instruction::I32Const(text_entry.offset as i32));
                        func.instruction(&Instruction::I32Const(text_entry.len as i32));
                        func.instruction(&Instruction::Call(Self::DOM_SET_TEXT_CONTENT_IMPORT_IDX));
                    }

                    // Attach event listeners via delegated data-vt-<event> attributes
                    for (ev_name, handler_name) in events {
                        let attr_name = format!("data-vt-{}", ev_name);
                        let table_idx = self
                            .plan
                            .methods
                            .iter()
                            .position(|m| &m.name == handler_name)
                            .ok_or_else(|| format!("Handler '{}' not found", handler_name))?;
                        let val_str = table_idx.to_string();

                        let name_entry = self.string_table.get(&attr_name).unwrap();
                        let val_entry = self.string_table.get(&val_str).unwrap();

                        func.instruction(&Instruction::LocalGet(elem_local));
                        func.instruction(&Instruction::I32Const(name_entry.offset as i32));
                        func.instruction(&Instruction::I32Const(name_entry.len as i32));
                        func.instruction(&Instruction::I32Const(val_entry.offset as i32));
                        func.instruction(&Instruction::I32Const(val_entry.len as i32));
                        func.instruction(&Instruction::Call(Self::DOM_SET_ATTRIBUTE_IMPORT_IDX));
                    }
                }
                PlannedNodeKind::StaticText(text) => {
                    let text_entry = self.string_table.get(text).unwrap();
                    // dom.createTextNode(textPtr, textLen) -> nodeId
                    func.instruction(&Instruction::I32Const(text_entry.offset as i32));
                    func.instruction(&Instruction::I32Const(text_entry.len as i32));
                    func.instruction(&Instruction::Call(Self::DOM_CREATE_TEXT_NODE_IMPORT_IDX));
                    func.instruction(&Instruction::LocalSet(elem_local));
                }
                PlannedNodeKind::DynamicText(slot_plan) => {
                    // dom.createTextNode("", 0) -> nodeId
                    let empty_entry = self.string_table.get("").unwrap();
                    func.instruction(&Instruction::I32Const(empty_entry.offset as i32));
                    func.instruction(&Instruction::I32Const(0));
                    func.instruction(&Instruction::Call(Self::DOM_CREATE_TEXT_NODE_IMPORT_IDX));
                    func.instruction(&Instruction::LocalSet(elem_local));

                    // Save node ID into component state struct slot
                    let slot_field_idx = self.dynamic_slot_field_idx(slot_plan.state_slot);
                    func.instruction(&Instruction::LocalGet(1)); // state ref
                    func.instruction(&Instruction::LocalGet(elem_local));
                    func.instruction(&Instruction::StructSet {
                        struct_type_index: struct_type_idx,
                        field_index: slot_field_idx,
                    });
                }
            }

            // Append child to parent
            let parent_local = if elem.parent_id == 0 {
                0 // parameter 0: root_id
            } else {
                let parent_idx = self
                    .plan
                    .elements
                    .iter()
                    .position(|e| e.id == elem.parent_id)
                    .unwrap();
                2 + parent_idx as u32
            };

            func.instruction(&Instruction::LocalGet(parent_local));
            func.instruction(&Instruction::LocalGet(elem_local));
            func.instruction(&Instruction::Call(Self::DOM_APPEND_CHILD_IMPORT_IDX));
        }

        // 3. Compute initial computeds
        for (i, comp) in self.plan.computeds.iter().enumerate() {
            let field_idx = self.computed_field_idx(i);
            func.instruction(&Instruction::LocalGet(1)); // state
            self.emit_expr(&comp.expr, struct_type_idx, extern_func_indices, &mut func)?;
            func.instruction(&Instruction::StructSet {
                struct_type_index: struct_type_idx,
                field_index: field_idx,
            });
        }

        // 4. Render initial dynamic text
        if self.plan.dynamic_text_slots > 0 {
            func.instruction(&Instruction::Call(num_imports + 1)); // render_dynamic_text
        }

        // Clear dirty mask to 0 after initial mount render
        func.instruction(&Instruction::LocalGet(1)); // state
        func.instruction(&Instruction::I32Const(0));
        func.instruction(&Instruction::StructSet {
            struct_type_index: struct_type_idx,
            field_index: Self::DIRTY_MASK_FIELD_IDX,
        });

        // 5. If onMount method exists, call it!
        if let Some(pos) = self.plan.methods.iter().position(|m| m.name == "onMount") {
            let on_mount_func_idx = method_start_func_idx + pos as u32;
            func.instruction(&Instruction::Call(on_mount_func_idx));
        }

        func.instruction(&Instruction::End);
        Ok(func)
    }

    fn emit_render_dynamic_text_function(
        &self,
        struct_type_idx: u32,
        _num_imports: u32,
        extern_func_indices: &std::collections::HashMap<String, u32>,
    ) -> Result<Function, String> {
        let locals = vec![(
            1,
            ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType::Concrete(struct_type_idx),
            }),
        )];
        let mut func = Function::new(locals);

        func.instruction(&Instruction::GlobalGet(0));
        func.instruction(&Instruction::LocalSet(0)); // local 0 = state

        for elem in &self.plan.elements {
            if let PlannedNodeKind::DynamicText(ref slot_plan) = elem.kind {
                // Guard: Only render if triggered by dirty_mask or on mount (-1)
                func.instruction(&Instruction::LocalGet(0));
                func.instruction(&Instruction::StructGet {
                    struct_type_index: struct_type_idx,
                    field_index: Self::DIRTY_MASK_FIELD_IDX,
                });
                if slot_plan.trigger_mask != 0 {
                    func.instruction(&Instruction::I32Const(slot_plan.trigger_mask as i32));
                    func.instruction(&Instruction::I32And);
                } else {
                    func.instruction(&Instruction::I32Const(-1));
                    func.instruction(&Instruction::I32Eq);
                }
                func.instruction(&Instruction::If(BlockType::Empty));

                // 1. Push node_id: struct.get state slot_field_idx
                let slot_field_idx = self.dynamic_slot_field_idx(slot_plan.state_slot);
                func.instruction(&Instruction::LocalGet(0));
                func.instruction(&Instruction::StructGet {
                    struct_type_index: struct_type_idx,
                    field_index: slot_field_idx,
                });

                // 2. Push numeric value: evaluate expr
                self.emit_expr(&slot_plan.expr, struct_type_idx, extern_func_indices, &mut func)?;

                // 3. Call dom.setTextNumber(node_id, val)
                func.instruction(&Instruction::Call(Self::DOM_SET_TEXT_NUMBER_IMPORT_IDX));

                func.instruction(&Instruction::End); // end if
            }
        }

        func.instruction(&Instruction::End);
        Ok(func)
    }

    fn emit_method_function(
        &self,
        _method_idx: usize,
        method: &FnDecl,
        struct_type_idx: u32,
        num_imports: u32,
        extern_func_indices: &std::collections::HashMap<String, u32>,
    ) -> Result<Function, String> {
        let locals = vec![(
            1,
            ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType::Concrete(struct_type_idx),
            }),
        )];
        let mut func = Function::new(locals);

        func.instruction(&Instruction::GlobalGet(0));
        func.instruction(&Instruction::LocalSet(0)); // local 0 = state

        for stmt in &method.body {
            match stmt {
                Stmt::Assign { target, value, .. } => {
                    if let Some(&sig_id) = self.plan.signal_name_to_id.get(target) {
                        let field_idx = self.signal_field_idx(sig_id);
                        func.instruction(&Instruction::LocalGet(0));
                        self.emit_expr(value, struct_type_idx, extern_func_indices, &mut func)?;
                        func.instruction(&Instruction::StructSet {
                            struct_type_index: struct_type_idx,
                            field_index: field_idx,
                        });

                        // Mark signal dirty in dirty_mask (field 0)
                        let sig_mask = 1i32 << (sig_id as u32);
                        func.instruction(&Instruction::LocalGet(0)); // for StructSet
                        func.instruction(&Instruction::LocalGet(0)); // for StructGet
                        func.instruction(&Instruction::StructGet {
                            struct_type_index: struct_type_idx,
                            field_index: Self::DIRTY_MASK_FIELD_IDX,
                        });
                        func.instruction(&Instruction::I32Const(sig_mask));
                        func.instruction(&Instruction::I32Or);
                        func.instruction(&Instruction::StructSet {
                            struct_type_index: struct_type_idx,
                            field_index: Self::DIRTY_MASK_FIELD_IDX,
                        });
                    }
                }
                Stmt::AssignOp {
                    target,
                    op,
                    value,
                    ..
                } => {
                    if let Some(&sig_id) = self.plan.signal_name_to_id.get(target) {
                        let field_idx = self.signal_field_idx(sig_id);
                        func.instruction(&Instruction::LocalGet(0)); // for StructSet
                        // Current value
                        func.instruction(&Instruction::LocalGet(0));
                        func.instruction(&Instruction::StructGet {
                            struct_type_index: struct_type_idx,
                            field_index: field_idx,
                        });
                        // Value operand
                        self.emit_expr(value, struct_type_idx, extern_func_indices, &mut func)?;
                        match op {
                            AssignOperator::AddEq => func.instruction(&Instruction::I32Add),
                            AssignOperator::SubEq => func.instruction(&Instruction::I32Sub),
                            AssignOperator::MulEq => func.instruction(&Instruction::I32Mul),
                            AssignOperator::DivEq => func.instruction(&Instruction::I32DivS),
                        };
                        func.instruction(&Instruction::StructSet {
                            struct_type_index: struct_type_idx,
                            field_index: field_idx,
                        });

                        // Mark signal dirty in dirty_mask (field 0)
                        let sig_mask = 1i32 << (sig_id as u32);
                        func.instruction(&Instruction::LocalGet(0)); // for StructSet
                        func.instruction(&Instruction::LocalGet(0)); // for StructGet
                        func.instruction(&Instruction::StructGet {
                            struct_type_index: struct_type_idx,
                            field_index: Self::DIRTY_MASK_FIELD_IDX,
                        });
                        func.instruction(&Instruction::I32Const(sig_mask));
                        func.instruction(&Instruction::I32Or);
                        func.instruction(&Instruction::StructSet {
                            struct_type_index: struct_type_idx,
                            field_index: Self::DIRTY_MASK_FIELD_IDX,
                        });
                    }
                }
                Stmt::Expr(expr) => {
                    self.emit_expr(expr, struct_type_idx, extern_func_indices, &mut func)?;
                    // If expression pushed a value on stack, drop it
                    if let Expr::Call { callee, .. } = expr {
                        // Check if called function returns a value
                        let has_return = self.extern_blocks.iter().any(|b| {
                            b.functions
                                .iter()
                                .any(|f| &f.name == callee && f.return_type.is_some())
                        });
                        if has_return {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                }
                _ => {}
            }
        }

        // Recompute computeds whose dependencies changed
        for (i, comp) in self.plan.computeds.iter().enumerate() {
            if comp.mask != 0 {
                let field_idx = self.computed_field_idx(i);
                func.instruction(&Instruction::LocalGet(0));
                func.instruction(&Instruction::StructGet {
                    struct_type_index: struct_type_idx,
                    field_index: Self::DIRTY_MASK_FIELD_IDX,
                });
                func.instruction(&Instruction::I32Const(comp.mask as i32));
                func.instruction(&Instruction::I32And);
                func.instruction(&Instruction::If(BlockType::Empty));

                func.instruction(&Instruction::LocalGet(0));
                self.emit_expr(&comp.expr, struct_type_idx, extern_func_indices, &mut func)?;
                func.instruction(&Instruction::StructSet {
                    struct_type_index: struct_type_idx,
                    field_index: field_idx,
                });

                func.instruction(&Instruction::End); // end if
            }
        }

        // Trigger dynamic text update if dirty_mask != 0
        if self.plan.dynamic_text_slots > 0 {
            func.instruction(&Instruction::LocalGet(0));
            func.instruction(&Instruction::StructGet {
                struct_type_index: struct_type_idx,
                field_index: Self::DIRTY_MASK_FIELD_IDX,
            });
            func.instruction(&Instruction::If(BlockType::Empty));
            func.instruction(&Instruction::Call(num_imports + 1)); // render_dynamic_text
            func.instruction(&Instruction::End);
        }

        // Clear dirty_mask to 0 at end of method
        func.instruction(&Instruction::LocalGet(0));
        func.instruction(&Instruction::I32Const(0));
        func.instruction(&Instruction::StructSet {
            struct_type_index: struct_type_idx,
            field_index: Self::DIRTY_MASK_FIELD_IDX,
        });

        func.instruction(&Instruction::End);
        Ok(func)
    }

    fn emit_getter_function(&self, field_idx: u32, struct_type_idx: u32) -> Function {
        let mut func = Function::new(vec![]);
        func.instruction(&Instruction::GlobalGet(0));
        func.instruction(&Instruction::StructGet {
            struct_type_index: struct_type_idx,
            field_index: field_idx,
        });
        func.instruction(&Instruction::End);
        func
    }

    fn emit_expr(
        &self,
        expr: &Expr,
        struct_type_idx: u32,
        extern_func_indices: &std::collections::HashMap<String, u32>,
        func: &mut Function,
    ) -> Result<(), String> {
        match expr {
            Expr::IntLit(n, _) => {
                func.instruction(&Instruction::I32Const(*n as i32));
            }
            Expr::BoolLit(b, _) => {
                func.instruction(&Instruction::I32Const(if *b { 1 } else { 0 }));
            }
            Expr::StringLit(s, _) => {
                let entry = self.string_table.get(s).unwrap();
                func.instruction(&Instruction::I32Const(entry.offset as i32));
            }
            Expr::Ident(name, _) => {
                if let Some(&sig_id) = self.plan.signal_name_to_id.get(name) {
                    func.instruction(&Instruction::GlobalGet(0));
                    func.instruction(&Instruction::StructGet {
                        struct_type_index: struct_type_idx,
                        field_index: self.signal_field_idx(sig_id),
                    });
                } else if let Some(&comp_id) = self.plan.computed_name_to_id.get(name) {
                    func.instruction(&Instruction::GlobalGet(0));
                    func.instruction(&Instruction::StructGet {
                        struct_type_index: struct_type_idx,
                        field_index: self.computed_field_idx(comp_id),
                    });
                } else {
                    return Err(format!("Unknown identifier '{}' in expression", name));
                }
            }
            Expr::Call { callee, args, .. } => {
                if let Some(&call_idx) = extern_func_indices.get(callee) {
                    for arg in args {
                        self.emit_expr(arg, struct_type_idx, extern_func_indices, func)?;
                    }
                    func.instruction(&Instruction::Call(call_idx));
                } else {
                    return Err(format!("Unknown function '{}' in call expression", callee));
                }
            }
            Expr::Binary { op, left, right, .. } => {
                self.emit_expr(left, struct_type_idx, extern_func_indices, func)?;
                self.emit_expr(right, struct_type_idx, extern_func_indices, func)?;
                match op {
                    BinaryOp::Add => func.instruction(&Instruction::I32Add),
                    BinaryOp::Sub => func.instruction(&Instruction::I32Sub),
                    BinaryOp::Mul => func.instruction(&Instruction::I32Mul),
                    BinaryOp::Div => func.instruction(&Instruction::I32DivS),
                    BinaryOp::Mod => func.instruction(&Instruction::I32RemS),
                    BinaryOp::Eq => func.instruction(&Instruction::I32Eq),
                    BinaryOp::NotEq => func.instruction(&Instruction::I32Ne),
                    BinaryOp::Lt => func.instruction(&Instruction::I32LtS),
                    BinaryOp::LtEq => func.instruction(&Instruction::I32LeS),
                    BinaryOp::Gt => func.instruction(&Instruction::I32GtS),
                    BinaryOp::GtEq => func.instruction(&Instruction::I32GeS),
                    BinaryOp::And => func.instruction(&Instruction::I32And),
                    BinaryOp::Or => func.instruction(&Instruction::I32Or),
                };
            }
            Expr::Unary { op, expr, .. } => {
                self.emit_expr(expr, struct_type_idx, extern_func_indices, func)?;
                match op {
                    UnaryOp::Neg => {
                        func.instruction(&Instruction::I32Const(0));
                        func.instruction(&Instruction::I32Sub);
                    }
                    UnaryOp::Not => {
                        func.instruction(&Instruction::I32Eqz);
                    }
                }
            }
            _ => {
                return Err(format!("Expression type {:?} not yet supported in codegen", expr));
            }
        }
        Ok(())
    }

    fn emit_const_i32_expr(&self, expr: &Expr, func: &mut Function) {
        match expr {
            Expr::IntLit(n, _) => {
                func.instruction(&Instruction::I32Const(*n as i32));
            }
            Expr::BoolLit(b, _) => {
                func.instruction(&Instruction::I32Const(if *b { 1 } else { 0 }));
            }
            _ => {
                func.instruction(&Instruction::I32Const(0));
            }
        }
    }
}
