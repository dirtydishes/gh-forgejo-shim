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
/// captured documents, the oracle's recorded gh requests, a contract case's, and ChatGPT's.
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
    assert!(documents.len() > 50, "found {} documents", documents.len());
    for (source, query) in documents {
        match validate(builder(), &query).await {
            // Room to spare, so a client's next version or a new field doesn't hit the limits.
            Ok(result) => assert!(
                result.complexity * 2 <= MAX_COMPLEXITY && result.depth + 5 <= MAX_DEPTH,
                "{source}: complexity {}, depth {}: raise the limits",
                result.complexity,
                result.depth
            ),
            // gh's `repo view` with every field asks for one the gateway doesn't serve.
            Err(errors) => assert!(!refused_by_a_limit(&errors), "{source}: {errors:?}"),
        }
    }
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

    let roomy = builder()
        .extension(budget::OutputBudget(size * 2))
        .finish()
        .execute(query)
        .await;
    assert!(roomy.errors.is_empty(), "{:?}", roomy.errors);
    assert_eq!(roomy.data, full.data);
}
