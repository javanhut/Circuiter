//! SPICE view: how a cell is expressed in a SPICE netlist.
//!
//! The template uses `${...}` placeholders, so SPICE's own `{expression}` syntax can be
//! used freely:
//!
//! - `${inst}` is the flattened, unique instance name
//! - `${node.PORT}` is the net connected to port `PORT`
//! - `${param.NAME}` is the value of parameter `NAME`; quantities are emitted as plain
//!   numbers, since SPICE reads `M` as milli
//! - `${model}` is a model name unique to this cell and its expanded `models` cards, so
//!   instances with identical model parameters share one `.model` card
//!
//! For example, a resistor: `R${inst} ${node.a} ${node.b} ${param.R}`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpiceView {
    pub template: String,
    /// `.model` / `.subckt` cards this view depends on. They may use placeholders and
    /// are emitted once per distinct expansion.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    /// A template that depends on a parameter's value instead: a multimeter is a
    /// resistor, a zero-volt source or a current source depending on its mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choose: Option<Choose>,
}

/// Templates by the value of one parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choose {
    pub param: String,
    pub templates: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placeholder<'a> {
    Instance,
    Model,
    Node(&'a str),
    Param(&'a str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment<'a> {
    Text(&'a str),
    Placeholder(Placeholder<'a>),
}

/// Splits a template into literal text and placeholders.
pub fn parse_template(template: &str) -> Result<Vec<Segment<'_>>, String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        if start > 0 {
            out.push(Segment::Text(&rest[..start]));
        }
        let after = &rest[start + 2..];
        let end = after.find('}').ok_or_else(|| "unterminated `${` in template".to_owned())?;
        let key = &after[..end];
        let placeholder = if key == "inst" {
            Placeholder::Instance
        } else if key == "model" {
            Placeholder::Model
        } else if let Some(port) = key.strip_prefix("node.") {
            Placeholder::Node(port)
        } else if let Some(param) = key.strip_prefix("param.") {
            Placeholder::Param(param)
        } else {
            return Err(format!(
                "unknown placeholder `${{{key}}}`; expected `inst`, `model`, `node.PORT` or `param.NAME`"
            ));
        };
        out.push(Segment::Placeholder(placeholder));
        rest = &after[end + 1..];
    }
    if !rest.is_empty() {
        out.push(Segment::Text(rest));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_placeholders_and_keeps_spice_braces() {
        let segs = parse_template("R${inst} ${node.a} ${node.b} {${param.R}*2}").unwrap();
        assert_eq!(
            segs,
            vec![
                Segment::Text("R"),
                Segment::Placeholder(Placeholder::Instance),
                Segment::Text(" "),
                Segment::Placeholder(Placeholder::Node("a")),
                Segment::Text(" "),
                Segment::Placeholder(Placeholder::Node("b")),
                Segment::Text(" {"),
                Segment::Placeholder(Placeholder::Param("R")),
                Segment::Text("*2}"),
            ]
        );
    }

    #[test]
    fn rejects_bad_placeholders() {
        assert!(parse_template("${nope}").is_err());
        assert_eq!(
            parse_template("${model}").unwrap(),
            vec![Segment::Placeholder(Placeholder::Model)]
        );
        assert!(parse_template("${node.a").is_err());
    }
}
