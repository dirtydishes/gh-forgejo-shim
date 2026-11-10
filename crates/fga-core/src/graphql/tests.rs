//! The query limits accept every document the repo holds from a real client, and refuse a
//! document that repeats a field under many aliases.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextExecute, NextValidation,
};
use async_graphql::{Response, ServerError, ValidationResult};

use super::*;

type Validation = Result<ValidationResult, Vec<ServerError>>;

/// Records what validation made of a document, and runs none of it: the resolvers need a
/// Forgejo session, and the limits apply before any resolver runs.
#[derive(Clone, Default)]
struct ValidateOnly(Arc<Mutex<Option<Validation>>>);

impl ExtensionFactory for ValidateOnly {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(self.clone())
    }
}

#[async_graphql::async_trait::async_trait]
impl Extension for ValidateOnly {
    async fn validation(
        &self,
        ctx: &ExtensionContext<'_>,
        next: NextValidation<'_>,
    ) -> Result<ValidationResult, Vec<ServerError>> {
        let result = next.run(ctx).await;
        *self.0.lock().expect("validation result") = Some(result.clone());
        result
    }

    async fn execute(
        &self,
        _ctx: &ExtensionContext<'_>,
        _operation_name: Option<&str>,
        _next: NextExecute<'_>,
    ) -> Response {
        Response::new(async_graphql::Value::Null)
    }
}

/// What a schema built by `builder` makes of `query`.
async fn validate(
    builder: async_graphql::SchemaBuilder<Query, Mutation, EmptySubscription>,
    query: &str,
) -> Validation {
    let seen = ValidateOnly::default();
    let response = builder
        .extension(seen.clone())
        .finish()
        .execute(query)
        .await;
    let validation = seen.0.lock().expect("validation result").take();
    validation.unwrap_or_else(|| Err(response.errors))
}

fn refused_by_a_limit(errors: &[ServerError]) -> bool {
    errors.iter().any(|error| {
        matches!(
            error.message.as_str(),
            "Query is too complex." | "Query is nested too deep."
        )
    })
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every GraphQL document the repo holds from a real client, with where it came from: gh's
/// captured documents, the oracle's recorded gh requests (reads and writes), a contract case's, and
/// ChatGPT's.
fn documents() -> Vec<(String, String)> {
    let mut documents = Vec::new();
    let read = |path: &Path| std::fs::read_to_string(path).expect("fixture");
    for dir in [
        "fixtures",
        "fixtures/gh-pr-read",
        "fixtures/gh-every-field",
        "crates/fga-core/tests/fixtures/chatgpt",
    ] {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(root().join(dir))
            .expect("fixture directory")
            .map(|entry| entry.expect("fixture entry").path())
            // github-g1.graphql is GitHub's schema, not a document.
            .filter(|path| {
                path.extension().is_some_and(|ext| ext == "graphql")
                    && !path.ends_with("github-g1.graphql")
            })
            .collect();
        paths.sort();
        for path in paths {
            documents.push((format!("{dir}/{}", name(&path)), read(&path)));
        }
    }
    for (dir, pattern) in [
        ("fixtures/oracle", ".requests.json"),
        ("fixtures/oracle/writes", ".requests.json"),
        ("contract/cases", ".json"),
    ] {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(root().join(dir))
            .expect("fixture directory")
            .map(|entry| entry.expect("fixture entry").path())
            .filter(|path| name(path).ends_with(pattern))
            .collect();
        paths.sort();
        for path in paths {
            let value: serde_json::Value =
                serde_json::from_str(&read(&path)).expect("JSON fixture");
            // The oracle records `[{"query": ..., "variables": ...}]`; a contract case passes its
            // document to `gh api graphql` as `-f query=...`.
            let oracle = value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|request| request["query"].as_str());
            let argv = value["argv"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|arg| arg.as_str()?.strip_prefix("query="));
            for (index, query) in oracle.chain(argv).enumerate() {
                documents.push((format!("{dir}/{}#{index}", name(&path)), query.to_string()));
            }
        }
    }
    documents
}

