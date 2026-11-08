//! A bound on the work one document makes async-graphql do before any limit of its own applies.
//! Its parser checks recursion depth by walking every fragment spread again, with no memory of
//! what it walked, so a fragment spread twice at each of 32 levels takes 2^32 steps. Its
//! validation keeps every error it finds, and its overlapping-fields rule compares each repeated
//! output name with the first, once per argument, across every fragment a selection reaches: a
//! document under 100 KB raised 8 million errors and held 4 GB. The complexity and depth limits
//! come after both.
//!
//! So each document is parsed and measured first, in steps that stop as soon as the count
//! passes [`MAX_COST`]: every node as written (field, argument, value, directive, fragment,
//! variable); every node again for each operation that reaches it through its fragments; each
//! step of the overlapping-fields rule, which this check runs itself and stops at its first
//! conflict; and, for each reference to a request variable after its first, the variable's size,
//! as validation copies and checks the whole value at every reference. A document that costs
//! more, spreads a fragment within itself, or has a conflict is refused with one error before
//! async-graphql parses it. None of these refusals can turn away a document async-graphql would
//! accept: every conflict or cycle found is one it refuses too, though its own error list may
//! name another first.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_graphql::extensions::{Extension, ExtensionContext, ExtensionFactory, NextParseQuery};
use async_graphql::parser::types::{Directive, ExecutableDocument, Field, Selection, SelectionSet};
use async_graphql::parser::{Positioned, parse_query};
use async_graphql::{ServerError, ServerResult, Variables};
use async_graphql_value::{ConstValue, Name, Value};

/// The most steps one document may cost. The costliest document the repo holds from a real
/// client, gh's `pr list` with every `--json` field, costs 885; `graphql::tests` checks every one
/// stays under a quarter of this. Validation's errors grow at most a few per step.
pub const MAX_COST: usize = 5_000;

pub(super) struct DocumentCost;

impl ExtensionFactory for DocumentCost {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(DocumentCost)
    }
}

#[async_graphql::async_trait::async_trait]
impl Extension for DocumentCost {
    async fn parse_query(
        &self,
        ctx: &ExtensionContext<'_>,
        query: &str,
        variables: &Variables,
        next: NextParseQuery<'_>,
    ) -> ServerResult<async_graphql::parser::types::ExecutableDocument> {
        // A document that doesn't parse gets async-graphql's own error.
        if let Ok(document) = parse_query(query)
            && let Err(refusal) = check(&document, variables)
        {
            return Err(ServerError::new(refusal.message(), None));
        }
        next.run(ctx, query, variables).await
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum Refusal {
    TooCostly,
    Cycle(String),
    Conflict(String),
}

impl Refusal {
    fn message(&self) -> String {
        match self {
            Self::TooCostly => "Query is too complex.".to_owned(),
            Self::Cycle(name) => format!("Cannot spread fragment \"{name}\" within itself"),
            Self::Conflict(message) => message.clone(),
        }
    }
}

/// Counts steps, failing once they pass [`MAX_COST`].
struct Meter<'a> {
    spent: usize,
    variables: &'a Variables,
    /// The request variables referenced so far as written.
    referenced: HashSet<&'a str>,
}

impl<'a> Meter<'a> {
    fn charge(&mut self, steps: usize) -> Result<(), Refusal> {
        self.spent = self.spent.saturating_add(steps);
        if self.spent > MAX_COST {
            Err(Refusal::TooCostly)
        } else {
            Ok(())
        }
    }

