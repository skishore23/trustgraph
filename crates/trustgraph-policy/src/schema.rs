//! JSON Schema (draft 2020-12) for `trustgraph.policy.yaml`, mirroring the
//! rules `parse` enforces. Hand-written so editors and agents can validate
//! before running the CLI.

use serde_json::{Value, json};

const NAME: &str = "^[a-z0-9][a-z0-9_.@-]{0,63}$";

fn names(description: &str) -> Value {
    json!({
        "type": "array",
        "description": description,
        "items": {"type": "string", "pattern": NAME},
    })
}

pub fn schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "TrustGraph policy",
        "description": "Where an AI agent's output may go, based on what the agent read. Anything not named is denied.",
        "type": "object",
        "additionalProperties": false,
        "required": ["version"],
        "properties": {
            "version": {"const": 1, "description": "Format version; must be 1."},
            "groups": {
                "type": "object",
                "description": "Named lists of people.",
                "propertyNames": {"pattern": NAME},
                "additionalProperties": names("People in the group.")
            },
            "agents": names("The AI agents this policy governs."),
            "sources": {
                "type": "object",
                "description": "Data an agent may read.",
                "propertyNames": {"pattern": NAME},
                "additionalProperties": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["readers"],
                    "properties": {
                        "readers": names("Groups or people who may receive anything derived from this source."),
                        "agents": names("Agents that may take this source as input.")
                    }
                }
            },
            "actions": {
                "type": "object",
                "description": "Places an agent may send output.",
                "propertyNames": {"pattern": NAME},
                "additionalProperties": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["agents", "audience"],
                    "properties": {
                        "agents": names("Agents that may attempt the action."),
                        "audience": names("Groups or people who receive the output."),
                        "approval": {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["reviewers", "valid_for"],
                            "properties": {
                                "reviewers": names("Groups or people who may approve."),
                                "valid_for": {
                                    "type": "string",
                                    "pattern": "^[0-9]+[smhd]$",
                                    "description": "How long an approval stays valid, e.g. 30m, 2h, 1d (at most 30d)."
                                }
                            }
                        }
                    }
                }
            },
            "tests": {
                "type": "array",
                "description": "Inline examples run by `trustgraph policy test`.",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["name", "output_from", "agent", "action", "expect"],
                    "properties": {
                        "name": {"type": "string", "description": "Unique test name."},
                        "output_from": names("Sources the agent read."),
                        "agent": {"type": "string", "pattern": NAME},
                        "action": {"type": "string", "pattern": NAME},
                        "recipients": names("Narrow the audience to these people or groups."),
                        "expect": {"enum": ["allow", "deny", "needs_approval"]}
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{ACTION_KEYS, APPROVAL_KEYS, CASE_KEYS, SOURCE_KEYS, TOP_KEYS};

    fn keys(v: &Value) -> Vec<&str> {
        v["properties"]
            .as_object()
            .expect("properties")
            .keys()
            .map(String::as_str)
            .collect()
    }

    fn same(schema_keys: Vec<&str>, parser: &[&str], what: &str) {
        let mut a = schema_keys;
        let mut b = parser.to_vec();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b, "{what} keys differ between schema and parser");
    }

    #[test]
    fn every_parser_key_is_in_the_schema() {
        let s = schema();
        same(keys(&s), TOP_KEYS, "top-level");
        same(
            keys(&s["properties"]["sources"]["additionalProperties"]),
            SOURCE_KEYS,
            "source",
        );
        let action = &s["properties"]["actions"]["additionalProperties"];
        same(keys(action), ACTION_KEYS, "action");
        same(
            keys(&action["properties"]["approval"]),
            APPROVAL_KEYS,
            "approval",
        );
        same(
            keys(&s["properties"]["tests"]["items"]),
            CASE_KEYS,
            "test case",
        );
    }

    #[test]
    fn closed_objects_and_dialect() {
        let s = schema();
        assert_eq!(s["$schema"], "https://json-schema.org/draft/2020-12/schema");
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(s["properties"]["version"]["const"], 1);
        assert_eq!(
            s["properties"]["tests"]["items"]["properties"]["expect"]["enum"],
            json!(["allow", "deny", "needs_approval"])
        );
    }
}