fn name(path: &Path) -> String {
    path.file_name()
        .expect("file name")
        .to_string_lossy()
        .into_owned()
}

#[tokio::test]
async fn every_captured_document_is_within_the_limits() {
    let documents = documents();
    assert!(documents.len() > 140, "found {} documents", documents.len());
    let mut invalid = Vec::new();
    for (source, query) in documents {
        let document = async_graphql::parser::parse_query(&query).expect("parses");
        let steps =
            cost::check(&document, &async_graphql::Variables::default()).expect("within cost");
        let shape = cost::shape(&query).expect("block strings closed");
        let mut merged = document.clone();
        merge::merge(&mut merged, &|_| true).expect("merges");
        assert_eq!(
            shown(&merged),
            shown(&document),
            "{source}: repeats a name at a mutation's root"
        );
        assert!(
            shape.depth * 4 <= cost::MAX_NESTING,
            "{source}: nests {}: raise MAX_NESTING",
            shape.depth
        );
        assert!(
            shape.longest_word * 4 <= cost::MAX_WORD,
            "{source}: has a {}-character name: raise MAX_WORD",
            shape.longest_word
        );
        assert!(
            steps * 4 <= cost::MAX_COST,
            "{source}: costs {steps}: raise MAX_COST"
        );
        match validate(builder(), &query).await {
            // Room to spare, so a client's next version or a new field doesn't hit the limits.
            Ok(result) => assert!(
                result.complexity * 2 <= MAX_COMPLEXITY && result.depth + 5 <= MAX_DEPTH,
                "{source}: complexity {}, depth {}: raise the limits",
                result.complexity,
                result.depth
            ),
            Err(errors) => {
                assert!(!refused_by_a_limit(&errors), "{source}: {errors:?}");
                invalid.push(source);
            }
        }
    }
    // Documents that name fields the gateway doesn't serve fail validation before any limit:
    // gh's `repo view` with every field (47 `Repository` fields, from `archivedAt` on), and
    // `pr create`/`pr edit`'s read-back (`PullRequest.assignedActors` and `Bot.id`). Any other
    // document that stops validating fails here.
    assert_eq!(
        invalid,
        [
            "fixtures/gh-every-field/repo-view-all.graphql",
            "fixtures/oracle/writes/base.requests.json#0",
            "fixtures/oracle/writes/edit-draft.requests.json#0",
            "fixtures/oracle/writes/edit.requests.json#0",
        ]
    );
}

/// The report's document: one issue body under 1,000 aliases. Each field costs one, so
/// `repository` and `issue` add two to the aliases.
#[tokio::test]
async fn a_field_repeated_under_many_aliases_is_refused() {
    let aliases = |count: usize| {
        let fields: Vec<String> = (0..count).map(|i| format!("a{i}:body")).collect();
        format!(
            "{{repository(owner:\"o\",name:\"r\"){{issue(number:1){{{}}}}}}}",
            fields.join(" ")
        )
    };

    let errors = validate(builder(), &aliases(1000))
        .await
        .expect_err("1,000 aliases");
    assert_eq!(errors[0].message, "Query is too complex.");
    assert!(
        validate(builder(), &aliases(MAX_COMPLEXITY - 2))
            .await
            .is_ok()
    );
    let errors = validate(builder(), &aliases(MAX_COMPLEXITY - 1))
        .await
        .expect_err("one past the limit");
    assert_eq!(errors[0].message, "Query is too complex.");
}

#[tokio::test]
async fn a_document_nested_past_the_limit_is_refused() {
    // `__type` is depth 1, `kind` under n `ofType`s is depth n + 2.
    let nested = |levels: usize| {
        format!(
            "{{__type(name:\"Query\"){{{}kind{}}}}}",
            "ofType{".repeat(levels),
            "}".repeat(levels)
        )
    };

    assert!(validate(builder(), &nested(MAX_DEPTH - 2)).await.is_ok());
    let errors = validate(builder(), &nested(MAX_DEPTH - 1))
        .await
        .expect_err("one past the limit");
    assert_eq!(errors[0].message, "Query is nested too deep.");
}

