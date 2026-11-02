use async_graphql::parser::{
    parse_schema,
    types::{
        FieldDefinition, InputValueDefinition, TypeDefinition, TypeKind, TypeSystemDefinition,
    },
};
use std::collections::{BTreeMap, BTreeSet};

fn types(sdl: &str) -> BTreeMap<String, TypeDefinition> {
    parse_schema(sdl)
        .unwrap()
        .definitions
        .into_iter()
        .filter_map(|definition| match definition {
            TypeSystemDefinition::Type(ty) => Some((ty.node.name.to_string(), ty.node)),
            _ => None,
        })
        .collect()
}
fn arguments(
    actual: &[async_graphql::Positioned<InputValueDefinition>],
    expected: &[async_graphql::Positioned<InputValueDefinition>],
) {
    for a in actual {
        let e = expected
            .iter()
            .find(|e| e.node.name == a.node.name)
            .expect("known argument/input field");
        assert_eq!(
            a.node.ty.node, e.node.ty.node,
            "{} type/nullability",
            a.node.name
        );
        assert_eq!(
            a.node.default_value.as_ref().map(|v| &v.node),
            e.node.default_value.as_ref().map(|v| &v.node),
            "{} default",
            a.node.name
        );
    }
    for e in expected {
        if !e.node.ty.node.nullable && e.node.default_value.is_none() {
            assert!(
                actual.iter().any(|a| a.node.name == e.node.name),
                "missing required input"
            );
        }
    }
}
fn fields(
    name: &str,
    actual: &[async_graphql::Positioned<FieldDefinition>],
    expected: &[async_graphql::Positioned<FieldDefinition>],
) {
    assert_eq!(
        actual
            .iter()
            .map(|f| f.node.name.to_string())
            .collect::<BTreeSet<_>>(),
        expected.iter().map(|f| f.node.name.to_string()).collect(),
        "{name} pinned fields"
    );
    for a in actual {
        let e = expected
            .iter()
            .find(|e| e.node.name == a.node.name)
            .expect("known field");
        assert_eq!(
            a.node.ty.node, e.node.ty.node,
            "{name}.{} type/nullability",
            a.node.name
        );
        arguments(&a.node.arguments, &e.node.arguments);
    }
}
#[test]
fn implemented_sdl_is_a_subset_of_pinned_github_schema() {
    let reference = types(include_str!("../../../fixtures/github-g1.graphql"));
    let implemented = types(&fga_core::graphql::schema().sdl());
    for (name, a) in &implemented {
        if matches!(name.as_str(), "Int" | "String" | "Boolean" | "ID" | "Float") {
            continue;
        }
        let e = reference
            .get(name)
            .unwrap_or_else(|| panic!("unknown type {name}"));
        match (&a.kind, &e.kind) {
            (TypeKind::Scalar, TypeKind::Scalar) => {}
            (TypeKind::Object(a), TypeKind::Object(e)) => {
                fields(name, &a.fields, &e.fields);
                assert!(
                    a.implements
                        .iter()
                        .map(|n| n.node.to_string())
                        .collect::<BTreeSet<_>>()
                        .is_subset(&e.implements.iter().map(|n| n.node.to_string()).collect()),
                    "{name} interfaces must exist upstream"
                );
            }
            (TypeKind::Interface(a), TypeKind::Interface(e)) => fields(name, &a.fields, &e.fields),
            (TypeKind::InputObject(a), TypeKind::InputObject(e)) => arguments(&a.fields, &e.fields),
            (TypeKind::Enum(a), TypeKind::Enum(e)) => assert_eq!(
                a.values
                    .iter()
                    .map(|v| v.node.value.to_string())
                    .collect::<BTreeSet<_>>(),
                e.values.iter().map(|v| v.node.value.to_string()).collect(),
                "{name} enum values"
            ),
            (TypeKind::Union(a), TypeKind::Union(e)) => assert_eq!(
                a.members
                    .iter()
                    .map(|m| m.node.to_string())
                    .collect::<BTreeSet<_>>(),
                e.members.iter().map(|m| m.node.to_string()).collect(),
                "{name} pinned union members"
            ),
            _ => panic!("{name} type kind differs"),
        }
    }
    for name in reference.keys() {
        assert!(implemented.contains_key(name), "missing pinned type {name}");
    }
}
