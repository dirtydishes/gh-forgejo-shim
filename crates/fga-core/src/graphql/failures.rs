//! Names the field each failure came from, for fga-server's usage counts (docs/telemetry.md). A
//! failure is an `INTERNAL` error ([`super::error`]): Forgejo timed out, failed, or sent an answer
//! the gateway can't read or hold. The name is the schema's own type and field, never the
//! document's alias, and goes in the error's extensions under [`FAILED_FIELD`]. fga-server reads it
//! and drops it, with every other extension, before it answers.
use std::sync::Arc;

use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextResolve, ResolveInfo,
};
use async_graphql::{ServerResult, Value};

/// The extension holding a failed field's `Type.field`.
pub const FAILED_FIELD: &str = "failed_field";

pub(super) struct NameFailures;

impl ExtensionFactory for NameFailures {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(NameFailures)
    }
}

#[async_graphql::async_trait::async_trait]
impl Extension for NameFailures {
    async fn resolve(
        &self,
        ctx: &ExtensionContext<'_>,
        info: ResolveInfo<'_>,
        next: NextResolve<'_>,
    ) -> ServerResult<Option<Value>> {
        let (parent, field) = (info.parent_type, info.name);
        next.run(ctx, info).await.map_err(|mut error| {
            // The resolver that failed is the first to see its error. A list's items have a
            // parent like `[Commit]`; the list's own field names those.
            let plain = parent
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_');
            if let Some(extensions) = error.extensions.as_mut() {
                let internal = extensions
                    .get("type")
                    .is_some_and(|kind| *kind == Value::String("INTERNAL".to_owned()));
                if plain && internal && extensions.get(FAILED_FIELD).is_none() {
                    extensions.set(FAILED_FIELD, format!("{parent}.{field}"));
                }
            }
            error
        })
    }
}
