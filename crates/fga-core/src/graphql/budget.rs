//! A cap on how much one GraphQL request may build, counted while its fields resolve. The
//! complexity limit bounds how many fields a document selects, but one field can be an issue body
//! or a file, an alias can be long, and a list repeats its selection for every item: the cap
//! bounds what all of that adds up to before the response is written out.
//!
//! What it counts is the response's JSON: each string at its escaped length; each object entry
//! at its escaped name plus [`PER_ENTRY`]; each list item at [`PER_ITEM`]; and each error a field
//! returns at its message and path plus [`PER_ERROR`]. Validation errors come before any field
//! resolves, so fga-server bounds those instead, by request size and error count. A field's
//! object or list is charged when the field
//! resolves, after its contents. Its names are the document's own, shared until the response is
//! written, so charging them then still comes before they cost anything. The one thing left
//! uncharged is a `__typename` in the root object, which reaches no hook and has no field above
//! it; that output is about the document's own size, which the request size limit bounds.
//!
//! The field that would pass the budget fails with `RESOURCE_LIMITS_EXCEEDED`, and every field
//! that starts after it is `null` without resolving, so it starts no Forgejo request. Fields
//! already resolving alongside it finish, since async-graphql resolves siblings together: the
//! budget bounds the response's size, not the upstream work. That `null` fills non-null fields
//! too, so a stopped response breaks its schema; its one error tells the client the data is
//! incomplete.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextResolve, ResolveInfo,
};
use async_graphql::{ErrorExtensionValues, PathSegment, ServerError, ServerResult, Value};

/// An object entry besides its name: its punctuation and a value the entry's own field doesn't
/// charge (a number, boolean, null, or a `__typename`, which reaches no extension). Every type
/// name is shorter: `graphql::tests` checks.
pub(super) const PER_ENTRY: usize = 64;

/// A list item besides its own value: its punctuation, and a number or boolean.
const PER_ITEM: usize = 24;

/// An error besides its message and path: its other keys and punctuation.
const PER_ERROR: usize = 128;

/// The `type` of the budget's own error, GitHub's for a query past its resource limits.
const BUDGET_ERROR: &str = "RESOURCE_LIMITS_EXCEEDED";

/// A response budget: each request may build this many bytes of response.
pub(super) struct OutputBudget(pub(super) usize);

impl ExtensionFactory for OutputBudget {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(Spending {
            budget: self.0,
            left: AtomicUsize::new(self.0),
            spent: AtomicBool::new(false),
        })
    }
}

/// One request's budget: see the module documentation.
struct Spending {
    budget: usize,
    left: AtomicUsize,
    spent: AtomicBool,
}

#[async_graphql::async_trait::async_trait]
impl Extension for Spending {
    async fn resolve(
        &self,
        ctx: &ExtensionContext<'_>,
        info: ResolveInfo<'_>,
        next: NextResolve<'_>,
    ) -> ServerResult<Option<Value>> {
        // Never `None`: a list item that resolves to `None` panics in async-graphql.
        if self.spent.load(Ordering::Relaxed) {
            return Ok(Some(Value::Null));
        }
        // No field holds the root object, so its entries are charged by the fields in it.
        let root_entry = if info.path_node.parent.is_none() {
            json_len(info.alias.unwrap_or(info.name)) + PER_ENTRY
        } else {
            0
        };
        let result = next.run(ctx, info).await;
        let cost = root_entry
            + match &result {
                Ok(Some(value)) => value_cost(value),
                Ok(None) => 0,
                Err(error) => error_cost(error),
            };
        if self
            .left
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                left.checked_sub(cost)
            })
            .is_ok()
        {
            return result;
        }
        if self.spent.swap(true, Ordering::Relaxed) {
            // Only the budget's own error passes up, so it reaches the response. Any other error
            // that lands after the budget ran out, such as a field's that was waiting on Forgejo,
            // is dropped like its value: each carries its path, and many may land at once.
            return match result {
                Err(error) if is_budget_error(&error) => Err(error),
                _ => Ok(Some(Value::Null)),
            };
        }
        let mut error = ServerError::new(
            format!(
                "This query's response would pass {} bytes. Ask for fewer fields, aliases or items.",
                self.budget
            ),
            None,
        );
        let mut extensions = ErrorExtensionValues::default();
        extensions.set("type", BUDGET_ERROR);
        error.extensions = Some(extensions);
        Err(error)
    }
}

fn is_budget_error(error: &ServerError) -> bool {
    error
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("type"))
        .is_some_and(|kind| *kind == Value::String(BUDGET_ERROR.to_owned()))
}

/// What a field's value adds to the response beyond what its own fields and items were charged.
fn value_cost(value: &Value) -> usize {
    match value {
        Value::String(text) => json_len(text),
        Value::Enum(name) => name.len(),
        // Each byte as a number of up to three digits and a comma.
        Value::Binary(bytes) => bytes.len() * 4,
        Value::List(items) => items.len() * PER_ITEM,
        Value::Object(entries) => entries.keys().map(|name| json_len(name) + PER_ENTRY).sum(),
        Value::Null | Value::Number(_) | Value::Boolean(_) => 0,
    }
}

fn error_cost(error: &ServerError) -> usize {
    let path: usize = error
        .path
        .iter()
        .map(|segment| match segment {
            PathSegment::Field(name) => json_len(name) + 3,
            PathSegment::Index(_) => 21,
        })
        .sum();
    json_len(&error.message) + path + PER_ERROR
}

/// `text`'s length as a JSON string body: serde_json writes `"` and `\` as two bytes, the named
/// control characters as two, and the rest below 0x20 as six (`\u00XX`).
pub(super) fn json_len(text: &str) -> usize {
    text.bytes()
        .map(|byte| match byte {
            b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 0x08 | 0x0c => 2,
            0..=0x1f => 6,
            _ => 1,
        })
        .sum()
}
