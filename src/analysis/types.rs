pub use crate::syntax::ast::Type;

impl Type {
    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            Type::I32 | Type::I64 | Type::U32 | Type::U64 | Type::F32 | Type::F64
        )
    }

    pub fn is_integer(&self) -> bool {
        matches!(self, Type::I32 | Type::I64 | Type::U32 | Type::U64)
    }

    pub fn is_compatible_with(&self, other: &Type) -> bool {
        if self == other {
            return true;
        }
        // Numeric conversions or auto-coercion can be checked here
        false
    }
}
