//! Reading SPICE decks: element cards, `.model` cards, continuation lines and comments.
//!
//! The dialect is the common subset ngspice, LTspice and PSpice agree on. Names and
//! keywords are case-insensitive. Analysis cards (`.op`, `.tran`...) are skipped: the
//! caller chooses the analysis.

use crate::number::{eval, eval_with, parse_number};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq)]
pub enum Waveform {
    /// `PULSE(V1 V2 TD TR TF PW PER)`. Unset times default from the analysis.
    Pulse {
        v1: f64,
        v2: f64,
        td: f64,
        tr: Option<f64>,
        tf: Option<f64>,
        pw: Option<f64>,
        per: Option<f64>,
    },
    /// `SIN(VO VA FREQ TD THETA PHASE)`; phase in degrees.
    Sin { vo: f64, va: f64, freq: Option<f64>, td: f64, theta: f64, phase: f64 },
    /// `PWL(T1 V1 T2 V2 ...)`.
    Pwl(Vec<(f64, f64)>),
}

/// The value of an independent source.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceSpec {
    pub dc: Option<f64>,
    /// AC magnitude and phase (degrees).
    pub ac: Option<(f64, f64)>,
    pub tran: Option<Waveform>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    /// The element name as written, e.g. `MU1.MN`. Its first letter is the kind.
    pub name: String,
    pub nodes: Vec<String>,
    /// Positional values after the nodes (a resistance, a diode area...).
    pub values: Vec<f64>,
    pub model: Option<String>,
    /// Other elements this one refers to: the inductors a `K` couples.
    pub refs: Vec<String>,
    /// `KEY=value` parameters, keys in lower case.
    pub params: BTreeMap<String, f64>,
    pub source: Option<SourceSpec>,
    /// For `POLY(n)` controlled sources, the number of controlling inputs. Their nodes
    /// (E, G) or source names (F, H) follow the output nodes; `values` holds the
    /// polynomial coefficients.
    pub poly: Option<usize>,
    pub line: usize,
}

impl Card {
    pub fn kind(&self) -> char {
        self.name.chars().next().unwrap_or(' ').to_ascii_uppercase()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelCard {
    /// Lower case.
    pub name: String,
    /// Lower case: `d`, `nmos`, `pmos`, `npn`...
    pub kind: String,
    /// Keys in lower case.
    pub params: BTreeMap<String, f64>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Deck {
    pub cards: Vec<Card>,
    pub models: Vec<ModelCard>,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Open,
    Close,
    Equals,
}

fn tokenize(line: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut Vec<Token>| {
        if !word.is_empty() {
            out.push(Token::Word(std::mem::take(word)));
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                flush(&mut word, &mut out);
                let mut depth = 1;
                let mut expr = String::from("{");
                for c in chars.by_ref() {
                    expr.push(c);
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                out.push(Token::Word(expr));
            }
            '(' => {
                flush(&mut word, &mut out);
                out.push(Token::Open);
            }
            ')' => {
                flush(&mut word, &mut out);
                out.push(Token::Close);
            }
            '=' => {
                flush(&mut word, &mut out);
                out.push(Token::Equals);
            }
            c if c.is_whitespace() || c == ',' => flush(&mut word, &mut out),
            c => word.push(c),
        }
    }
    flush(&mut word, &mut out);
    out
}

/// A value: a number with an optional scale suffix, or a `{...}` expression.
pub fn value(text: &str) -> Result<f64, String> {
    if let Some(inner) = text.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
        return eval(inner);
    }
    parse_number(text).ok_or_else(|| format!("`{text}` is not a number"))
}

/// Joins continuation lines and drops comments, keeping each logical line's number.
fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let mut line = raw.trim_end();
        // Inline comments: `;` anywhere, `$` after whitespace (ngspice).
        if let Some(k) = line.find(';') {
            line = &line[..k];
        }
        if let Some(k) = line.find(" $").or_else(|| line.find("\t$")) {
            line = &line[..k];
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('*') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('+') {
            if let Some(last) = out.last_mut() {
                last.1.push(' ');
                last.1.push_str(rest);
                continue;
            }
        }
        out.push((i + 1, trimmed.to_owned()));
    }
    out
}

/// A `.subckt` definition: ports, default parameters (as expressions) and body lines.
#[derive(Debug, Clone)]
struct Subckt {
    ports: Vec<String>,
    params: Vec<(String, String)>,
    body: Vec<(usize, String)>,
}

/// What names mean while expanding one level of the hierarchy.
#[derive(Debug, Clone, Default)]
struct Scope {
    /// Prepended to local node and element names: `X1.`, `X1.X2.`.
    prefix: String,
    /// Port name (lower case) to the node it is connected to outside.
    ports: HashMap<String, String>,
    /// Parameter values by lower-case name.
    params: HashMap<String, f64>,
    /// Local model name (lower case) to its name in the flattened deck.
    models: HashMap<String, String>,
    subckts: HashMap<String, Rc<Subckt>>,
}

impl Scope {
    fn node(&self, name: &str) -> String {
        let lower = name.to_ascii_lowercase();
        if lower == "0" || lower == "gnd" {
            return "0".to_owned();
        }
        match self.ports.get(&lower) {
            Some(outer) => outer.clone(),
            None => format!("{}{name}", self.prefix),
        }
    }