/// Introspection resolves without Forgejo, so it shows the budget at work: past it, the response
/// has one error and stops growing; under it, the response is the same as without one.
#[tokio::test]
async fn a_response_stops_growing_at_its_output_budget() {
    let query = "{__schema{types{name description fields{name description}}}}";
    let full = builder().finish().execute(query).await;
    assert!(full.errors.is_empty(), "{:?}", full.errors);
    let size = serde_json::to_vec(&full).expect("response").len();

    let limited = builder()
        .extension(budget::OutputBudget(size / 4))
        .finish()
        .execute(query)
        .await;
    assert_eq!(limited.errors.len(), 1, "{:?}", limited.errors);
    let error = serde_json::to_value(&limited.errors[0]).expect("error");
    assert_eq!(error["extensions"]["type"], "RESOURCE_LIMITS_EXCEEDED");
    assert!(serde_json::to_vec(&limited).expect("response").len() < size / 2);

    // An entry costs more than its JSON (`PER_ENTRY`), so leave room.
    let roomy = builder()
        .extension(budget::OutputBudget(size * 8))
        .finish()
        .execute(query)
        .await;
    assert!(roomy.errors.is_empty(), "{:?}", roomy.errors);
    assert_eq!(roomy.data, full.data);
}

/// What `query` builds under an output budget of `budget` bytes: its errors and its size as JSON.
async fn under_budget(budget: usize, query: &str) -> (Vec<ServerError>, usize) {
    let response = builder()
        .extension(budget::OutputBudget(budget))
        .finish()
        .execute(query)
        .await;
    let size = serde_json::to_vec(&response).expect("response").len();
    (response.errors, size)
}

/// `count` fields of `field` under long aliases, in every type the schema has.
fn long_aliases(field: &str, count: usize) -> String {
    let prefix = "a".repeat(3000);
    let fields: Vec<String> = (0..count).map(|i| format!("{prefix}{i}:{field}")).collect();
    format!("{{__schema{{types{{{}}}}}}}", fields.join(" "))
}

/// The response names each alias once per type, so long aliases weigh far more than the
/// document; the budget counts them.
#[tokio::test]
async fn long_aliases_count_against_the_output_budget() {
    let budget = 1024 * 1024;
    let (errors, size) = under_budget(budget, &long_aliases("name", 497)).await;

    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(size <= budget + 4096, "{size} bytes");
}

/// `__typename` resolves without reaching the extension; its entry is charged where its object
/// is.
#[tokio::test]
async fn typename_entries_count_against_the_output_budget() {
    let budget = 64 * 1024;
    let fields: Vec<String> = (0..497).map(|i| format!("t{i}:__typename")).collect();
    let query = format!("{{__schema{{types{{{}}}}}}}", fields.join(" "));

    let (errors, size) = under_budget(budget, &query).await;

    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(size <= budget + 4096, "{size} bytes");
}

/// An entry's charge covers a `__typename` value, so every type name must fit in it.
#[tokio::test]
async fn every_type_name_fits_in_an_entry() {
    let response = builder().finish().execute("{__schema{types{name}}}").await;
    let data = response.data.into_json().expect("data");
    let names = data["__schema"]["types"].as_array().expect("types");
    assert!(names.len() > 50);
    for name in names {
        let name = name["name"].as_str().expect("name");
        assert!(name.len() + 16 <= budget::PER_ENTRY, "{name}");
    }
}

#[test]
fn json_len_counts_escapes() {
    assert_eq!(budget::json_len("plain é"), "plain é".len());
    assert_eq!(budget::json_len("\"\\\n"), 6);
    assert_eq!(budget::json_len("\u{1}"), 6);
    assert_eq!(
        budget::json_len("\u{1}\"\n"),
        serde_json::to_string("\u{1}\"\n").expect("JSON").len() - 2
    );
}

