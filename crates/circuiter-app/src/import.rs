//! Importing manufacturer SPICE models as project cells.
//!
//! A `.subckt` becomes a cell whose ports are its pins, with a generated symbol. A
//! `.model` for a diode or transistor becomes a cell that looks like the matching
//! standard part. Either way the model text is kept verbatim (renamed to `${model}`), and
//! the simulator tries it at once so unsupported models are reported on import.

use circuiter_model::builder;
use circuiter_model::cell::{Cell, ParamDef, ParamKind, Port, PortKind, validate_name};
use circuiter_model::spice::SpiceView;
use circuiter_model::{CellDef, CellId, Design, STD_LIBRARY};
use circuiter_sim::number::parse_number;
use circuiter_sim::parse::Definition;

/// The project folder imported models are listed under.
pub const IMPORTED: &str = "imported";

/// What importing one definition produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub def: CellDef,
    /// Why the simulator can't run it yet, if it can't.
    pub warning: Option<String>,
}

/// The name a definition is known by in its file.
pub fn definition_name(d: &Definition) -> &str {
    match d {
        Definition::Subckt { name, .. } | Definition::Model { name, .. } => name,
    }
}

/// A short description of a definition for listing: `sub-circuit, 5 pins`, `NPN model`.
pub fn describe(d: &Definition) -> String {
    match d {
        Definition::Subckt { pins, .. } => format!("sub-circuit, {} pins", pins.len()),
        Definition::Model { kind, .. } => format!("{} model", kind.to_ascii_uppercase()),
    }
}

/// A valid cell or port name made from `raw`: other characters become `_`, and a name
/// starting with a digit gets `prefix` in front.
fn clean(raw: &str, prefix: &str) -> String {
    let mut s: String =
        raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if !s.starts_with(|c: char| c.is_ascii_alphabetic()) {
        s.insert_str(0, prefix);
    }
    s
}

/// Turns one definition into a cell named so it doesn't clash with `taken`.
pub fn import(design: &Design, d: &Definition, taken: &[String]) -> Result<Imported, String> {
    let base = clean(definition_name(d), "m");
    let mut name = base.clone();
    let mut k = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
        name = format!("{base}_{k}");
        k += 1;
    }
    let (def, trial) = match d {
        Definition::Subckt { name: sub, pins, text } => subcircuit(&name, sub, pins, text)?,
        Definition::Model { name: model, kind, text } => device(design, &name, model, kind, text)?,
    };
    // Try it in the simulator: one instance, every pin on its own node.
    let warning = circuiter_sim::Circuit::parse(&trial).err();
    Ok(Imported { def, warning })
}

fn subcircuit(
    name: &str,
    sub: &str,
    pins: &[String],
    text: &str,
) -> Result<(CellDef, String), String> {
    let mut ports: Vec<String> = Vec::new();
    for pin in pins {
        let mut p = clean(pin, "pin");
        if ports.iter().any(|q| q.eq_ignore_ascii_case(&p)) {
            p = format!("pin{}", ports.len() + 1);
        }
        ports.push(p);
    }
    // Numeric parameters with defaults become cell parameters.
    let header = text.lines().next().unwrap_or_default();
    let mut params = Vec::new();
    if let Some(at) = header.to_ascii_lowercase().find("params:") {
        for pair in header[at + 7..].split_whitespace() {
            if let Some((k, v)) = pair.split_once('=') {
                if validate_name(k).is_ok() && parse_number(v).is_some() {
                    params.push(ParamDef {
                        name: k.to_owned(),
                        kind: ParamKind::Real { unit: String::new() },
                        default: Some(v.to_owned()),
                        description: format!("Sub-circuit parameter {k}"),
                    });
                }
            }
        }
    }
    let mut template = String::from("X${inst}");
    for p in &ports {
        template.push_str(&format!(" ${{node.{p}}}"));
    }
    template.push_str(" ${model}");
    if !params.is_empty() {
        template.push_str(" PARAMS:");
        for p in &params {
            template.push_str(&format!(" {0}=${{param.{0}}}", p.name));
        }
    }
    // The definition, renamed so each distinct parameter set gets its own copy.
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let mut header_words = lines[0].split_whitespace();
    let keyword = header_words.next().unwrap_or(".subckt");
    let _ = header_words.next();
    lines[0] = format!("{keyword} ${{model}} {}", header_words.collect::<Vec<_>>().join(" "));
    if let Some(last) = lines.last_mut() {
        if last.to_ascii_lowercase().starts_with(".ends") {
            *last = ".ends".to_owned();
        }
    }
    if text.contains("${") {
        return Err(format!("`{sub}` contains `${{`, which Circuiter uses for placeholders"));
    }
    let cell = Cell {
        description: format!("Imported sub-circuit {sub}"),
        prefix: "U".into(),
        ports: ports.iter().map(|p| Port::new(p, PortKind::Passive)).collect(),
        params,
        global_net: None,
        physics: None,
    };
    let symbol = builder::generate(name, &cell);
    let trial = format!(
        "X1 {} {sub}\n{text}",
        (0..ports.len()).map(|k| format!("t{k}")).collect::<Vec<_>>().join(" ")
    );
    let def = CellDef {
        name: name.to_owned(),
        folder: IMPORTED.to_owned(),
        cell,
        symbol: Some(symbol),
        schematic: None,
        spice: Some(SpiceView { template, models: vec![lines.join("\n")] }),
    };
    Ok((def, trial))
}

