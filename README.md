# Circuiter

Design hardware and its software in one place, from a single transistor to a finished
product.

Circuiter aims to be a one-stop tool for electronics: design components hierarchically
(a MOSFET, then an inverter, then a flip-flop, up to a CPU), simulate them with real
physics, including parts that heat up and fail when misused, run firmware on simulated
RISC-V cores, lay out boards, put them in a 3D enclosure, and export everything needed
to manufacture it.

> **Status:** early development. Milestone 1 ("circuit lab") is in progress. What exists
> today is the core design model, the standard component library, a command-line
> interface, and a desktop app that opens projects and lets you browse and inspect
> schematics (editing comes next). See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for
> the design and roadmap.

## Building

Requires Rust 1.85 or newer.

```sh
cargo build --release
cargo test --workspace
```

## Trying it

### Desktop app

```sh
cargo run --release -p circuiter-app -- examples/cmos_inverter
```

| Action | How |
|---|---|
| Zoom | Mouse wheel (about the cursor), or pinch |
| Pan | Middle or right drag, or Space + left drag |
| Fit to window | `F` or `Home` |
| Select | Click a part, wire, label or port. Selecting a wire highlights its whole net |
| Open a sub-circuit | Double-click it, or use the inspector's "Open schematic" |
| Go up a level | `Backspace`, or click a breadcrumb |
| Reload from disk | `F5` |
| Open a project | `Ctrl+O` |

The left panel browses libraries (click a part to preview its symbol). The right panel
inspects the selection: parameters, pin-to-net connections, and the problems on the
current sheet, such as unconnected pins.

### Command line

```sh
# Browse the standard library
circuiter lib list std
circuiter lib show std:nmos

# Check and explore the example: a CMOS inverter built from two MOSFETs
circuiter -C examples/cmos_inverter check
circuiter -C examples/cmos_inverter tree

# Connectivity, and an SVG of a schematic
circuiter -C examples/cmos_inverter nets inverter
circuiter -C examples/cmos_inverter render inverter -o inverter.svg --theme bench

# Start a project of your own
circuiter new my_amp
```

During development, use `cargo run -p circuiter-cli --` in place of `circuiter`.

## Project layout

| Path | Contents |
|---|---|
| `crates/circuiter-model` | Design data model: cells, views, libraries, projects, checks |
| `crates/circuiter-stdlib` | Built-in `std` component library (`library/` holds the cell files) |
| `crates/circuiter-scene` | Schematic display lists, colour themes, SVG export |
| `crates/circuiter-app` | The desktop app (`circuiter-gui`), egui on wgpu |
| `crates/circuiter-cli` | The `circuiter` command-line tool |
| `examples/` | Example projects |
| `docs/` | Architecture and design notes |

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in Circuiter by you, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.