/// The budget's own error must pass up through a list that can't be null, even when too little
/// budget is left to charge it there. Introspection's `types` is `[__Type!]!`: each type below
/// costs exactly `per_type`, the budget holds five and 40 bytes more, so the sixth runs it out,
/// and its error, worth more than 40 bytes, lands on fields with nothing left to charge.
#[tokio::test]
async fn the_budget_error_survives_a_list_that_cannot_be_null() {
    let alias = "a".repeat(100);
    let fields: Vec<String> = (0..10)
        .map(|i| format!("{alias}{i:02}:__typename"))
        .collect();
    let per_type = 10 * (alias.len() + 2 + budget::PER_ENTRY);
    let query = format!("{{__schema{{types{{{}}}}}}}", fields.join(" "));

    let (errors, size) = under_budget(5 * per_type + 40, &query).await;

    assert_eq!(errors.len(), 1, "{errors:?}");
    let error = serde_json::to_value(&errors[0]).expect("error");
    assert_eq!(error["extensions"]["type"], "RESOURCE_LIMITS_EXCEEDED");
    assert!(size < 1024, "{size} bytes");
}

/// The one error a refused document gets, after asserting there is only one.
async fn refused(query: &str) -> String {
    let response = builder().finish().execute(query).await;
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    response.errors[0].message.clone()
}

/// async-graphql's overlapping-fields rule reports one error per argument for each repeat of an
/// output name, before the complexity limit applies: here 40 repeats of 200 arguments would be
/// about 8,000 errors, and the same shape at 512 KiB billions. The first conflict refuses it.
#[tokio::test]
async fn a_repeated_field_with_differing_arguments_is_refused_once() {
    let arguments = vec!["b:1"; 200].join(" ");
    let query = format!("{{viewer({arguments}){{login}} {}}}", "viewer ".repeat(40));
    assert!(
        refused(&query)
            .await
            .contains("conflict because they have differing arguments")
    );

    let small = refused("{viewer(b:1){login} viewer{login}}").await;
    assert!(small.contains("differing arguments"), "{small}");
}

/// The same rule walks every fragment a selection set reaches, once per selection set: 200
/// selections of a fragment that spreads 200 conflicting ones would be 40,000 errors.
#[tokio::test]
async fn conflicts_reached_through_fragments_are_refused_once() {
    let selections: Vec<String> = (0..200).map(|i| format!("v{i}:viewer{{...G}}")).collect();
    let spreads: Vec<String> = (0..200).map(|i| format!("...f{i}")).collect();
    let fragments: Vec<String> = (0..200)
        .map(|i| format!("fragment f{i} on User{{a:login a:name}}"))
        .collect();
    let query = format!(
        "{{{}}} fragment G on User{{{}}} {}",
        selections.join(" "),
        spreads.join(" "),
        fragments.join(" ")
    );

    assert!(refused(&query).await.contains("are different fields"));
}

/// async-graphql's recursion-depth check walks a spread again each time it appears, so a
/// fragment spread twice at each of 30 levels would take a billion steps.
#[tokio::test]
async fn fragments_spread_twice_at_each_level_are_refused() {
    let mut query = String::from("{...F0}");
    for level in 0..30 {
        let next = level + 1;
        query.push_str(&format!(
            " fragment F{level} on Query{{...F{next} ...F{next}}}"
        ));
    }
    query.push_str(" fragment F30 on Query{__typename}");

    assert_eq!(refused(&query).await, "Query is too complex.");
}

#[tokio::test]
async fn a_fragment_spread_within_itself_is_refused() {
    let message = refused("{...F} fragment F on Query{...F ...F}").await;
    assert_eq!(message, "Cannot spread fragment \"F\" within itself");
}

#[tokio::test]
async fn a_document_past_its_cost_is_refused() {
    let arguments = vec!["b:1"; cost::MAX_COST].join(" ");
    let query = format!("{{viewer({arguments}){{login}}}}");
    assert_eq!(refused(&query).await, "Query is too complex.");
}

