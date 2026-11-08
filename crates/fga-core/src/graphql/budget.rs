//! A cap on how much one GraphQL request may build, counted while its fields resolve. The
//! complexity limit bounds how many fields a document selects, but one field can be an issue body
//! or a file, and a list can repeat a selection a hundred times: the cap bounds what all of that
//! adds up to before the response is assembled.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextResolve, ResolveInfo,
};
use async_graphql::{ErrorExtensionValues, ServerError, ServerResult, Value};

/// What each value costs besides its text: its name and punctuation in the response.
const PER_VALUE: usize = 16;

/// One request may build `0` bytes of values.
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

/// One request's budget. Each field's value is charged as it resolves: a string its length, any
/// other value [`PER_VALUE`]. A list's items and an object's fields resolve, and are charged, on
/// their own. The field that would pass the budget fails with `RESOURCE_LIMITS_EXCEEDED`; every
/// field after it is `null` without resolving, so the response carries that one error and no
/// further Forgejo request runs.
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
        let value = next.run(ctx, info).await?;
        let cost = match &value {
            Some(Value::String(text)) => text.len() + PER_VALUE,
            Some(Value::Binary(bytes)) => bytes.len() + PER_VALUE,
            _ => PER_VALUE,
        };
        if self
            .left
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                left.checked_sub(cost)
            })
            .is_ok()
        {
            return Ok(value);
        }
        if self.spent.swap(true, Ordering::Relaxed) {
            return Ok(Some(Value::Null));
        }
        let mut error = ServerError::new(
            format!(
                "This query's response would pass {} bytes. Ask for fewer fields, aliases or items.",
                self.budget
            ),
            None,
        );
        let mut extensions = ErrorExtensionValues::default();
        extensions.set("type", "RESOURCE_LIMITS_EXCEEDED");
        error.extensions = Some(extensions);
        Err(error)
    }
}
