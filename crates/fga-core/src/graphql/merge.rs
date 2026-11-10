//! GraphQL merges the fields a selection set selects under one response name, fragments included,
//! and runs each merged field once (the spec's CollectFields). async-graphql 7.2 runs every
//! selection of the field and merges only their results, so a mutation selected twice, as two
//! fragments on `Mutation` may each select it, would write twice and answer with fields of both
//! writes. Its overlapping-fields rule doesn't catch the same name with other arguments either
//! when one selection comes through a fragment and the other doesn't: both would write.
//!
//! So before validation, a mutation whose root repeats a response name is rewritten with each
//! name selected once, the repeats' sub-selections joined under the first, and fragments left
//! unused by that dropped. Repeats that can't be one field are refused, writing nothing: another
//! field or other arguments under the name, and repeats where `@skip` or `@include` could choose
//! between them, which async-graphql decides only after validation. A document that repeats no
//! name at a mutation's root, as every real client's, is left as parsed, and so is one that
//! repeats a name `Mutation` doesn't have, so validation reports the unknown field as it would.
//! Below the root, fields only read; conflicting sub-selections under one merged field (`x: id`
//! and `x: number`) stay unchecked, as async-graphql leaves them in queries.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_graphql::extensions::{Extension, ExtensionContext, ExtensionFactory, NextParseQuery};
use async_graphql::parser::Positioned;
use async_graphql::parser::types::{
    DocumentOperations, ExecutableDocument, Field, OperationType, Selection, SelectionSet,
};
use async_graphql::{ServerError, ServerResult, Variables};

pub(super) struct MergeMutationFields;

impl ExtensionFactory for MergeMutationFields {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(MergeMutationFields)
    }
}

#[async_graphql::async_trait::async_trait]
impl Extension for MergeMutationFields {
    async fn parse_query(
        &self,
        ctx: &ExtensionContext<'_>,
        query: &str,
        variables: &Variables,
        next: NextParseQuery<'_>,
    ) -> ServerResult<ExecutableDocument> {
        let mut document = next.run(ctx, query, variables).await?;
        let registry = &ctx.schema_env.registry;
        let mutation = registry
            .mutation_type
            .as_deref()
            .and_then(|name| registry.concrete_type_by_name(name));
        merge(&mut document, &|name| {
            mutation.is_some_and(|mutation| mutation.field_by_name(name).is_some())
        })?;
        Ok(document)
    }
}

/// Merges the root of each mutation in `document` by response name. `is_field` says whether
/// `Mutation` has a field of that name.
pub(super) fn merge(
    document: &mut ExecutableDocument,
    is_field: &dyn Fn(&str) -> bool,
) -> ServerResult<()> {
    let mut spread = HashSet::new();
    let mut merged = Vec::new();
    for (name, operation) in document.operations.iter() {
        if operation.node.ty != OperationType::Mutation {
            continue;
        }
        if let Some(set) = merged_root(
            document,
            &operation.node.selection_set,
            is_field,
            &mut spread,
        )? {
            merged.push((name.cloned(), set));
        }
    }
    if merged.is_empty() {
        return Ok(());
    }
    for (name, set) in merged {
        let operation = match (&mut document.operations, name) {
            (DocumentOperations::Single(operation), None) => operation,
            (DocumentOperations::Multiple(operations), Some(name)) => operations
                .get_mut(name.as_str())
                .expect("the operation was just read"),
            _ => unreachable!("the operation was just read"),
        };
        operation.node.selection_set = set;
    }
    // A fragment spread only where the merge inlined it is now unused, which validation refuses.
    // Fragments no operation spread before stay, and validation refuses them as it would have.
    let used = used_fragments(document);
    document
        .fragments
        .retain(|name, _| !spread.contains(name.as_str()) || used.contains(name.as_str()));
    Ok(())
}

/// A field at a mutation's root, and whether a fragment that carries a directive leads to it.
struct Collected<'a> {
    field: &'a Positioned<Field>,
    directed: bool,
}