    /// A reference to variable `name` as written. Validation copies and checks the request's
    /// value at each one, so each after the first costs the value's size; one reference, as real
    /// clients make, costs no more than any other node.
    fn reference(&mut self, name: &'a str) -> Result<(), Refusal> {
        if self.referenced.insert(name) {
            return Ok(());
        }
        let size = self.variables.get(name).map_or(0, const_value_size);
        self.charge(size)
    }
}

/// What `document` costs with the request's `variables`, or why it is refused.
pub(super) fn check(
    document: &ExecutableDocument,
    variables: &Variables,
) -> Result<usize, Refusal> {
    let mut meter = Meter {
        spent: 0,
        variables,
        referenced: HashSet::new(),
    };
    let operations: Vec<_> = document.operations.iter().map(|(_, op)| op).collect();
    // As written: every node once, and the overlapping-fields rule for every selection set.
    for operation in &operations {
        for variable in &operation.node.variable_definitions {
            meter.charge(1)?;
            directives(&mut meter, &variable.node.directives, Walk::Written)?;
            if let Some(default) = &variable.node.default_value {
                meter.charge(const_value_nodes(&default.node))?;
            }
        }
        directives(&mut meter, &operation.node.directives, Walk::Written)?;
        written(&mut meter, document, &operation.node.selection_set)?;
    }
    for fragment in document.fragments.values() {
        meter.charge(1)?;
        directives(&mut meter, &fragment.node.directives, Walk::Written)?;
        written(&mut meter, document, &fragment.node.selection_set)?;
    }
    // Each operation with its fragments expanded, as the recursion-depth check, the per-operation
    // variable rules and the complexity and depth limits walk it.
    for operation in &operations {
        expanded(&mut meter, document, &operation.node.selection_set)?;
    }
    Ok(meter.spent)
}

/// Which walk charges a node: references to request variables cost their size only as written,
/// where validation checks them.
#[derive(Clone, Copy, PartialEq)]
enum Walk {
    Written,
    Expanded,
}

/// Every selection set below `root` as written, each node charged once, with the
/// overlapping-fields rule run on each set validation enters. It never enters one below
/// `__typename`, which it reads as a leaf whatever follows.
fn written<'a>(
    meter: &mut Meter<'a>,
    document: &'a ExecutableDocument,
    root: &'a Positioned<SelectionSet>,
) -> Result<(), Refusal> {
    let mut sets = vec![(root, true)];
    while let Some((set, entered)) = sets.pop() {
        if entered {
            overlapping_fields(meter, document, set)?;
        }
        for selection in &set.node.items {
            meter.charge(1)?;
            match &selection.node {
                Selection::Field(field) => {
                    arguments(meter, &field.node.arguments, Walk::Written)?;
                    directives(meter, &field.node.directives, Walk::Written)?;
                    let leaf = field.node.name.node == "__typename";
                    sets.push((&field.node.selection_set, entered && !leaf));
                }
                Selection::FragmentSpread(spread) => {
                    directives(meter, &spread.node.directives, Walk::Written)?
                }
                Selection::InlineFragment(inline) => {
                    directives(meter, &inline.node.directives, Walk::Written)?;
                    sets.push((&inline.node.selection_set, entered));
                }
            }
        }
    }
    Ok(())
}

/// Every node below `root` with fragment spreads expanded where they appear. A spread of a
/// fragment already on the path is a cycle, which async-graphql refuses too.
fn expanded<'a>(
    meter: &mut Meter<'a>,
    document: &'a ExecutableDocument,
    root: &'a Positioned<SelectionSet>,
) -> Result<(), Refusal> {
    // Each entry is a set to walk and the fragments on the path to it.
    let mut sets: Vec<(&Positioned<SelectionSet>, Arc<Vec<&str>>)> =
        vec![(root, Arc::new(Vec::new()))];
    while let Some((set, path)) = sets.pop() {
        for selection in &set.node.items {
            meter.charge(1)?;
            match &selection.node {
                Selection::Field(field) => {
                    arguments(meter, &field.node.arguments, Walk::Expanded)?;
                    directives(meter, &field.node.directives, Walk::Expanded)?;
                    sets.push((&field.node.selection_set, Arc::clone(&path)));
                }
                Selection::InlineFragment(inline) => {
                    directives(meter, &inline.node.directives, Walk::Expanded)?;
                    sets.push((&inline.node.selection_set, Arc::clone(&path)));
                }
                Selection::FragmentSpread(spread) => {
                    directives(meter, &spread.node.directives, Walk::Expanded)?;
                    let name = spread.node.fragment_name.node.as_str();
                    let Some(fragment) = document.fragments.get(name) else {
                        continue;
                    };
                    if path.contains(&name) {
                        return Err(Refusal::Cycle(name.to_owned()));
                    }
                    meter.charge(path.len())?;
                    let mut inner = Vec::clone(&path);
                    inner.push(name);
                    sets.push((&fragment.node.selection_set, Arc::new(inner)));
                }
            }
        }
    }
    Ok(())
}

