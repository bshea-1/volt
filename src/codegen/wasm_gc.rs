use crate::analysis::reactivity::{ComponentPlan, TextPart};
use crate::codegen::memory_layout::{StringTable, DATA_START, SCRATCH_OFFSET};
use crate::syntax::ast::*;
use wasm_encoder::*;

pub struct WasmGcCompiler {
    plan: ComponentPlan,
    extern_blocks: Vec<ExternBlock>,
    string_table: StringTable,
}

impl WasmGcCompiler {
    pub fn new(plan: ComponentPlan, extern_blocks: Vec<ExternBlock>) -> Self {
        Self {
            plan,
            extern_blocks,
            string_table: StringTable::new(),
        }
    }

    pub fn compile(&mut self) -> Result<Vec<u8>, String> {
        let mut module = Module::new();

        // 1. Populate string table with all static strings
        self.collect_strings();

        // 2. Type Section
        let mut types = TypeSection::new();
        // Type indices:
        // 0: sig_dom_create (i32, i32) -> (i32)
        types.ty().function(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
        // 1: sig_dom_set_text (i32, i32, i32) -> ()
        types.ty().function(vec![ValType::I32, ValType::I32, ValType::I32], vec![]);
        // 2: sig_dom_set_attr (i32, i32, i32, i32, i32) -> ()
        types.ty().function(
            vec![ValType::I32, ValType::I32, ValType::I32, ValType::I32, ValType::I32],
            vec![],
        );
        // 3: sig_dom_append (i32, i32) -> ()
        types.ty().function(vec![ValType::I32, ValType::I32], vec![]);
        // 4: sig_dom_add_event (i32, i32, i32, i32) -> ()
        types.ty().function(
            vec![ValType::I32, ValType::I32, ValType::I32, ValType::I32],
            vec![],
        );
        // 5: sig_mount (i32) -> ()
        types.ty().function(vec![ValType::I32], vec![]);
        // 6: sig_i32_to_str (i32, i32) -> (i32)
        types.ty().function(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
        // 7: sig_void () -> ()
        types.ty().function(vec![], vec![]);
        // 8: sig_getter () -> (i32)
        types.ty().function(vec![], vec![ValType::I32]);

        // GC Struct Type: ComponentState
        // Total fields = signals (i32) + computeds (i32) + dynamic_text_slots (i32)
        let total_fields = self.plan.signals.len() + self.plan.computeds.len() + self.plan.dynamic_text_slots;
        let mut struct_fields = Vec::new();
        for _ in 0..total_fields {
            struct_fields.push(FieldType {
                element_type: StorageType::Val(ValType::I32),
                mutable: true,
            });
        }
        let struct_type = StructType {
            fields: struct_fields.into_boxed_slice(),
        };
        let struct_type_idx = 9;
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
        // 0: dom.createElement
        imports.import("dom", "createElement", EntityType::Function(0));
        // 1: dom.createTextNode
        imports.import("dom", "createTextNode", EntityType::Function(0));
        // 2: dom.setTextContent
        imports.import("dom", "setTextContent", EntityType::Function(1));
        // 3: dom.setAttribute
        imports.import("dom", "setAttribute", EntityType::Function(2));
        // 4: dom.appendChild
        imports.import("dom", "appendChild", EntityType::Function(3));
        // 5: dom.addEventListener
        imports.import("dom", "addEventListener", EntityType::Function(4));

        let mut num_imports = 6u32;
        let mut extern_func_indices = std::collections::HashMap::new();

        // Add any foreign functions from extern blocks
        for block in &self.extern_blocks {
            for func in &block.functions {
                let type_idx = if func.params.len() == 2 && func.return_type.is_some() {
                    0 // (i32, i32) -> i32
                } else if func.params.len() == 2 {
                    3 // (i32, i32) -> ()
                } else if func.params.len() == 1 && func.return_type.is_none() {
                    5 // (i32) -> ()
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
        // func 6: mount (type 5: [i32] -> [])
        // func 7: i32_to_str (type 6: [i32, i32] -> [i32])
        // func 8: render_dynamic_text (type 7: [] -> [])
        // func 9..: methods (type 7: [] -> [])
        // func (after methods): getters for signals/computeds (type 8: [] -> [i32])
        let mut functions = FunctionSection::new();
        functions.function(5); // mount
        functions.function(6); // i32_to_str
        functions.function(7); // render_dynamic_text

        let method_start_func_idx = num_imports + 3;
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

        // i32_to_str(val: i32, out_ptr: i32) -> len: i32
        codes.function(&self.emit_i32_to_str_function());

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
            codes.function(&self.emit_getter_function(sig.id as u32, struct_type_idx));
        }
        for comp in &self.plan.computeds {
            let field_idx = (self.plan.signals.len() + comp.id) as u32;
            codes.function(&self.emit_getter_function(field_idx, struct_type_idx));
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
            self.string_table.insert(&elem.tag);
            for (attr_name, attr_val) in &elem.static_attrs {
                self.string_table.insert(attr_name);
                self.string_table.insert(attr_val);
            }
            for (ev_name, _) in &elem.events {
                self.string_table.insert(ev_name);
            }
            if let Some(ref text) = elem.static_text {
                self.string_table.insert(text);
            }
            if let Some(ref dyn_plan) = elem.dynamic_text {
                for part in &dyn_plan.parts {
                    if let TextPart::Lit(s) = part {
                        self.string_table.insert(s);
                    }
                }
            }
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
        // param 0: root_id (i32)
        // local 1: state struct ref
        // local 2..: local vars for created DOM element node IDs (one for each planned element)
        let num_elements = self.plan.elements.len();
        let locals = vec![
            (
                1,
                ValType::Ref(RefType {
                    nullable: true,
                    heap_type: HeapType::Concrete(struct_type_idx),
                }),
            ),
            (num_elements as u32, ValType::I32),
        ];

        let mut func = Function::new(locals);

        // 1. Allocate ComponentState GC struct
        // Push initial values for all signals
        for sig in &self.plan.signals {
            self.emit_const_i32_expr(&sig.init_expr, &mut func);
        }
        // Push initial values for computeds (0 initially)
        for _ in &self.plan.computeds {
            func.instruction(&Instruction::I32Const(0));
        }
        // Push initial values for dynamic text node slots (0 initially)
        for _ in 0..self.plan.dynamic_text_slots {
            func.instruction(&Instruction::I32Const(0));
        }

        func.instruction(&Instruction::StructNew(struct_type_idx));
        func.instruction(&Instruction::LocalSet(1)); // local 1 = state
        func.instruction(&Instruction::LocalGet(1));
        func.instruction(&Instruction::GlobalSet(0)); // global 0 = state

        // 2. Instantiate DOM elements
        for (i, elem) in self.plan.elements.iter().enumerate() {
            let elem_local = 2 + i as u32;
            let tag_entry = self.string_table.get(&elem.tag).unwrap();

            // dom.createElement(tagPtr, tagLen) -> nodeId
            func.instruction(&Instruction::I32Const(tag_entry.offset as i32));
            func.instruction(&Instruction::I32Const(tag_entry.len as i32));
            func.instruction(&Instruction::Call(0)); // dom.createElement
            func.instruction(&Instruction::LocalSet(elem_local));

            // Set static attributes
            for (attr_name, attr_val) in &elem.static_attrs {
                let name_entry = self.string_table.get(attr_name).unwrap();
                let val_entry = self.string_table.get(attr_val).unwrap();

                func.instruction(&Instruction::LocalGet(elem_local));
                func.instruction(&Instruction::I32Const(name_entry.offset as i32));
                func.instruction(&Instruction::I32Const(name_entry.len as i32));
                func.instruction(&Instruction::I32Const(val_entry.offset as i32));
                func.instruction(&Instruction::I32Const(val_entry.len as i32));
                func.instruction(&Instruction::Call(3)); // dom.setAttribute
            }

            // Set static text content if present
            if let Some(ref static_text) = elem.static_text {
                let text_entry = self.string_table.get(static_text).unwrap();
                func.instruction(&Instruction::LocalGet(elem_local));
                func.instruction(&Instruction::I32Const(text_entry.offset as i32));
                func.instruction(&Instruction::I32Const(text_entry.len as i32));
                func.instruction(&Instruction::Call(2)); // dom.setTextContent
            }

            // If dynamic text, save node ID into component state struct slot
            if let Some(ref dyn_plan) = elem.dynamic_text {
                let slot_field_idx = (self.plan.signals.len()
                    + self.plan.computeds.len()
                    + dyn_plan.state_slot) as u32;

                func.instruction(&Instruction::LocalGet(1)); // state ref
                func.instruction(&Instruction::LocalGet(elem_local));
                func.instruction(&Instruction::StructSet {
                    struct_type_index: struct_type_idx,
                    field_index: slot_field_idx,
                });
            }

            // Attach event listeners
            for (ev_name, handler_name) in &elem.events {
                let ev_entry = self.string_table.get(ev_name).unwrap();
                let table_idx = self
                    .plan
                    .methods
                    .iter()
                    .position(|m| &m.name == handler_name)
                    .ok_or_else(|| format!("Handler '{}' not found", handler_name))?
                    as i32;

                func.instruction(&Instruction::LocalGet(elem_local));
                func.instruction(&Instruction::I32Const(ev_entry.offset as i32));
                func.instruction(&Instruction::I32Const(ev_entry.len as i32));
                func.instruction(&Instruction::I32Const(table_idx));
                func.instruction(&Instruction::Call(5)); // dom.addEventListener
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
            func.instruction(&Instruction::Call(4)); // dom.appendChild
        }

        // 3. Compute initial computeds
        for (i, comp) in self.plan.computeds.iter().enumerate() {
            let field_idx = (self.plan.signals.len() + i) as u32;
            func.instruction(&Instruction::LocalGet(1)); // state
            self.emit_expr(&comp.expr, struct_type_idx, extern_func_indices, &mut func)?;
            func.instruction(&Instruction::StructSet {
                struct_type_index: struct_type_idx,
                field_index: field_idx,
            });
        }

        // 4. Render initial dynamic text
        if self.plan.dynamic_text_slots > 0 {
            func.instruction(&Instruction::Call(num_imports + 2)); // render_dynamic_text
        }

        // 5. If onMount method exists, call it!
        if let Some(pos) = self.plan.methods.iter().position(|m| m.name == "onMount") {
            let on_mount_func_idx = method_start_func_idx + pos as u32;
            func.instruction(&Instruction::Call(on_mount_func_idx));
        }

        func.instruction(&Instruction::End);
        Ok(func)
    }

    fn emit_i32_to_str_function(&self) -> Function {
        // Param 0: val (i32)
        // Param 1: out_ptr (i32)
        // Local 2: is_neg (i32)
        // Local 3: len (i32)
        // Local 4: temp_len (i32)
        // Local 5: rem (i32)
        // Local 6: char (i32)
        let locals = vec![(5, ValType::I32)];
        let mut func = Function::new(locals);

        // If val == 0: store '0' (48), return 1
        func.instruction(&Instruction::LocalGet(0));
        func.instruction(&Instruction::I32Eqz);
        func.instruction(&Instruction::If(BlockType::Empty));
        func.instruction(&Instruction::LocalGet(1));
        func.instruction(&Instruction::I32Const(48)); // '0'
        func.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        func.instruction(&Instruction::I32Const(1));
        func.instruction(&Instruction::Return);
        func.instruction(&Instruction::End);

        // Check if val < 0
        func.instruction(&Instruction::LocalGet(0));
        func.instruction(&Instruction::I32Const(0));
        func.instruction(&Instruction::I32LtS);
        func.instruction(&Instruction::If(BlockType::Empty));
        func.instruction(&Instruction::I32Const(1));
        func.instruction(&Instruction::LocalSet(2)); // is_neg = 1
        func.instruction(&Instruction::I32Const(0));
        func.instruction(&Instruction::LocalGet(0));
        func.instruction(&Instruction::I32Sub);
        func.instruction(&Instruction::LocalSet(0)); // val = -val
        func.instruction(&Instruction::End);

        // Extract digits into out_ptr + 16
        func.instruction(&Instruction::I32Const(0));
        func.instruction(&Instruction::LocalSet(4)); // temp_len = 0

        func.instruction(&Instruction::Loop(BlockType::Empty));
        // rem = val % 10
        func.instruction(&Instruction::LocalGet(0));
        func.instruction(&Instruction::I32Const(10));
        func.instruction(&Instruction::I32RemU);
        func.instruction(&Instruction::LocalSet(5));

        // store8(out_ptr + 16 + temp_len, 48 + rem)
        func.instruction(&Instruction::LocalGet(1));
        func.instruction(&Instruction::I32Const(16));
        func.instruction(&Instruction::I32Add);
        func.instruction(&Instruction::LocalGet(4));
        func.instruction(&Instruction::I32Add);
        func.instruction(&Instruction::I32Const(48));
        func.instruction(&Instruction::LocalGet(5));
        func.instruction(&Instruction::I32Add);
        func.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));

        // temp_len += 1
        func.instruction(&Instruction::LocalGet(4));
        func.instruction(&Instruction::I32Const(1));
        func.instruction(&Instruction::I32Add);
        func.instruction(&Instruction::LocalSet(4));

        // val = val / 10
        func.instruction(&Instruction::LocalGet(0));
        func.instruction(&Instruction::I32Const(10));
        func.instruction(&Instruction::I32DivU);
        func.instruction(&Instruction::LocalTee(0));

        func.instruction(&Instruction::I32Const(0));
        func.instruction(&Instruction::I32GtU);
        func.instruction(&Instruction::BrIf(0));
        func.instruction(&Instruction::End);

        // Now reverse digits into out_ptr
        func.instruction(&Instruction::I32Const(0));
        func.instruction(&Instruction::LocalSet(3)); // len = 0

        // If is_neg: write '-' (45)
        func.instruction(&Instruction::LocalGet(2));
        func.instruction(&Instruction::If(BlockType::Empty));
        func.instruction(&Instruction::LocalGet(1));
        func.instruction(&Instruction::I32Const(45)); // '-'
        func.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        func.instruction(&Instruction::I32Const(1));
        func.instruction(&Instruction::LocalSet(3)); // len = 1
        func.instruction(&Instruction::End);

        // Loop temp_len > 0
        func.instruction(&Instruction::Block(BlockType::Empty));
        func.instruction(&Instruction::Loop(BlockType::Empty));
        func.instruction(&Instruction::LocalGet(4));
        func.instruction(&Instruction::I32Eqz);
        func.instruction(&Instruction::BrIf(1)); // break to outer block

        // temp_len -= 1
        func.instruction(&Instruction::LocalGet(4));
        func.instruction(&Instruction::I32Const(1));
        func.instruction(&Instruction::I32Sub);
        func.instruction(&Instruction::LocalSet(4));

        // char = load8(out_ptr + 16 + temp_len)
        func.instruction(&Instruction::LocalGet(1));
        func.instruction(&Instruction::I32Const(16));
        func.instruction(&Instruction::I32Add);
        func.instruction(&Instruction::LocalGet(4));
        func.instruction(&Instruction::I32Add);
        func.instruction(&Instruction::I32Load8U(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        func.instruction(&Instruction::LocalSet(6));

        // store8(out_ptr + len, char)
        func.instruction(&Instruction::LocalGet(1));
        func.instruction(&Instruction::LocalGet(3));
        func.instruction(&Instruction::I32Add);
        func.instruction(&Instruction::LocalGet(6));
        func.instruction(&Instruction::I32Store8(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));

        // len += 1
        func.instruction(&Instruction::LocalGet(3));
        func.instruction(&Instruction::I32Const(1));
        func.instruction(&Instruction::I32Add);
        func.instruction(&Instruction::LocalSet(3));

        func.instruction(&Instruction::Br(0)); // continue loop
        func.instruction(&Instruction::End); // end loop
        func.instruction(&Instruction::End); // end block

        func.instruction(&Instruction::LocalGet(3)); // return len
        func.instruction(&Instruction::End);
        func
    }

    fn emit_render_dynamic_text_function(
        &self,
        struct_type_idx: u32,
        num_imports: u32,
        extern_func_indices: &std::collections::HashMap<String, u32>,
    ) -> Result<Function, String> {
        // Local 0: state ref
        // Local 1: curr_scratch_ptr (i32)
        // Local 2: written_len (i32)
        // Local 3: node_id (i32)
        // Local 4: loop counter (i32)
        let locals = vec![
            (
                1,
                ValType::Ref(RefType {
                    nullable: true,
                    heap_type: HeapType::Concrete(struct_type_idx),
                }),
            ),
            (4, ValType::I32),
        ];
        let mut func = Function::new(locals);

        func.instruction(&Instruction::GlobalGet(0));
        func.instruction(&Instruction::LocalSet(0)); // local 0 = state

        for elem in &self.plan.elements {
            if let Some(ref dyn_plan) = elem.dynamic_text {
                // Initialize scratch pointer
                func.instruction(&Instruction::I32Const(SCRATCH_OFFSET as i32));
                func.instruction(&Instruction::LocalSet(1)); // curr_ptr = SCRATCH_OFFSET

                for part in &dyn_plan.parts {
                    match part {
                        TextPart::Lit(lit) => {
                            let entry = self.string_table.get(lit).unwrap();
                            // Copy `len` bytes from `entry.offset` to `curr_ptr`
                            func.instruction(&Instruction::I32Const(0));
                            func.instruction(&Instruction::LocalSet(4)); // i = 0

                            func.instruction(&Instruction::Block(BlockType::Empty));
                            func.instruction(&Instruction::Loop(BlockType::Empty));
                            func.instruction(&Instruction::LocalGet(4));
                            func.instruction(&Instruction::I32Const(entry.len as i32));
                            func.instruction(&Instruction::I32GeU);
                            func.instruction(&Instruction::BrIf(1)); // break

                            // store8(curr_ptr + i, load8(offset + i))
                            func.instruction(&Instruction::LocalGet(1));
                            func.instruction(&Instruction::LocalGet(4));
                            func.instruction(&Instruction::I32Add); // curr_ptr + i

                            func.instruction(&Instruction::I32Const(entry.offset as i32));
                            func.instruction(&Instruction::LocalGet(4));
                            func.instruction(&Instruction::I32Add); // offset + i
                            func.instruction(&Instruction::I32Load8U(MemArg {
                                offset: 0,
                                align: 0,
                                memory_index: 0,
                            }));

                            func.instruction(&Instruction::I32Store8(MemArg {
                                offset: 0,
                                align: 0,
                                memory_index: 0,
                            }));

                            // i += 1
                            func.instruction(&Instruction::LocalGet(4));
                            func.instruction(&Instruction::I32Const(1));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalSet(4));
                            func.instruction(&Instruction::Br(0));
                            func.instruction(&Instruction::End); // end loop
                            func.instruction(&Instruction::End); // end block

                            // curr_ptr += len
                            func.instruction(&Instruction::LocalGet(1));
                            func.instruction(&Instruction::I32Const(entry.len as i32));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalSet(1));
                        }
                        TextPart::Expr(expr) => {
                            // Evaluate expr
                            self.emit_expr(expr, struct_type_idx, extern_func_indices, &mut func)?;
                            func.instruction(&Instruction::LocalGet(1)); // out_ptr = curr_ptr
                            func.instruction(&Instruction::Call(num_imports + 1)); // i32_to_str(val, curr_ptr) -> written_len
                            func.instruction(&Instruction::LocalSet(2)); // written_len

                            // curr_ptr += written_len
                            func.instruction(&Instruction::LocalGet(1));
                            func.instruction(&Instruction::LocalGet(2));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalSet(1));
                        }
                    }
                }

                // Node ID from state struct
                let slot_field_idx = (self.plan.signals.len()
                    + self.plan.computeds.len()
                    + dyn_plan.state_slot) as u32;

                func.instruction(&Instruction::LocalGet(0));
                func.instruction(&Instruction::StructGet {
                    struct_type_index: struct_type_idx,
                    field_index: slot_field_idx,
                });
                func.instruction(&Instruction::LocalSet(3)); // node_id

                // Call dom.setTextContent(node_id, SCRATCH_OFFSET, total_len)
                func.instruction(&Instruction::LocalGet(3));
                func.instruction(&Instruction::I32Const(SCRATCH_OFFSET as i32));
                func.instruction(&Instruction::LocalGet(1));
                func.instruction(&Instruction::I32Const(SCRATCH_OFFSET as i32));
                func.instruction(&Instruction::I32Sub); // total_len = curr_ptr - SCRATCH_OFFSET
                func.instruction(&Instruction::Call(2)); // dom.setTextContent
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
                        func.instruction(&Instruction::LocalGet(0));
                        self.emit_expr(value, struct_type_idx, extern_func_indices, &mut func)?;
                        func.instruction(&Instruction::StructSet {
                            struct_type_index: struct_type_idx,
                            field_index: sig_id as u32,
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
                        func.instruction(&Instruction::LocalGet(0)); // for StructSet
                        // Current value
                        func.instruction(&Instruction::LocalGet(0));
                        func.instruction(&Instruction::StructGet {
                            struct_type_index: struct_type_idx,
                            field_index: sig_id as u32,
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
                            field_index: sig_id as u32,
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

        // Recompute all computeds
        for (i, comp) in self.plan.computeds.iter().enumerate() {
            let field_idx = (self.plan.signals.len() + i) as u32;
            func.instruction(&Instruction::LocalGet(0));
            self.emit_expr(&comp.expr, struct_type_idx, extern_func_indices, &mut func)?;
            func.instruction(&Instruction::StructSet {
                struct_type_index: struct_type_idx,
                field_index: field_idx,
            });
        }

        // Trigger dynamic text update
        if self.plan.dynamic_text_slots > 0 {
            func.instruction(&Instruction::Call(num_imports + 2)); // render_dynamic_text
        }

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
                        field_index: sig_id as u32,
                    });
                } else if let Some(&comp_id) = self.plan.computed_name_to_id.get(name) {
                    let field_idx = (self.plan.signals.len() + comp_id) as u32;
                    func.instruction(&Instruction::GlobalGet(0));
                    func.instruction(&Instruction::StructGet {
                        struct_type_index: struct_type_idx,
                        field_index: field_idx,
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