/// The mutation root `set` with each response name selected once, or `None` when no name repeats
/// or the set holds what validation will refuse anyway, such as a repeated field `Mutation`
/// doesn't have (`is_field`). Records the fragments it spreads.
fn merged_root(
    document: &ExecutableDocument,
    set: &Positioned<SelectionSet>,
    is_field: &dyn Fn(&str) -> bool,
    spread: &mut HashSet<String>,
) -> ServerResult<Option<Positioned<SelectionSet>>> {
    let mut fields = Vec::new();
    let mut path = Vec::new();
    let mut names = HashSet::new();
    if !collect(document, set, false, &mut path, &mut fields, &mut names) {
        return Ok(None);
    }
    let mut groups: Vec<Vec<&Collected>> = Vec::new();
    let mut group_of: HashMap<&str, usize> = HashMap::new();
    for collected in &fields {
        let key = collected.field.node.response_key().node.as_str();
        match group_of.get(key) {
            Some(&group) => groups[group].push(collected),
            None => {
                group_of.insert(key, groups.len());
                groups.push(vec![collected]);
            }
        }
    }
    if groups.len() == fields.len() {
        return Ok(None);
    }
    let unknown = groups
        .iter()
        .filter(|group| group.len() > 1)
        .flatten()
        .any(|collected| !is_field(collected.field.node.name.node.as_str()));
    if unknown {
        return Ok(None);
    }
    // Inlining fragments drops their directives; a field selected twice keeps only the first's.
    let directed = fields.iter().find(|collected| collected.directed);
    let repeated_directive = groups
        .iter()
        .filter(|group| group.len() > 1)
        .flatten()
        .find(|collected| !collected.field.node.directives.is_empty())
        .copied();
    if let Some(collected) = directed.or(repeated_directive) {
        return Err(ServerError::new(
            "A mutation that selects a field more than once can't use directives on the fields or fragments at its root.",
            Some(collected.field.pos),
        ));
    }
    let mut items = Vec::with_capacity(groups.len());
    for group in groups {
        let first = group[0].field;
        let key = first.node.response_key().node.as_str();
        let mut field = first.clone();
        for collected in &group[1..] {
            if let Some(message) = super::cost::conflict(key, first, collected.field) {
                return Err(ServerError::new(message, Some(collected.field.pos)));
            }
            field.node.selection_set.node.items.extend(
                collected
                    .field
                    .node
                    .selection_set
                    .node
                    .items
                    .iter()
                    .cloned(),
            );
        }
        items.push(Positioned::new(Selection::Field(field), first.pos));
    }
    spread.extend(names);
    Ok(Some(Positioned::new(SelectionSet { items }, set.pos)))
}

/// Gathers the fields `set` selects at its own level, in order, through its fragments. False when
/// it reaches what validation refuses: an unknown fragment, one spread within itself, or a type
/// condition other than `Mutation`.
fn collect<'a>(
    document: &'a ExecutableDocument,
    set: &'a Positioned<SelectionSet>,
    directed: bool,
    path: &mut Vec<&'a str>,
    fields: &mut Vec<Collected<'a>>,
    names: &mut HashSet<String>,
) -> bool {
    for selection in &set.node.items {
        match &selection.node {
            Selection::Field(field) => fields.push(Collected { field, directed }),
            Selection::InlineFragment(inline) => {
                let on = inline.node.type_condition.as_ref();
                if on.is_some_and(|on| on.node.on.node != "Mutation")
                    || !collect(
                        document,
                        &inline.node.selection_set,
                        directed || !inline.node.directives.is_empty(),
                        path,
                        fields,
                        names,
                    )
                {
                    return false;
                }
            }
            Selection::FragmentSpread(spread) => {
                let name = spread.node.fragment_name.node.as_str();
                let Some(fragment) = document.fragments.get(name) else {
                    return false;
                };
                if path.contains(&name) || fragment.node.type_condition.node.on.node != "Mutation" {
                    return false;
                }
                names.insert(name.to_owned());
                path.push(name);
                let directed = directed
                    || !spread.node.directives.is_empty()
                    || !fragment.node.directives.is_empty();
                let collected = collect(
                    document,
                    &fragment.node.selection_set,
                    directed,
                    path,
                    fields,
                    names,
                );
                path.pop();
                if !collected {
                    return false;
                }
            }
        }
    }
    true
}

/// The fragments some operation of `document` spreads, at any depth.
fn used_fragments(document: &ExecutableDocument) -> HashSet<String> {
    let mut used = HashSet::new();
    let mut sets: Vec<&Positioned<SelectionSet>> = document
        .operations
        .iter()
        .map(|(_, operation)| &operation.node.selection_set)
        .collect();
    while let Some(set) = sets.pop() {
        for selection in &set.node.items {
            match &selection.node {
                Selection::Field(field) => sets.push(&field.node.selection_set),
                Selection::InlineFragment(inline) => sets.push(&inline.node.selection_set),
                Selection::FragmentSpread(spread) => {
                    let name = spread.node.fragment_name.node.as_str();
                    if let Some(fragment) = document.fragments.get(name)
                        && used.insert(name.to_owned())
                    {
                        sets.push(&fragment.node.selection_set);
                    }
                }
            }
        }
    }
    used
}
