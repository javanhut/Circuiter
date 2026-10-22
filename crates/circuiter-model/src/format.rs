//! On-disk encoding helpers. Design files are RON, written with a fixed layout so that
//! saving an unchanged design produces byte-identical files.
//!
//! The layout works like rustfmt: a bracketed group stays on one line if it fits within
//! [`WIDTH`] columns, otherwise each of its items goes on its own line. Comments are not
//! preserved, because files are rewritten whenever a design is saved.

use crate::error::{ModelError, Result};
use ron::extensions::Extensions;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::Path;

fn options() -> ron::Options {
    // `implicit_some` lets files say `global_net: "GND"` instead of `Some("GND")`.
    ron::Options::default().with_default_extension(Extensions::IMPLICIT_SOME)
}

/// Maximum line width for the canonical layout.
const WIDTH: usize = 100;
const INDENT: usize = 4;

/// `value` in the canonical layout the design files use; `path` names it in errors.
pub fn to_ron<T: Serialize>(path: &Path, value: &T) -> Result<String> {
    let compact = options().to_string(value).map_err(|e| ModelError::parse(path, e))?;
    Ok(layout(&compact))
}

/// Reads RON as the design files are read; `path` names the text in errors.
pub fn from_ron<T: DeserializeOwned>(path: &Path, text: &str) -> Result<T> {
    options().from_str(text).map_err(|e| ModelError::parse(path, e))
}

pub(crate) fn write_file(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ModelError::io(parent, e))?;
    }
    // Skip unchanged files so saving doesn't touch mtimes or create diffs.
    if std::fs::read_to_string(path).is_ok_and(|old| old == contents) {
        return Ok(());
    }
    std::fs::write(path, contents).map_err(|e| ModelError::io(path, e))
}

/// Re-lays out compact RON (as produced by `ron::to_string`) in the canonical layout.
fn layout(compact: &str) -> String {
    let item = parse_item(&mut Tokens::new(compact), None);
    let mut out = String::new();
    match item.as_slice() {
        // The top-level value is always broken out, one field per line.
        [Node::Group(group)] if !group.items.is_empty() => write_broken(group, 0, &mut out),
        _ => write_item_flat(&item, &mut out),
    }
    out.push('\n');
    out
}

#[derive(Debug)]
enum Node {
    /// Identifiers, numbers, strings and `:`.
    Atom(String),
    Group(Group),
}

#[derive(Debug)]
struct Group {
    open: char,
    close: char,
    /// Comma-separated items; each is a sequence like `key`, `:`, `(..)`.
    items: Vec<Vec<Node>>,
}

struct Tokens<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl<'a> Tokens<'a> {
    fn new(s: &'a str) -> Self {
        Tokens { chars: s.chars().peekable() }
    }
}

fn closer(open: char) -> char {
    match open {
        '(' => ')',
        '[' => ']',
        _ => '}',
    }
}

/// Parses one comma-separated item. Consumes the trailing `,` if there is one, and stops
/// before `close` so the enclosing group can consume it.
fn parse_item(t: &mut Tokens<'_>, close: Option<char>) -> Vec<Node> {
    let mut item = Vec::new();
    let mut atom = String::new();
    let flush = |atom: &mut String, item: &mut Vec<Node>| {
        if !atom.is_empty() {
            item.push(Node::Atom(std::mem::take(atom)));
        }
    };
    while let Some(&c) = t.chars.peek() {
        match c {
            '"' => {
                flush(&mut atom, &mut item);
                t.chars.next();
                let mut s = String::from('"');
                while let Some(c) = t.chars.next() {
                    s.push(c);
                    match c {
                        '\\' => s.extend(t.chars.next()),
                        '"' => break,
                        _ => {}
                    }
                }
                item.push(Node::Atom(s));
            }
            '(' | '[' | '{' => {
                flush(&mut atom, &mut item);
                t.chars.next();
                item.push(Node::Group(parse_group(t, c)));
            }
            ',' => {
                flush(&mut atom, &mut item);
                t.chars.next();
                return item;
            }
            ':' => {
                flush(&mut atom, &mut item);
                t.chars.next();
                item.push(Node::Atom(":".to_owned()));
            }
            c if Some(c) == close => {
                flush(&mut atom, &mut item);
                return item;
            }
            c if c.is_whitespace() => {
                flush(&mut atom, &mut item);
                t.chars.next();
            }
            _ => {
                atom.push(c);
                t.chars.next();
            }
        }
    }
    flush(&mut atom, &mut item);
    item
}

