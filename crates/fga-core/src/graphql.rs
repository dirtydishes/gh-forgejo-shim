use crate::forgejo;
use async_graphql::{EmptySubscription, ErrorExtensions, Schema};
mod checks;
mod connections;
mod git;
mod issues;
mod mutation_inputs;
mod mutations;
mod pulls;
mod query;
mod search;
mod threads;
mod types;
use checks::*;
use connections::*;
use git::*;
use issues::*;
use mutation_inputs::*;
pub use mutations::Mutation;
use pulls::*;
pub use query::{Query, Repository, SearchResultItem};
use threads::*;
pub use types::*;

pub type GithubSchema = Schema<Query, Mutation, EmptySubscription>;

/// The most fields one document may select, counting each alias and each use of a fragment. A
/// document that repeats a large field under many aliases builds a response many times the size
/// of what it reads from Forgejo. The largest real document the repo holds selects 221 (gh's `pr
/// list` with every `--json` field); `graphql::tests` checks every one stays under half of this.
pub const MAX_COMPLEXITY: usize = 500;

/// The deepest one document may nest. gh's documents and the standard introspection query reach
/// 13.
pub const MAX_DEPTH: usize = 20;

pub fn schema() -> GithubSchema {
    builder().finish()
}

fn builder() -> async_graphql::SchemaBuilder<Query, Mutation, EmptySubscription> {
    Schema::build(Query, Mutation, EmptySubscription)
        .data(crate::map::WipPrefixes::default())
        .limit_complexity(MAX_COMPLEXITY)
        .limit_depth(MAX_DEPTH)
}

#[cfg(test)]
mod tests;

pub fn error(error: forgejo::Error) -> async_graphql::Error {
    let kind = match error {
        forgejo::Error::Unauthorized => "UNAUTHORIZED",
        forgejo::Error::NotFound => "NOT_FOUND",
        forgejo::Error::Forbidden | forgejo::Error::MissingScope(_) => "FORBIDDEN",
        forgejo::Error::Timeout | forgejo::Error::Upstream | forgejo::Error::TooLarge => "INTERNAL",
    };
    async_graphql::Error::new(error.to_string())
        .extend_with(|_, extensions| extensions.set("type", kind))
}