/// Validation copies and checks a request variable's whole value at each reference, so a second
/// reference to a large one costs its size. One reference, as clients send, costs no more.
#[tokio::test]
async fn a_large_variable_referenced_twice_is_refused() {
    let variables =
        async_graphql::Variables::from_json(serde_json::json!({ "v": vec![0; 20_000] }));
    let once = r#"query($v:String!){repository(owner:$v,name:"x"){id}}"#;
    let twice = r#"query($v:String!){a:repository(owner:$v,name:"x"){id} b:repository(owner:$v,name:"y"){id}}"#;

    let response = builder()
        .finish()
        .execute(async_graphql::Request::new(once).variables(variables.clone()))
        .await;
    assert!(
        response
            .errors
            .iter()
            .all(|error| error.message != "Query is too complex."),
        "{:?}",
        response.errors
    );

    let response = builder()
        .finish()
        .execute(async_graphql::Request::new(twice).variables(variables))
        .await;
    assert_eq!(response.errors.len(), 1, "{:?}", response.errors);
    assert_eq!(response.errors[0].message, "Query is too complex.");
}

/// Validation never enters a selection under `__typename`, so a conflict there refuses nothing.
#[tokio::test]
async fn a_selection_under_typename_is_not_checked_for_conflicts() {
    let response = builder()
        .finish()
        .execute("{__typename{a:login a:name}}")
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().expect("data"),
        serde_json::json!({ "__typename": "Query" })
    );
}

