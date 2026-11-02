use serde_json::{Map, Value, json};
use shep_client::dogs::{SHEEP_SCHEMA_KEY, config_schema_with_sheep, validate_answer};

use super::{SheepSettings, parse, verdict};
use crate::config::Section;

fn table(value: Value) -> Map<String, Value> {
    let Value::Object(table) = value else {
        panic!("a table is a JSON object: {value}");
    };
    table
}

#[test]
fn a_table_with_only_allowed_domains_parses() {
    let config = parse(&table(
        json!({"network": {"allowedDomains": ["example.com"]}}),
    ))
    .expect("the omitted lists are empty");

    assert_eq!(config.network.allowed_domains.len(), 1);
    assert!(config.network.denied_domains.is_empty());
    assert!(config.filesystem.deny_read.is_empty());
    assert!(config.filesystem.allow_write.is_empty());
    assert!(config.filesystem.deny_write.is_empty());
}

#[test]
fn an_empty_table_parses() {
    assert!(parse(&Map::new()).is_ok());
}

#[test]
fn a_table_with_a_key_crabpot_refuses_names_the_key() {
    // Only a denied entry may be `*`.
    let err = parse(&table(json!({"network": {"allowedDomains": ["*"]}})))
        .expect_err("crabpot refuses a wildcard allow");

    assert!(err.to_string().contains("network.allowedDomains"), "{err}");
}

#[test]
fn a_group_that_is_not_an_object_is_left_for_crabpot_to_refuse() {
    let err = parse(&table(json!({"network": "open"}))).expect_err("network is a group");

    assert!(err.to_string().contains("network"), "{err}");
}

#[test]
fn an_unknown_key_is_dropped_as_crabpot_drops_it() {
    assert!(parse(&table(json!({"flavour": "mild"}))).is_ok());
}

/// The node a `$ref` points at, or the node itself when it holds none.
fn resolved<'a>(root: &'a Value, node: &'a Value) -> &'a Value {
    match node.get("$ref").and_then(Value::as_str) {
        Some(reference) => root
            .pointer(reference.strip_prefix('#').expect("a local reference"))
            .expect("the reference resolves"),
        None => node,
    }
}

#[test]
fn the_served_schema_requires_none_of_the_five_lists() {
    let schema = config_schema_with_sheep::<Section, SheepSettings>().to_value();
    let sheep = resolved(&schema, &schema[SHEEP_SCHEMA_KEY]);
    let network = resolved(&schema, &sheep["properties"]["network"]);
    let filesystem = resolved(&schema, &sheep["properties"]["filesystem"]);

    for group in [sheep, network, filesystem] {
        assert!(group.get("required").is_none(), "{group}");
    }
    assert!(network["properties"]["allowedDomains"].is_object());
    assert!(filesystem["properties"]["denyWrite"].is_object());
}

fn answer(table: Value) -> Value {
    let params = json!({"table": table, "sheep": "api"}).to_string();
    serde_json::from_str(&validate_answer(Some(&params), verdict)).expect("an answer is JSON")
}

#[test]
fn the_validate_closure_refuses_what_from_json_refuses_and_accepts_what_it_accepts() {
    let refused = answer(json!({"network": {"allowedDomains": ["*"]}}));
    let accepted = answer(json!({"network": {"allowedDomains": ["example.com"]}}));

    assert_eq!(refused["accept"], false, "{refused}");
    assert!(
        refused["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("network.allowedDomains")),
        "{refused}"
    );
    assert_eq!(accepted, json!({"accept": true}));
}