/// async-graphql's `OverlappingFieldsCanBeMerged` on one selection set, in the same order, each
/// step charged, failing at the first conflict it would report.
fn overlapping_fields(
    meter: &mut Meter<'_>,
    document: &ExecutableDocument,
    root: &Positioned<SelectionSet>,
) -> Result<(), Refusal> {
    let mut outputs: HashMap<(Option<&str>, &str), &Positioned<Field>> = HashMap::new();
    let mut visited: HashSet<&str> = HashSet::new();
    let mut stack = vec![(None, root.node.items.iter())];
    while let Some((on_type, items)) = stack.last_mut() {
        let on_type = *on_type;
        let Some(selection) = items.next() else {
            stack.pop();
            continue;
        };
        meter.charge(1)?;
        match &selection.node {
            Selection::Field(field) => {
                let name = field.node.response_key().node.as_str();
                let Some(previous) = outputs.get(&(on_type, name)) else {
                    outputs.insert((on_type, name), field);
                    continue;
                };
                // Each argument is looked up by a scan of the other field's.
                meter.charge(
                    previous.node.arguments.len().max(1) * field.node.arguments.len().max(1),
                )?;
                if let Some(message) = conflict(name, previous, field) {
                    return Err(Refusal::Conflict(message));
                }
            }
            Selection::InlineFragment(inline) => {
                let on_type = inline
                    .node
                    .type_condition
                    .as_ref()
                    .map(|condition| condition.node.on.node.as_str());
                stack.push((on_type, inline.node.selection_set.node.items.iter()));
            }
            Selection::FragmentSpread(spread) => {
                let name = spread.node.fragment_name.node.as_str();
                if let Some(fragment) = document.fragments.get(name)
                    && visited.insert(name)
                {
                    let on_type = Some(fragment.node.type_condition.node.on.node.as_str());
                    stack.push((on_type, fragment.node.selection_set.node.items.iter()));
                }
            }
        }
    }
    Ok(())
}

/// The first error async-graphql's rule reports for `field` under `name`, after `previous`. For
/// one differing argument it names the argument, not the field, as the rule does.
fn conflict(name: &str, previous: &Positioned<Field>, field: &Positioned<Field>) -> Option<String> {
    let differing = |name: &str| {
        format!(
            "Fields \"{name}\" conflict because they have differing arguments. Use different aliases on the fields to fetch both if this was intentional."
        )
    };
    if previous.node.name.node != field.node.name.node {
        return Some(format!(
            "Fields \"{name}\" conflict because \"{}\" and \"{}\" are different fields. Use different aliases on the fields to fetch both if this was intentional.",
            previous.node.name.node, field.node.name.node
        ));
    }
    if previous.node.arguments.len() != field.node.arguments.len() {
        return Some(differing(name));
    }
    previous
        .node
        .arguments
        .iter()
        .find(|(argument, value)| field.node.get_argument(&argument.node) != Some(value))
        .map(|(argument, _)| differing(&argument.node))
}

fn arguments<'a>(
    meter: &mut Meter<'a>,
    arguments: &'a [(Positioned<Name>, Positioned<Value>)],
    walk: Walk,
) -> Result<(), Refusal> {
    for (_, value) in arguments {
        meter.charge(1)?;
        // Every value it holds, itself included, walked without recursion.
        let mut values = vec![&value.node];
        while let Some(value) = values.pop() {
            meter.charge(1)?;
            match value {
                Value::List(items) => values.extend(items),
                Value::Object(entries) => values.extend(entries.values()),
                Value::Variable(name) if walk == Walk::Written => meter.reference(name)?,
                _ => {}
            }
        }
    }
    Ok(())
}

fn directives<'a>(
    meter: &mut Meter<'a>,
    directives: &'a [Positioned<Directive>],
    walk: Walk,
) -> Result<(), Refusal> {
    for directive in directives {
        meter.charge(1)?;
        arguments(meter, &directive.node.arguments, walk)?;
    }
    Ok(())
}

/// A request variable's size to copy and check: its values, and its text by the 64 bytes.
fn const_value_size(value: &ConstValue) -> usize {
    let mut size = 0;
    let mut values = vec![value];
    while let Some(value) = values.pop() {
        size += 1;
        match value {
            ConstValue::List(items) => values.extend(items),
            ConstValue::Object(entries) => {
                size += entries.keys().map(|key| key.len() / 64).sum::<usize>();
                values.extend(entries.values());
            }
            ConstValue::String(text) => size += text.len() / 64,
            ConstValue::Binary(bytes) => size += bytes.len() / 64,
            _ => {}
        }
    }
    size
}

/// How many values a variable's default holds, itself included.
fn const_value_nodes(value: &ConstValue) -> usize {
    let mut count = 0;
    let mut values = vec![value];
    while let Some(value) = values.pop() {
        count += 1;
        match value {
            ConstValue::List(items) => values.extend(items),
            ConstValue::Object(entries) => values.extend(entries.values()),
            _ => {}
        }
    }
    count
}
