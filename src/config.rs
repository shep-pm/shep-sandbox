//! The dog's `[sandbox]` section of `dogs.toml`.

use schemars::JsonSchema;
use serde::Deserialize;
use shep_client::dogs::dog_config;

/// The `[sandbox]` section of `dogs.toml`, which has no keys yet.
///
/// An unknown key is refused so that a setting written before it exists is
/// reported rather than ignored.
#[dog_config]
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Section {}

#[cfg(test)]
mod tests {
    use shep_client::dogs::parse_section;

    use super::Section;

    #[test]
    fn an_empty_section_parses() {
        assert!(parse_section::<Section>("sandbox", "").is_ok());
    }

    #[test]
    fn a_key_the_dog_does_not_have_is_refused() {
        let err = parse_section::<Section>("sandbox", "preset = \"strict\"\n")
            .expect_err("no key exists yet");

        assert_eq!(err.section(), "sandbox");
    }
}