fn device(
    design: &Design,
    name: &str,
    model: &str,
    kind: &str,
    text: &str,
) -> Result<(CellDef, String), String> {
    let (base, template, trial_line) = match kind {
        "d" => ("diode", "D${inst} ${node.a} ${node.k} ${model}", format!("D1 t0 t1 {model}")),
        "npn" | "pnp" => (
            kind,
            "Q${inst} ${node.c} ${node.b} ${node.e} ${model}",
            format!("Q1 t0 t1 t2 {model}"),
        ),
        "nmos" | "pmos" => (
            kind,
            "M${inst} ${node.d} ${node.g} ${node.s} ${node.b} ${model} W=${param.w} L=${param.l}",
            format!("M1 t0 t1 t2 t3 {model}"),
        ),
        other => return Err(format!("`{other}` models can't be imported yet")),
    };
    let std = design
        .cell(&CellId::new(STD_LIBRARY, base))
        .ok_or_else(|| format!("std:{base} is missing"))?;
    let mut cell = std.cell.clone();
    cell.description = format!("Imported {} model {model}", kind.to_ascii_uppercase());
    // Only the geometry parameters remain; the model card holds the rest.
    cell.params.retain(|p| p.name == "w" || p.name == "l");
    // Rename the model to `${model}`: `.model NAME TYPE(...)`.
    let mut words = text.splitn(3, char::is_whitespace);
    let keyword = words.next().unwrap_or(".model");
    let _ = words.next();
    let card = format!("{keyword} ${{model}} {}", words.next().unwrap_or_default());
    let def = CellDef {
        name: name.to_owned(),
        folder: IMPORTED.to_owned(),
        cell,
        symbol: std.symbol.clone(),
        schematic: None,
        spice: Some(SpiceView { template: template.to_owned(), models: vec![card] }),
    };
    Ok((def, format!("{trial_line}\n{text}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use circuiter_sim::parse::definitions;

    fn design() -> Design {
        let mut d = Design::new();
        d.add_library(circuiter_stdlib::load().unwrap()).unwrap();
        d
    }

    const LIB: &str = "\
* A vendor library
.model 1N4001 D(IS=29.5n RS=73.5m N=1.96 BV=50 IBV=10u CJO=34p)
.model Q2N2222 NPN(IS=14.34f BF=255.9 VAF=74.03)
.subckt LM_TEST in+ in- out params: gain=1e5
E1 out 0 in+ in- {gain}
.ends LM_TEST
.subckt Weird 1 2
B1 1 2 V=1
.ends";

    #[test]
    fn imports_subcircuits_and_device_models() {
        let d = design();
        let defs = definitions(LIB);
        assert_eq!(defs.len(), 4);
        let diode = import(&d, &defs[0], &[]).unwrap();
        assert_eq!(diode.def.name, "m1N4001");
        assert_eq!(diode.def.folder, IMPORTED);
        assert_eq!(diode.warning, None);
        assert!(diode.def.spice.as_ref().unwrap().models[0].starts_with(".model ${model} D("));

        let bjt = import(&d, &defs[1], &[]).unwrap();
        assert_eq!(bjt.def.cell.ports.len(), 3);
        assert!(bjt.def.symbol.is_some());

        let amp = import(&d, &defs[2], &["LM_TEST".into()]).unwrap();
        assert_eq!(amp.def.name, "LM_TEST_2", "renamed away from an existing cell");
        let ports: Vec<&str> = amp.def.cell.ports.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(ports, ["in_", "pin2", "out"]);
        assert_eq!(amp.def.cell.params[0].name, "gain");
        let spice = amp.def.spice.as_ref().unwrap();
        assert_eq!(
            spice.template,
            "X${inst} ${node.in_} ${node.pin2} ${node.out} ${model} PARAMS: gain=${param.gain}"
        );
        assert!(
            spice.models[0].starts_with(".SUBCKT ${model} in+ in- out")
                || spice.models[0].starts_with(".subckt ${model} in+ in- out")
        );
        assert!(builder::is_generated(&amp.def));
        assert_eq!(amp.warning, None);

        let weird = import(&d, &defs[3], &[]).unwrap();
        assert_eq!(weird.def.cell.ports[0].name, "pin1");
        assert!(weird.warning.as_deref().is_some_and(|w| w.contains("not supported")));
    }

    #[test]
    fn imported_parts_simulate_through_the_netlister() {
        let d = design();
        let defs = definitions(LIB);
        let mut lib = circuiter_model::Library::new("p");
        let amp = import(&d, &defs[2], &[]).unwrap().def;
        lib.insert(amp);
        let mut design = d;
        design.add_library(lib).unwrap();
        // A netlist with the imported part: its model text is emitted once, renamed.
        let id = CellId::new("p", "LM_TEST");
        let def = design.cell(&id).unwrap().clone();
        let spice = def.spice.unwrap();
        let expanded = spice.models[0].replace("${model}", "lm_test_1");
        let deck =
            format!("V1 a 0 DC 1u\nX1 a 0 b lm_test_1 PARAMS: gain=1000\nRL b 0 1k\n{expanded}");
        let c = circuiter_sim::Circuit::parse(&deck).unwrap();
        let r = circuiter_sim::op(&c, &circuiter_sim::Options::default()).unwrap();
        assert!((r.voltage(&c, "b").unwrap() - 1e-3).abs() < 1e-9, "gain 1000 from the parameter");
    }
}
