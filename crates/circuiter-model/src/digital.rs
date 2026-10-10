//! Digital view: how a cell runs in the digital simulator, as one of its built-in parts
//! (`dff`, `alu`, `rom`…) with parameters. Parameter values may use `${param.NAME}` to
//! take the instance's own parameters, as SPICE templates do. Ports are matched by
//! name; cell ports the part doesn't have (a gate's hidden supply pins) are ignored.
//!
//! ```ron
//! (model: "dff", params: {"width": "${param.width}", "has_reset": "1"})
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DigitalView {
    pub model: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// The parameter naming a file to load the part's contents from: a ROM's program
    /// (assembly, C or an ELF executable), relative to the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contents: Option<String>,
    /// Cell ports named differently on the part: cell port → part port.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ports: BTreeMap<String, String>,
}

/// `text` with every `${param.NAME}` replaced by `lookup(NAME)`.
pub fn expand(text: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find('}').ok_or("unterminated `${`")?;
        let key = &after[..end];
        let name = key
            .strip_prefix("param.")
            .ok_or_else(|| format!("unknown placeholder `${{{key}}}`"))?;
        out.push_str(&lookup(name).ok_or_else(|| format!("no parameter `{name}`"))?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}
