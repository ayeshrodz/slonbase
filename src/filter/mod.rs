//! PocketBase-compatible filter expressions, used for the `filter` query parameter and for
//! collection API rules.

pub mod compile;
pub mod parser;
pub mod sort;

pub use compile::{CompileError, Params, RequestInfo, Scope, compile, quote_ident};
pub use parser::{Expr, ParseError, parse};
pub use sort::compile_sort;

/// Parses and compiles an expression in one step.
pub fn to_sql(input: &str, scope: &Scope, params: &mut Params) -> Result<String, CompileError> {
    let expr = parse(input).map_err(|e| CompileError(e.0))?;
    compile(&expr, scope, params)
}
