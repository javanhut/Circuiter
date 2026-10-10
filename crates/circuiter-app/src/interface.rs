//! Editing a cell's interface: its ports, parameters and the metadata shown with them.
//!
//! Port changes are carried into every view that names ports (schematic port flags,
//! symbol pins, SPICE templates). The workspace then brings a generated symbol back in
//! line with [`circuiter_model::builder::sync`].

use circuiter_model::CellDef;
use circuiter_model::cell::{ParamDef, Port, PortKind, validate_name};

pub fn set_description(def: &mut CellDef, text: &str) {
    def.cell.description = text.trim().to_owned();
}

/// `" /analog/ filters/"` → `"analog/filters"`.
pub fn clean_folder(folder: &str) -> String {
    folder.split('/').map(str::trim).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("/")
}

/// Sets the reference designator prefix: `R`, `U`, `IC`...
pub fn set_prefix(def: &mut CellDef, prefix: &str) -> Result<(), String> {
    let prefix = prefix.trim();
    if prefix.is_empty() || prefix.len() > 8 || !prefix.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err("use one to eight letters, like `R` or `U`".into());
    }
    def.cell.prefix = prefix.to_owned();
    Ok(())
}

/// Adds a port to the interface. On a cell with a schematic it still has to be placed
/// there with a port flag.
pub fn add_port(def: &mut CellDef, name: &str, kind: PortKind) -> Result<(), String> {
    validate_name(name)?;
    if def.cell.port(name).is_some() {
        return Err(format!("there is already a port `{name}`"));
    }
    def.cell.ports.push(Port::new(name, kind));
    Ok(())
}

pub fn rename_port(def: &mut CellDef, old: &str, new: &str) -> Result<(), String> {
    validate_name(new)?;
    if old == new {
        return Ok(());
    }
    if def.cell.port(new).is_some() {
        return Err(format!("there is already a port `{new}`"));
    }
    let port = def.cell.ports.iter_mut().find(|p| p.name == old).ok_or("no such port")?;
    port.name = new.to_owned();
    if let Some(s) = def.schematic.as_mut() {
        s.ports.iter_mut().filter(|p| p.port == old).for_each(|p| p.port = new.to_owned());
    }
    if let Some(s) = def.symbol.as_mut() {
        s.pins.iter_mut().filter(|p| p.port == old).for_each(|p| p.port = new.to_owned());
    }
    if let Some(spice) = def.spice.as_mut() {
        let (from, to) = (format!("${{node.{old}}}"), format!("${{node.{new}}}"));
        for text in std::iter::once(&mut spice.template).chain(&mut spice.models) {
            *text = text.replace(&from, &to);
        }
    }
    Ok(())
}

pub fn set_port_kind(def: &mut CellDef, name: &str, kind: PortKind) {
    if let Some(port) = def.cell.ports.iter_mut().find(|p| p.name == name) {
        port.kind = kind;
    }
}

/// Removes a port from the interface, with its port flags and pin.
pub fn remove_port(def: &mut CellDef, name: &str) {
    def.cell.ports.retain(|p| p.name != name);
    if let Some(s) = def.schematic.as_mut() {
        s.ports.retain(|p| p.port != name);
    }
    if let Some(s) = def.symbol.as_mut() {
        s.pins.retain(|p| p.port != name);
    }
}

/// Moves port `index` one place earlier (`up`) or later. Port order is the order of a
/// SPICE subcircuit's nodes and of pins on a generated symbol.
pub fn move_port(def: &mut CellDef, index: usize, up: bool) {
    let ports = &mut def.cell.ports;
    let other = if up { index.checked_sub(1) } else { Some(index + 1) };
    if let Some(other) = other.filter(|&o| o < ports.len() && index < ports.len()) {
        ports.swap(index, other);
    }
}

pub fn add_param(def: &mut CellDef, param: ParamDef) -> Result<(), String> {
    validate_name(&param.name)?;
    if def.cell.param(&param.name).is_some() {
        return Err(format!("there is already a parameter `{}`", param.name));
    }
    if let Some(default) = &param.default {
        param.kind.validate(default)?;
    }
    def.cell.params.push(param);
    Ok(())
}

pub fn remove_param(def: &mut CellDef, name: &str) {
    def.cell.params.retain(|p| p.name != name);
}

/// Sets a parameter's default value; `None` makes the parameter required.
pub fn set_param_default(
    def: &mut CellDef,
    name: &str,
    default: Option<String>,
) -> Result<(), String> {
    let param = def.cell.params.iter_mut().find(|p| p.name == name).ok_or("no such parameter")?;
    if let Some(value) = &default {
        param.kind.validate(value)?;
    }
    param.default = default;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use circuiter_model::cell::ParamKind;
    use circuiter_model::schematic::{Schematic, SchematicPort};
    use circuiter_model::spice::SpiceView;
    use circuiter_model::{Point, builder};

    fn def() -> CellDef {
        let mut def = CellDef { name: "buf".into(), ..CellDef::default() };
        add_port(&mut def, "a", PortKind::Input).unwrap();
        add_port(&mut def, "y", PortKind::Output).unwrap();
        def.symbol = Some(builder::generate("buf", &def.cell));
        def.schematic = Some(Schematic {
            ports: vec![SchematicPort { port: "a".into(), at: Point::ORIGIN }],
            ..Schematic::default()
        });
        def.spice =
            Some(SpiceView { template: "X${inst} ${node.a} ${node.y} buf".into(), models: vec![] });
        def
    }

    #[test]
    fn renaming_a_port_updates_every_view() {
        let mut d = def();
        assert!(rename_port(&mut d, "a", "y").is_err(), "taken");
        assert!(rename_port(&mut d, "a", "1x").is_err(), "invalid");
        rename_port(&mut d, "a", "din").unwrap();
        assert!(d.cell.port("din").is_some());
        assert_eq!(d.schematic.as_ref().unwrap().ports[0].port, "din");
        assert!(d.symbol.as_ref().unwrap().pin("din").is_some());
        assert_eq!(d.spice.as_ref().unwrap().template, "X${inst} ${node.din} ${node.y} buf");
    }

    #[test]
    fn removing_and_reordering_ports() {
        let mut d = def();
        move_port(&mut d, 1, true);
        assert_eq!(d.cell.ports[0].name, "y");
        move_port(&mut d, 0, true);
        move_port(&mut d, 1, false);
        assert_eq!(d.cell.ports[0].name, "y", "out-of-range moves do nothing");
        remove_port(&mut d, "a");
        assert_eq!(d.cell.ports.len(), 1);
        assert!(d.schematic.as_ref().unwrap().ports.is_empty());
        assert!(d.symbol.as_ref().unwrap().pin("a").is_none());
    }

    #[test]
    fn parameters_validate_their_defaults() {
        let mut d = def();
        let real = |default: &str| ParamDef {
            name: "R".into(),
            kind: ParamKind::Real { unit: "Ω".into() },
            default: Some(default.into()),
            description: String::new(),
        };
        assert!(add_param(&mut d, real("lots")).is_err());
        add_param(&mut d, real("4k7")).unwrap();
        assert!(add_param(&mut d, real("1k")).is_err(), "duplicate");
        assert!(set_param_default(&mut d, "R", Some("x".into())).is_err());
        set_param_default(&mut d, "R", None).unwrap();
        assert_eq!(d.cell.params[0].default, None);
        assert!(set_prefix(&mut d, "U2").is_err());
        set_prefix(&mut d, "IC").unwrap();
    }
}