fn parse_group(t: &mut Tokens<'_>, open: char) -> Group {
    let close = closer(open);
    let mut items = Vec::new();
    loop {
        let item = parse_item(t, Some(close));
        if !item.is_empty() {
            items.push(item);
        }
        match t.chars.peek() {
            Some(&c) if c == close => {
                t.chars.next();
                break;
            }
            None => break,
            _ => {} // a comma was consumed; continue with the next item
        }
    }
    Group { open, close, items }
}

fn write_item_flat(item: &[Node], out: &mut String) {
    for node in item {
        match node {
            Node::Atom(a) if a == ":" => out.push_str(": "),
            Node::Atom(a) => out.push_str(a),
            Node::Group(g) => write_group_flat(g, out),
        }
    }
}

fn write_group_flat(g: &Group, out: &mut String) {
    out.push(g.open);
    for (i, item) in g.items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_item_flat(item, out);
    }
    out.push(g.close);
}

/// Writes `item` (followed by a trailing comma) starting at column `indent`.
fn write_item(item: &[Node], indent: usize, out: &mut String) {
    let mut flat = String::new();
    write_item_flat(item, &mut flat);
    let last_group = item.iter().rposition(|n| matches!(n, Node::Group(g) if !g.items.is_empty()));
    match last_group {
        Some(i) if indent + flat.chars().count() + 1 > WIDTH => {
            write_item_flat(&item[..i], out);
            let Node::Group(g) = &item[i] else { unreachable!() };
            write_broken(g, indent, out);
            write_item_flat(&item[i + 1..], out);
        }
        _ => out.push_str(&flat),
    }
    out.push(',');
}

fn write_broken(g: &Group, indent: usize, out: &mut String) {
    out.push(g.open);
    out.push('\n');
    for item in &g.items {
        out.extend(std::iter::repeat_n(' ', indent + INDENT));
        write_item(item, indent + INDENT, out);
        out.push('\n');
    }
    out.extend(std::iter::repeat_n(' ', indent));
    out.push(g.close);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_groups_stay_inline_and_long_ones_break() {
        let compact =
            r#"(name:"MP",cell:"std:pmos",placement:(position:(10.16,5.08)),params:{"w":"2u"})"#;
        assert_eq!(
            layout(compact),
            "(\n    name: \"MP\",\n    cell: \"std:pmos\",\n    placement: (position: (10.16, 5.08)),\n    params: {\"w\": \"2u\"},\n)\n"
        );
        let long = format!("(points:[{}])", ["(10.16,-2.54)"; 12].join(","));
        let text = layout(&long);
        assert!(text.contains("    points: [\n        (10.16, -2.54),\n"), "{text}");
    }

    #[test]
    fn strings_with_brackets_commas_and_escapes_are_untouched() {
        let compact = r#"(template:"V${inst} SIN(0, 1) {a:b} \"q\"")"#;
        assert_eq!(
            layout(compact),
            "(\n    template: \"V${inst} SIN(0, 1) {a:b} \\\"q\\\"\",\n)\n"
        );
    }

    #[test]
    fn empty_groups_and_unit_values() {
        assert_eq!(layout("(instances:[],wires:[])"), "(\n    instances: [],\n    wires: [],\n)\n");
        assert_eq!(layout("()"), "()\n");
    }
}
