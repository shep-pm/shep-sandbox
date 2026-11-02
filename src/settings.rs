//! A sheep's `[app.dogs.sandbox]` table: crabpot's own settings.
//!
//! The keys, their types and their camelCase names are crabpot's. The dog
//! serves crabpot's schema for them and hands a proposed table to
//! [`Config::from_json`], so `shep` refuses what crabpot would refuse.

use std::borrow::Cow;

use crabpot::Config;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde_json::{Map, Value};
use shep_client::dogs::{Proposed, Verdict, dog_config};

use crate::error::Error;

/// The groups crabpot requires, and the lists it requires inside each.
const REQUIRED_LISTS: [(&str, &str); 5] = [
    ("network", "allowedDomains"),
    ("network", "deniedDomains"),
    ("filesystem", "denyRead"),
    ("filesystem", "allowWrite"),
    ("filesystem", "denyWrite"),
];

/// The definitions of crabpot's schema that carry a `required` list.
const GROUP_DEFINITIONS: [&str; 3] = ["Config", "Network", "Filesystem"];

/// The settings table of one sheep, as the schema describes it.
///
/// Only the schema is served from this type: the table is read by [`parse`].
#[dog_config]
#[derive(Debug)]
pub struct SheepSettings {}

// The schema is crabpot's with every `required` entry dropped, because
// `parse` fills what crabpot requires and a table need not spell it out.
impl JsonSchema for SheepSettings {
    fn schema_name() -> Cow<'static, str> {
        "SheepSettings".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let schema = generator.subschema_for::<Config>();
        for name in GROUP_DEFINITIONS {
            if let Some(Value::Object(definition)) = generator.definitions_mut().get_mut(name) {
                definition.remove("required");
            }
        }
        schema
    }
}

/// `table` with the five lists crabpot requires added, empty, where it omits
/// them.
///
/// A group that is not an object is left as it is, for crabpot to refuse.
pub fn filled(table: &Map<String, Value>) -> Map<String, Value> {
    let mut settings = table.clone();
    for (group, list) in REQUIRED_LISTS {
        let group = settings
            .entry(group)
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(group) = group {
            group
                .entry(list)
                .or_insert_with(|| Value::Array(Vec::new()));
        }
    }
    settings
}

/// The settings `table` holds, as crabpot reads them.
///
/// The five lists crabpot requires are empty when the table omits them. A key
/// crabpot does not know is dropped, as crabpot drops it.
///
/// # Errors
///
/// [`Error::Settings`] when crabpot refuses the table, naming the key.
pub fn parse(table: &Map<String, Value>) -> Result<Config, Error> {
    Config::from_json(&Value::Object(filled(table)).to_string()).map_err(Error::Settings)
}

/// What the dog answers a `shep.validate` action with for `proposed`.
pub fn verdict(proposed: &Proposed) -> Verdict {
    match parse(proposed.table()) {
        Ok(_) => Verdict::Accept,
        Err(err) => Verdict::refuse(err.to_string()),
    }
}

#[cfg(test)]
mod tests;
