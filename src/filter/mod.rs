//! PocketBase-compatible filter expressions, used for the `filter` query parameter and for
//! collection API rules.

pub mod compile;
pub mod parser;
pub mod sort;

pub use compile::{CompileError, Params, RequestInfo, Scope, compile, quote_ident};
pub use parser::{Expr, ParseError, parse, parse_with_params};
pub use sort::compile_sort;

/// Parses and compiles an expression in one step.
pub fn to_sql(input: &str, scope: &Scope, params: &mut Params) -> Result<String, CompileError> {
    let expr = parse(input).map_err(|e| CompileError(e.0))?;
    compile(&expr, scope, params)
}

/// Like [`to_sql`], with `{:name}` placeholders replaced by `values`.
pub fn to_sql_with_params(
    input: &str,
    values: &serde_json::Map<String, serde_json::Value>,
    scope: &Scope,
    params: &mut Params,
) -> Result<String, CompileError> {
    let expr = parse_with_params(input, values).map_err(|e| CompileError(e.0))?;
    compile(&expr, scope, params)
}
