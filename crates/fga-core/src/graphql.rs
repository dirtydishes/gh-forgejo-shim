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

pub fn schema() -> GithubSchema {
    Schema::build(Query, Mutation, EmptySubscription)
        .data(crate::map::WipPrefixes::default())
        .finish()
}

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