    fn element(&self, name: &str) -> String {
        if self.prefix.is_empty() {
            return name.to_owned();
        }
        // Keep the kind letter first, as SPICE does: `R1` in `X1` is `R.X1.R1`.
        let letter = &name[..name.chars().next().map_or(0, char::len_utf8)];
        format!("{letter}.{}{name}", self.prefix)
    }

    fn eval(&self, expr: &str) -> Result<f64, String> {
        let inner = expr.strip_prefix('{').and_then(|e| e.strip_suffix('}')).unwrap_or(expr);
        eval_with(inner, &|n| self.params.get(n).copied())
    }

    /// Replaces every `{expression}` in `line` by its value.
    fn substitute(&self, line: &str) -> Result<String, String> {
        let mut out = String::with_capacity(line.len());
        let mut rest = line;
        while let Some(start) = rest.find('{') {
            out.push_str(&rest[..start]);
            let mut depth = 0;
            let mut end = None;
            for (k, c) in rest[start..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(start + k);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let end = end.ok_or("unmatched `{`")?;
            let v = self.eval(&rest[start..=end])?;
            out.push_str(&format!("{v:e}"));
            rest = &rest[end + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }
}

/// Parses a deck, expanding sub-circuits. Errors name the line they occurred on.
pub fn parse(text: &str) -> Result<Deck, String> {
    let mut lines = logical_lines(text);
    if let Some(end) = lines.iter().position(|(_, l)| first_word(l) == ".end") {
        lines.truncate(end);
    }
    // ngspice `.control` blocks hold interactive commands, not circuit.
    let mut in_control = false;
    lines.retain(|(_, l)| match first_word(l).as_str() {
        ".control" => {
            in_control = true;
            false
        }
        ".endc" => {
            in_control = false;
            false
        }
        _ => !in_control,
    });
    let mut deck = Deck::default();
    expand(&lines, Scope::default(), &mut deck, 0)?;
    Ok(deck)
}

fn first_word(line: &str) -> String {
    line.split_whitespace().next().unwrap_or("").to_ascii_lowercase()
}

/// Expands one level: collects its sub-circuit definitions, parameters and models, then
/// its elements, instantiating sub-circuits recursively.
fn expand(
    lines: &[(usize, String)],
    mut scope: Scope,
    deck: &mut Deck,
    depth: usize,
) -> Result<(), String> {
    if depth > 64 {
        return Err("sub-circuits nest more than 64 deep; is one instantiating itself?".into());
    }
    // Definitions, which may nest.
    let mut body = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (line, text) = &lines[i];
        if first_word(text) != ".subckt" {
            body.push((*line, text.clone()));
            i += 1;
            continue;
        }
        let mut nesting = 0;
        let mut end = None;
        for (k, (_, t)) in lines.iter().enumerate().skip(i) {
            match first_word(t).as_str() {
                ".subckt" => nesting += 1,
                ".ends" => {
                    nesting -= 1;
                    if nesting == 0 {
                        end = Some(k);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end.ok_or(format!("line {line}: .subckt without .ends"))?;
        let tokens = tokenize(text);
        let words: Vec<&str> = tokens.iter().filter_map(word).collect();
        let name = words.get(1).ok_or(format!("line {line}: .subckt needs a name"))?;
        let (ports, params) =
            split_params(&tokens[2..]).map_err(|e| format!("line {line}: {e}"))?;
        let def = Subckt { ports, params, body: lines[i + 1..end].to_vec() };
        scope.subckts.insert(name.to_ascii_lowercase(), Rc::new(def));
        i = end + 1;
    }

    // Parameters, then models, then elements.
    for (line, text) in body.iter().filter(|(_, t)| first_word(t) == ".param") {
        let at = |e: String| format!("line {line}: {e}");
        let tokens = tokenize(text);
        let (_, pairs) = split_params(&tokens[1..]).map_err(at)?;
        for (k, expr) in pairs {
            let v = scope.eval(&expr).map_err(at)?;
            scope.params.insert(k, v);
        }
    }
    for (line, text) in body.iter().filter(|(_, t)| first_word(t) == ".model") {
        let at = |e: String| format!("line {line}: {e}");
        let tokens = tokenize(&scope.substitute(text).map_err(at)?);
        let mut card = model_card(&tokens, *line).map_err(at)?;
        if !scope.prefix.is_empty() {
            let global = format!("{}{}", scope.prefix.to_ascii_lowercase(), card.name);
            scope.models.insert(card.name.clone(), global.clone());
            card.name = global;
        }
        deck.models.push(card);
    }
    for (line, text) in &body {
        if text.starts_with('.') {
            // Analyses, options, output and control are chosen by the caller.
            continue;
        }
        let at = |e: String| format!("line {line}: {e}");
        let substituted = scope.substitute(text).map_err(at)?;
        let tokens = tokenize(&substituted);
        let Some(Token::Word(first)) = tokens.first() else { continue };
        if first.starts_with(['x', 'X']) {
            instantiate(&tokens, &scope, deck, depth).map_err(at)?;
            continue;
        }
        let mut card = element_card(&tokens, *line).map_err(at)?;
        if !scope.prefix.is_empty() || !scope.models.is_empty() {
            card.name = scope.element(&card.name);
            card.nodes = card.nodes.iter().map(|n| scope.node(n)).collect();
            if matches!(card.kind(), 'K' | 'F' | 'H') {
                card.refs = card.refs.iter().map(|r| scope.element(r)).collect();
            }
            if let Some(m) = &card.model {
                if let Some(global) = scope.models.get(m) {
                    card.model = Some(global.clone());
                }
            }
            if card.kind() == 'Q' {
                card.refs = card
                    .refs
                    .iter()
                    .map(|r| {
                        scope.models.get(&r.to_ascii_lowercase()).cloned().unwrap_or(r.clone())
                    })
                    .collect();
            }
        }
        deck.cards.push(card);
    }
    Ok(())
}

/// Splits `nodes... [params:] key=value...` into the names and the parameter
/// expressions (lower-case keys, values as written).
/// Names, then `(key, expression)` parameters.
type Split = (Vec<String>, Vec<(String, String)>);

fn split_params(tokens: &[Token]) -> Result<Split, String> {
    let mut names = Vec::new();
    let mut params = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let w = word(&tokens[i]).ok_or("unexpected punctuation")?;
        if tokens.get(i + 1) == Some(&Token::Equals) {
            let v = tokens.get(i + 2).and_then(word).ok_or(format!("`{w}=` has no value"))?;
            params.push((w.to_ascii_lowercase(), v.to_owned()));
            i += 3;
        } else {
            if !w.eq_ignore_ascii_case("params:") {
                names.push(w.to_owned());
            }
            i += 1;
        }
    }
    Ok((names, params))
}

/// Expands `Xname nodes... subckt [params]`.
fn instantiate(
    tokens: &[Token],
    scope: &Scope,
    deck: &mut Deck,
    depth: usize,
) -> Result<(), String> {
    let (mut words, overrides) = split_params(tokens)?;
    let xname = words.remove(0);
    let sub = words.pop().ok_or(format!("{xname} needs a sub-circuit name"))?;
    let def = scope
        .subckts
        .get(&sub.to_ascii_lowercase())
        .ok_or(format!("{xname}: no sub-circuit `{sub}`"))?
        .clone();
    if words.len() != def.ports.len() {
        return Err(format!(
            "{xname}: `{sub}` has {} ports but {} nodes are given",
            def.ports.len(),
            words.len()
        ));
    }
    let mut child = Scope {
        prefix: format!("{}{xname}.", scope.prefix),
        ports: def
            .ports
            .iter()
            .zip(&words)
            .map(|(port, node)| (port.to_ascii_lowercase(), scope.node(node)))
            .collect(),
        params: scope.params.clone(),
        models: scope.models.clone(),
        subckts: scope.subckts.clone(),
    };
    // Defaults may refer to each other and to outer parameters; instance values are
    // evaluated where the instance is.
    for (k, expr) in &def.params {
        let v = child.eval(expr).map_err(|e| format!("{xname}: default for `{k}`: {e}"))?;
        child.params.insert(k.clone(), v);
    }
    for (k, expr) in overrides {
        let v = scope.eval(&expr).map_err(|e| format!("{xname}: `{k}`: {e}"))?;
        child.params.insert(k, v);
    }
    expand(&def.body, child, deck, depth + 1)
}

fn word(t: &Token) -> Option<&str> {
    match t {
        Token::Word(w) => Some(w),
        _ => None,
    }
}

fn model_card(tokens: &[Token], line: usize) -> Result<ModelCard, String> {
    let name = tokens.get(1).and_then(word).ok_or(".model needs a name")?;
    let kind = tokens.get(2).and_then(word).ok_or(".model needs a type")?;
    let params = key_values(&tokens[3..])?;
    Ok(ModelCard { name: name.to_ascii_lowercase(), kind: kind.to_ascii_lowercase(), params, line })
}

/// `KEY=value` pairs, ignoring the parentheses around them.
fn key_values(tokens: &[Token]) -> Result<BTreeMap<String, f64>, String> {
    let mut out = BTreeMap::new();
    let tokens: Vec<&Token> =
        tokens.iter().filter(|t| !matches!(t, Token::Open | Token::Close)).collect();
    let mut i = 0;
    while i < tokens.len() {
        let key = word(tokens[i]).ok_or("expected a parameter name")?;
        if tokens.get(i + 1) != Some(&&Token::Equals) {
            // A bare flag (e.g. `OFF`); not supported values are ignored.
            i += 1;
            continue;
        }
        let v = tokens.get(i + 2).and_then(|t| word(t)).ok_or(format!("`{key}=` has no value"))?;
        out.insert(key.to_ascii_lowercase(), value(v)?);
        i += 3;
    }
    Ok(out)
}

fn element_card(tokens: &[Token], line: usize) -> Result<Card, String> {
    let name = word(&tokens[0]).expect("caller checked").to_owned();
    let kind = name.chars().next().unwrap_or(' ').to_ascii_uppercase();
    let n_nodes = match kind {
        'R' | 'C' | 'L' | 'V' | 'I' | 'D' => 2,
        'M' => 4,
        'Q' => 3,
        'E' | 'G' => 4,
        'F' | 'H' => 2,
        'S' => 4,
        'W' => 2,
        'K' => 0,
        _ => return Err(format!("{name}: `{kind}` elements are not supported yet")),
    };
    let mut card = Card {
        name: name.clone(),
        nodes: Vec::new(),
        values: Vec::new(),
        model: None,
        refs: Vec::new(),
        params: BTreeMap::new(),
        source: None,
        poly: None,
        line,
    };
    if matches!(kind, 'E' | 'G' | 'F' | 'H')
        && tokens.get(3).and_then(word).is_some_and(|w| w.eq_ignore_ascii_case("poly"))
    {
        return poly_card(card, tokens);
    }
    let mut i = 1;
    for _ in 0..n_nodes {
        let node = tokens.get(i).and_then(word).ok_or(format!("{name} needs {n_nodes} nodes"))?;
        card.nodes.push(node.to_owned());
        i += 1;
    }
    let rest = &tokens[i..];
    match kind {
        'V' | 'I' => card.source = Some(source_spec(rest).map_err(|e| format!("{name}: {e}"))?),
        'K' => {
            let words: Vec<&str> = rest.iter().filter_map(word).collect();
            let [a, b, k] = words[..] else {
                return Err(format!("{name} needs two inductors and a coupling factor"));
            };
            card.refs = vec![a.to_owned(), b.to_owned()];
            card.values = vec![value(k)?];
        }
        'S' | 'W' => {
            // `[control source] model [ON|OFF]`.
            let mut words: Vec<&str> = rest.iter().filter_map(word).collect();
            if kind == 'W' {
                if words.is_empty() {
                    return Err(format!("{name} needs a controlling voltage source"));
                }
                card.refs = vec![words.remove(0).to_owned()];
            }
            let model = words.first().ok_or(format!("{name} needs a switch model"))?;
            card.model = Some(model.to_ascii_lowercase());
            if words.get(1).is_some_and(|w| w.eq_ignore_ascii_case("on")) {
                card.params.insert("on".into(), 1.0);
            }
        }
        'F' | 'H' => {
            let words: Vec<&str> = rest.iter().filter_map(word).collect();
            let [source, gain] = words[..] else {
                return Err(format!("{name} needs a controlling voltage source and a gain"));
            };
            card.refs = vec![source.to_owned()];
            card.values = vec![value(gain)?];
        }
        'Q' => {
            // `[substrate] model [area]`: which word is the model is only known once
            // the models are, so they are resolved when the circuit is built.
            let (words, params) = values_and_params_raw(rest)?;
            card.refs = words;
            card.params = params;
        }
        'D' | 'M' => {
            let model = rest.first().and_then(word).ok_or(format!("{name} needs a model name"))?;
            card.model = Some(model.to_ascii_lowercase());
            let (values, params) = values_and_params(&rest[1..])?;
            card.values = values;
            card.params = params;
        }
        _ => {
            let (values, params) = values_and_params(rest)?;
            card.values = values;
            card.params = params;
            if card.values.is_empty() {
                return Err(format!("{name} needs a value"));
            }
        }
    }
    Ok(card)
}

/// `E/G name n+ n- POLY(n) nc1+ nc1- ... p0 p1 ...` and
/// `F/H name n+ n- POLY(n) v1 ... p0 p1 ...`.
fn poly_card(mut card: Card, tokens: &[Token]) -> Result<Card, String> {
    let name = card.name.clone();
    let dims = match (tokens.get(4), tokens.get(5).and_then(word), tokens.get(6)) {
        (Some(Token::Open), Some(n), Some(Token::Close)) => {
            n.parse::<usize>().map_err(|_| format!("{name}: POLY({n}) needs a whole number"))?
        }
        _ => return Err(format!("{name}: expected POLY(n)")),
    };
    if dims == 0 {
        return Err(format!("{name}: POLY needs at least one input"));
    }
    let words: Vec<&str> = tokens.iter().filter_map(word).collect();
    // name, n+, n-, POLY, n, then inputs and coefficients.
    let inputs = if matches!(card.kind(), 'E' | 'G') { 2 * dims } else { dims };
    let rest = words.get(5..).unwrap_or_default();
    if rest.len() < inputs {
        return Err(format!("{name}: POLY({dims}) needs {inputs} controlling inputs"));
    }
    card.nodes = vec![words[1].to_owned(), words[2].to_owned()];
    let (controls, coeffs) = rest.split_at(inputs);
    if matches!(card.kind(), 'E' | 'G') {
        card.nodes.extend(controls.iter().map(|w| (*w).to_owned()));
    } else {
        card.refs = controls.iter().map(|w| (*w).to_owned()).collect();
    }
    card.values = coeffs.iter().map(|w| value(w)).collect::<Result<_, _>>()?;
    card.poly = Some(dims);
    Ok(card)
}

fn values_and_params_raw(tokens: &[Token]) -> Result<(Vec<String>, BTreeMap<String, f64>), String> {
    let split = tokens.iter().position(|t| *t == Token::Equals).map_or(tokens.len(), |k| k - 1);
    let words = tokens[..split].iter().filter_map(word).map(str::to_owned).collect();
    Ok((words, key_values(&tokens[split..])?))
}

fn values_and_params(tokens: &[Token]) -> Result<(Vec<f64>, BTreeMap<String, f64>), String> {
    let split = tokens.iter().position(|t| *t == Token::Equals).map_or(tokens.len(), |k| k - 1);
    let values =
        tokens[..split].iter().filter_map(word).map(value).collect::<Result<Vec<_>, _>>()?;
    Ok((values, key_values(&tokens[split..])?))
}

fn source_spec(tokens: &[Token]) -> Result<SourceSpec, String> {
    let mut spec = SourceSpec::default();
    let mut i = 0;
    // Numbers inside `NAME( ... )`.
    let args = |i: &mut usize| -> Result<Vec<f64>, String> {
        let mut out = Vec::new();
        if tokens.get(*i) == Some(&Token::Open) {
            *i += 1;
            while let Some(t) = tokens.get(*i) {
                *i += 1;
                match t {
                    Token::Close => return Ok(out),
                    Token::Word(w) => out.push(value(w)?),
                    _ => return Err("unexpected `=` in a source function".into()),
                }
            }
            return Err("missing `)`".into());
        }
        // Without parentheses the arguments run to the next keyword.
        while let Some(Token::Word(w)) = tokens.get(*i) {
            match value(w) {
                Ok(v) => out.push(v),
                Err(_) => break,
            }
            *i += 1;
        }
        Ok(out)
    };
    while i < tokens.len() {
        let Some(w) = word(&tokens[i]) else {
            return Err("unexpected punctuation".into());
        };
        let key = w.to_ascii_lowercase();
        i += 1;
        match key.as_str() {
            "dc" => {
                let v = tokens.get(i).and_then(word).ok_or("DC needs a value")?;
                spec.dc = Some(value(v)?);
                i += 1;
            }
            "ac" => {
                let mag = tokens.get(i).and_then(word).map(value).transpose()?.unwrap_or(1.0);
                i += 1;
                let phase = match tokens.get(i).and_then(word).map(value) {
                    Some(Ok(p)) => {
                        i += 1;
                        p
                    }
                    _ => 0.0,
                };
                spec.ac = Some((mag, phase));
            }
            "pulse" => {
                let a = args(&mut i)?;
                if a.len() < 2 {
                    return Err("PULSE needs at least V1 and V2".into());
                }
                let at = |k: usize| a.get(k).copied();
                spec.tran = Some(Waveform::Pulse {
                    v1: a[0],
                    v2: a[1],
                    td: at(2).unwrap_or(0.0),
                    tr: at(3),
                    tf: at(4),
                    pw: at(5),
                    per: at(6),
                });
            }
            "sin" => {
                let a = args(&mut i)?;
                if a.len() < 2 {
                    return Err("SIN needs at least VO and VA".into());
                }
                let at = |k: usize| a.get(k).copied();
                spec.tran = Some(Waveform::Sin {
                    vo: a[0],
                    va: a[1],
                    freq: at(2),
                    td: at(3).unwrap_or(0.0),
                    theta: at(4).unwrap_or(0.0),
                    phase: at(5).unwrap_or(0.0),
                });
            }
            "pwl" => {
                let a = args(&mut i)?;
                if a.len() < 2 || a.len() % 2 != 0 {
                    return Err("PWL needs time-value pairs".into());
                }
                let points: Vec<(f64, f64)> = a.chunks(2).map(|p| (p[0], p[1])).collect();
                if points.windows(2).any(|w| w[1].0 < w[0].0) {
                    return Err("PWL times must not decrease".into());
                }
                spec.tran = Some(Waveform::Pwl(points));
            }
            _ => match value(w) {
                // A bare number is the DC value.
                Ok(v) if spec.dc.is_none() => spec.dc = Some(v),
                _ => return Err(format!("unknown source keyword `{w}`")),
            },
        }
    }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_elements_models_and_sources() {
        let deck = parse(
            "* title\n\
             VVCLK IN 0 PULSE(0 1.8 0 5e-11 5e-11 {0.5/1e8-5e-11} {1/1e8})\n\
             V1 a 0 DC 1 AC 1 SIN(0 1 1k)\n\
             R1 a b 4.7k ; load\n\
             MU1.MN OUT IN 0 0 nmos_1 W=1e-6\n\
             + L=1.8e-7\n\
             .model nmos_1 NMOS(LEVEL=1 VTO=0.7 KP=110u)\n\
             .tran 1n 10n\n\
             .end\n\
             R9 x y 1",
        )
        .unwrap();
        assert_eq!(deck.cards.len(), 4);
        let pulse = deck.cards[0].source.as_ref().unwrap().tran.clone().unwrap();
        let Waveform::Pulse { pw, per, .. } = pulse else { panic!() };
        assert!((pw.unwrap() - 4.95e-9).abs() < 1e-18);
        assert_eq!(per, Some(1e-8));
        let v1 = deck.cards[1].source.as_ref().unwrap();
        assert_eq!((v1.dc, v1.ac), (Some(1.0), Some((1.0, 0.0))));
        assert_eq!(deck.cards[2].values, [4700.0]);
        let m = &deck.cards[3];
        assert_eq!(m.kind(), 'M');
        assert_eq!(m.model.as_deref(), Some("nmos_1"));
        assert_eq!(m.params["l"], 1.8e-7);
        assert_eq!(deck.models[0].kind, "nmos");
        assert_eq!(deck.models[0].params["kp"], 110e-6);
    }

    #[test]
    fn errors_name_the_line() {
        let e = parse("R1 a b\nJ1 d g s jx").unwrap_err();
        assert!(e.starts_with("line 1"), "{e}");
        assert!(parse("J1 d g s jx").unwrap_err().contains("not supported"));
    }
}

/// A model found in a model library file.
#[derive(Debug, Clone, PartialEq)]
pub enum Definition {
    /// `.subckt name pins... ... .ends`, with its full text.
    Subckt { name: String, pins: Vec<String>, text: String },
    /// `.model name kind (...)`, with its text joined onto one line.
    Model { name: String, kind: String, text: String },
}

/// The top-level `.subckt` and `.model` definitions in a model library, such as a
/// manufacturer's `.lib` file, with comments removed and continuation lines joined.
pub fn definitions(text: &str) -> Vec<Definition> {
    let lines = logical_lines(text);
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let first = first_word(&lines[i].1);
        if first == ".model" {
            let tokens = tokenize(&lines[i].1);
            let words: Vec<&str> = tokens.iter().filter_map(word).collect();
            if let (Some(name), Some(kind)) = (words.get(1), words.get(2)) {
                out.push(Definition::Model {
                    name: (*name).to_owned(),
                    kind: kind.to_ascii_lowercase(),
                    text: lines[i].1.clone(),
                });
            }
        } else if first == ".subckt" {
            let mut depth = 0;
            let mut end = lines.len() - 1;
            for (k, (_, t)) in lines.iter().enumerate().skip(i) {
                match first_word(t).as_str() {
                    ".subckt" => depth += 1,
                    ".ends" => {
                        depth -= 1;
                        if depth == 0 {
                            end = k;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let tokens = tokenize(&lines[i].1);
            let name = tokens.get(1).and_then(word).unwrap_or_default().to_owned();
            let pins =
                split_params(&tokens[2.min(tokens.len())..]).map(|s| s.0).unwrap_or_default();
            let text: Vec<&str> = lines[i..=end].iter().map(|(_, t)| t.as_str()).collect();
            out.push(Definition::Subckt { name, pins, text: text.join("\n") });
            i = end;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod library_tests {
    use super::*;

    #[test]
    fn finds_models_and_subcircuits_in_a_library() {
        let lib = "* Vendor library\n\
                   .model D1N4148 D(IS=2.52n\n+ N=1.752) ; comment\n\
                   .SUBCKT OPA 1 2 3 4 5 PARAMS: GAIN=1\n\
                   .subckt inner a b\nR1 a b 1k\n.ends\n\
                   X1 1 2 inner\n.ENDS OPA\n";
        let defs = definitions(lib);
        assert_eq!(defs.len(), 2);
        assert!(
            matches!(&defs[0], Definition::Model { name, kind, text } if name == "D1N4148" && kind == "d" && text.contains("N=1.752"))
        );
        let Definition::Subckt { name, pins, text } = &defs[1] else { panic!() };
        assert_eq!((name.as_str(), pins.len()), ("OPA", 5));
        assert!(text.contains(".ends\nX1"), "nested definition kept: {text}");
    }
}