/// The refusal names a differing argument as async-graphql's rule does.
#[tokio::test]
async fn a_differing_argument_is_named_as_the_rule_names_it() {
    let message = refused(r#"{viewer(login:"a"){id} viewer(login:"b"){id}}"#).await;
    assert!(
        message.starts_with("Fields \"login\" conflict because they have differing arguments"),
        "{message}"
    );
}

fn depth(query: &str) -> Option<usize> {
    cost::shape(query).map(|shape| shape.depth)
}

#[test]
fn nesting_skips_strings_and_comments() {
    assert_eq!(depth("{a(b:[1,{c:2}])}"), Some(4));
    assert_eq!(depth(r#"{a(b:"[[[[\"[[")}"#), Some(2));
    assert_eq!(depth("{a(b:\"\"\"[[[\\\"\"\"[[[\"\"\")}"), Some(2));
    assert_eq!(depth("{a # [[[[\n}"), Some(1));
    assert_eq!(depth("{a(b:\"x\ny\")}"), Some(2));
    assert_eq!(depth("{a(b:\"\"\"x\"\"\"[[1]])}"), Some(4));
    // The parser would read an unclosed block string as `""` and then an ordinary string.
    assert_eq!(depth(r#"{a(b:["""x"[[[[1]]]]])}"#), None);
    assert_eq!(depth(r#"{a(b:"x""""")}"#), None);
}

#[test]
fn words_are_measured_outside_strings_and_comments() {
    let word = |query: &str| cost::shape(query).map(|shape| shape.longest_word);
    assert_eq!(word("{viewer{login}}"), Some(6));
    assert_eq!(
        word(
            r#"{a(b:"aaaaaaaa") # bbbbbbbb
}"#
        ),
        Some(1)
    );
    assert_eq!(word("{a(b:12345678)}"), Some(8));
}

/// async-graphql copies an operation's name into the error for each variable it leaves unused,
/// so a long name times many variables would build gigabytes of error text.
#[tokio::test]
async fn a_long_name_is_refused_before_parsing() {
    let name = "A".repeat(cost::MAX_WORD + 1);
    let variables: String = (0..100).map(|n| format!("$v{n}:Int ")).collect();
    let query = format!("query {name}({variables}){{__typename}}");
    assert_eq!(
        refused(&query).await,
        "Query has a name or number that is too long."
    );
    // At the limit it reaches validation, which reports each unused variable.
    let query = format!("query {}({variables}){{__typename}}", &name[1..]);
    let response = builder().finish().execute(query).await;
    assert_eq!(response.errors.len(), 100, "{:?}", response.errors[0]);
}

/// Reading on past an unclosed block string takes quadratic time in the scan and the parser.
#[tokio::test]
async fn an_unclosed_block_string_is_refused_before_parsing() {
    let query = format!("{{viewer(a:{}){{login}}}}", "\"\"\"\"\"\\\\".repeat(10_000));
    assert_eq!(refused(&query).await, "Query has an unclosed block string.");
}

/// The parser recurses once per level of a value with no limit of its own, so deep nesting is
/// refused before it parses. Each of these stays shallow enough to parse safely on a test
/// thread even without the refusal.
#[tokio::test]
async fn deep_nesting_is_refused_before_parsing() {
    for (open, close) in [("[", "]"), ("{c:", "}")] {
        let levels = cost::MAX_NESTING;
        let query = format!(
            "{{viewer(a:{}1{}){{login}}}}",
            open.repeat(levels),
            close.repeat(levels)
        );
        assert_eq!(refused(&query).await, "Query is nested too deep.");
    }
    let types = format!(
        "query($v:{}Int{}){{viewer{{login}}}}",
        "[".repeat(cost::MAX_NESTING + 1),
        "]".repeat(cost::MAX_NESTING + 1)
    );
    assert_eq!(refused(&types).await, "Query is nested too deep.");
}

/// Every operation and fragment of `document`, in an order that doesn't depend on its maps'.
fn shown(document: &async_graphql::parser::types::ExecutableDocument) -> Vec<String> {
    let mut shown: Vec<String> = document
        .operations
        .iter()
        .map(|operation| format!("{operation:?}"))
        .chain(
            document
                .fragments
                .iter()
                .map(|fragment| format!("{fragment:?}")),
        )
        .collect();
    shown.sort();
    shown
}

/// `document` with its mutations' roots merged, as validation receives it.
fn merged(
    document: &str,
) -> async_graphql::ServerResult<async_graphql::parser::types::ExecutableDocument> {
    let mut document = async_graphql::parser::parse_query(document).expect("parses");
    merge::merge(&mut document, &|_| true)?;
    Ok(document)
}

/// The response names `document`'s one operation selects at its root.
fn root_names(document: &async_graphql::parser::types::ExecutableDocument) -> Vec<String> {
    use async_graphql::parser::types::Selection;
    let (_, operation) = document.operations.iter().next().expect("an operation");
    operation
        .node
        .selection_set
        .node
        .items
        .iter()
        .map(|selection| match &selection.node {
            Selection::Field(field) => field.node.response_key().node.to_string(),
            other => panic!("{other:?} left at the root"),
        })
        .collect()
}

/// Every mutation the schema serves, selected twice through fragments and once more directly,
/// is selected once, with the repeats' sub-selections under it in order.
#[test]
fn every_mutation_selected_more_than_once_is_selected_once() {
    use async_graphql::parser::types::{Selection, TypeKind, TypeSystemDefinition};
    let sdl = async_graphql::parser::parse_schema(builder().finish().sdl()).expect("SDL");
    let mutations: Vec<String> = sdl
        .definitions
        .into_iter()
        .find_map(|definition| match definition {
            TypeSystemDefinition::Type(ty) if ty.node.name.node == "Mutation" => match ty.node.kind
            {
                TypeKind::Object(object) => Some(
                    object
                        .fields
                        .into_iter()
                        .map(|field| field.node.name.node.to_string())
                        .collect(),
                ),
                _ => None,
            },
            _ => None,
        })
        .expect("a Mutation type");
    assert!(mutations.len() > 10, "{mutations:?}");
    for mutation in mutations {
        let document = merged(&format!(
            "mutation{{...A ... on Mutation{{...B}} {mutation}(input:$i){{c}} __typename}}
            fragment A on Mutation{{{mutation}(input:$i){{a}}}}
            fragment B on Mutation{{{mutation}(input:$i){{b}}}}"
        ))
        .expect("merges");
        assert_eq!(root_names(&document), [mutation.as_str(), "__typename"]);
        let (_, operation) = document.operations.iter().next().expect("an operation");
        let Selection::Field(field) = &operation.node.selection_set.node.items[0].node else {
            unreachable!()
        };
        let selected: Vec<String> = field
            .node
            .selection_set
            .node
            .items
            .iter()
            .map(|selection| match &selection.node {
                Selection::Field(field) => field.node.name.node.to_string(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(selected, ["a", "b", "c"], "{mutation}");
        // Both fragments were spread only at the root, so they go with it.
        assert!(document.fragments.is_empty(), "{mutation}");
    }
}

/// Only a mutation's root is merged: a query, and a mutation that repeats no name, stay as parsed.
#[test]
fn documents_that_repeat_no_mutation_name_stay_as_parsed() {
    for query in [
        "{viewer{login} viewer{name}}",
        "mutation{a:closePullRequest(input:$i){b} c:closePullRequest(input:$i){d}}",
        "mutation{...A} fragment A on Mutation{closePullRequest(input:$i){b}} fragment U on Mutation{x}",
    ] {
        let parsed = async_graphql::parser::parse_query(query).expect("parses");
        assert_eq!(
            shown(&merged(query).expect("merges")),
            shown(&parsed),
            "{query}"
        );
    }
}

/// A fragment the merge inlines stays where another operation, or a field's selection, spreads
/// it; a fragment nothing spread stays for validation to refuse.
#[test]
fn fragments_still_spread_elsewhere_stay() {
    let document = merged(
        "mutation M{...A ...A} query Q{...R}
        fragment A on Mutation{closePullRequest(input:$i){...P}}
        fragment P on ClosePullRequestPayload{clientMutationId}
        fragment R on Query{viewer{login}}
        fragment U on Query{viewer{name}}",
    )
    .expect("merges");
    let mut names: Vec<&str> = document
        .fragments
        .keys()
        .map(|name| name.as_str())
        .collect();
    names.sort();
    assert_eq!(names, ["P", "R", "U"]);
}

/// The merge chooses between repeats only where `@skip` and `@include` can't, as async-graphql
/// applies them after validation.
#[tokio::test]
async fn a_repeated_mutation_under_a_directive_is_refused() {
    for query in [
        "mutation($x:Boolean!){...A @include(if:$x) closePullRequest(input:{pullRequestId:\"P\"}){clientMutationId}}
        fragment A on Mutation{closePullRequest(input:{pullRequestId:\"P\"}){clientMutationId}}",
        "mutation($x:Boolean!){closePullRequest(input:{pullRequestId:\"P\"}) @skip(if:$x){clientMutationId}
        closePullRequest(input:{pullRequestId:\"P\"}){clientMutationId}}",
    ] {
        let message = refused(query).await;
        assert!(message.contains("can't use directives"), "{message}");
    }
}

/// A repeated field `Mutation` doesn't have is left to validation, which names it, so the gateway
/// counts it as unsupported as it would without the repeat.
#[tokio::test]
async fn a_repeated_unknown_mutation_is_left_to_validation() {
    let response = builder()
        .finish()
        .execute(
            "mutation{notAField(input:{a:1}){x} ...B} fragment B on Mutation{notAField(input:{a:2}){x}}",
        )
        .await;
    assert!(
        response
            .errors
            .iter()
            .any(|error| error.message.starts_with("Unknown field \"notAField\"")),
        "{:?}",
        response.errors
    );
}

/// The same name for another field or with other arguments can't merge, wherever the repeats
/// sit, and is refused with the overlapping-fields rule's wording.
#[test]
fn a_repeated_name_that_cannot_merge_is_refused() {
    for (query, wording) in [
        (
            "mutation{x:closePullRequest(input:$i){a} ...A} fragment A on Mutation{x:reopenPullRequest(input:$i){a}}",
            "Fields \"x\" conflict because \"closePullRequest\" and \"reopenPullRequest\" are different fields.",
        ),
        (
            "mutation{closePullRequest(input:$i){a} ... {closePullRequest(input:$j){a}}}",
            "Fields \"input\" conflict because they have differing arguments.",
        ),
    ] {
        let message = merged(query).expect_err("refused").message;
        assert!(message.starts_with(wording), "{message}");
    }
}
