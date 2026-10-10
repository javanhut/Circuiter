# Circuiter

Design hardware and its software in one place, from a single transistor to a finished
product.

Circuiter aims to be a one-stop tool for electronics: design components hierarchically
(a MOSFET, then an inverter, then a flip-flop, up to a CPU), simulate them with real
physics, including parts that heat up and fail when misused, run firmware on simulated
RISC-V cores, lay out boards, put them in a 3D enclosure, and export everything needed
to manufacture it.

> **Status:** early development. Milestone 1 ("circuit lab") is in progress. What exists
> today is the core design model, the standard component library, and the `circuiter`
> desktop app: an editor for schematics, symbols and component interfaces, with a
> built-in circuit simulator (operating point, DC sweep, transient, AC) and a physics
> engine in which overstressed parts heat up and fail. A virtual bench is next. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for
> the design and roadmap.

## Building

Requires Rust 1.85 or newer.

```sh
cargo build --release
cargo test --workspace
```

## Trying it

Circuiter is a single desktop app, `circuiter`:

```sh
cargo run --release -p circuiter-app -- examples/cmos_inverter
cargo run --release -p circuiter-app -- examples/ce_amplifier
```

The examples are a CMOS inverter built from two MOSFETs and driven by a clock, a
one-transistor audio amplifier (a 2N3904 common-emitter stage, about 44 dB of gain)
with a power LED, a half adder built from logic gates, and an overstress lab where an
LED, a fuse and a resistor are pushed
past their limits.

Start your own with File → New Project… (or the welcome screen): pick an empty folder
and the project is named after it.

**The window**

- Left: **Project** and **Libraries** trees (folders and cells), and below them
  **Components**: every part by kind, with a search box. Click a part, then the sheet;
  or drag it onto the sheet.
- Middle: open cells as **tabs**, each remembering where it was left. Above the sheet,
  switch between **Schematic**, **Symbol** and **Netlist** (the SPICE deck the
  simulator runs), and pick drawing tools and zoom. The column of common parts beside
  the sheet works like Components.
- Right: **Properties** of what's selected (or the cell), **Simulation** (run, results,
  plotted signals) and **Physics** (ratings, temperatures, what failed).
- Bottom: **Waveforms**, **Thermal** and **Problems**. Hide it with its ×; Run brings
  it back.
- Help → Keyboard Shortcuts lists every key.

**Viewing**

| Action | How |
|---|---|
| Zoom | Mouse wheel (about the cursor), or pinch |
| Pan | Middle or right drag, or Space + left drag |
| Fit to window | `F` or `Home` |
| Open a sub-circuit | Double-click it, or use the inspector's "Open schematic" |
| Go up a level | `Alt+↑`, or click the path above the sheet |
| Back, forward | `Alt+←`, `Alt+→`, the mouse's back and forward buttons, or the arrows in the toolbar. Looking at a library part's symbol, "Back to …" returns to your sheet |
| Tabs | Opening a cell gives it a tab (or goes to its tab). Middle-click or × closes one |
| Reload from disk | `F5` |
| Open a project | `Ctrl+O` |

**Editing** (any sheet in your project; the standard library is read-only)

| Action | How |
|---|---|
| Add a part | Drag it from Components or the parts column onto the sheet; or click it there, then click the sheet; or `A` / `Ctrl+K` and search (by name, folder or description). After one click you're back to selecting; Shift+click places several. `G` places a ground, `Shift+V` a VDD symbol. `R` rotates, `X` mirrors, `Esc` cancels |
| Draw a wire | Drag out of a pin's end (it's marked when the pointer is over it) and let go on another pin, or click the pin's end and click onwards. Or `W`, then click. Wires route at right angles (`/` flips the corner) and finish when they reach a pin or wire, or on double-click. A wire connects to a pin only at the wire's end or a corner: running past a pin doesn't connect it (Problems points it out). The status bar says what each edit connected or disconnected |
| Net label | `L`, type a name, click on a wire. Same-named labels are connected |
| Port | `P`, name and direction, then click. Ports are how a cell connects to the sheets that use it |
| Select | Click; Shift+click adds; drag across empty space to box-select. Selecting a wire highlights its whole net |
| Move | Drag a part or a wire (whatever tool is active). Wires stay attached, re-routing at right angles around the part; whatever joins a wire part-way (a T, a label) stays joined |
| Rotate, mirror, delete | `R`, `X`, `Delete` or `Backspace` |
| Copy, cut, paste, duplicate | `Ctrl+C`, `Ctrl+X`, `Ctrl+V` (at the pointer), `Ctrl+D`. Parts come with the wires between them and get fresh names; paste works across sheets and between Circuiter windows |
| Edit values | Change the name or any parameter in the inspector (`4k7`, `100n`, `2u`…); invalid values are refused. ↺ resets to the default |
| Undo, redo | `Ctrl+Z`, `Ctrl+Y` (per sheet) |
| New cell | `Ctrl+N`: an empty schematic to build a new component in, optionally in a folder |
| Folders | Cells sit in folders, as deep as you like (`cpu/alu/adders`). Right-click a folder (or the project) for New Folder, New Cell Here, Rename and Delete (when empty); drag a cell onto a folder to move it, or set its folder in Properties. Folders stay until deleted, even empty, and on disk they're ordinary directories. The standard library is sorted the same way (`gates/`, `resistors/`, `transistors/`…); imported models go in `imported/` |
| Save | `Ctrl+S`. The title bar shows ● while there are unsaved changes, and you're asked before anything would discard them |

**Building a component**

1. `Ctrl+N` makes a new cell. Draw its circuit, and place a port (`P`) for each
   connection it should have.
2. As soon as it has ports, a symbol is generated: inputs on the left, outputs on the
   right, supplies on top, grounds at the bottom. The cell can now be placed on other
   sheets (`+` in the library panel).
3. With nothing selected, the inspector edits the interface: description, reference
   prefix, ports (rename, change direction, reorder, remove) and parameters. A
   generated symbol follows every change.
4. To draw the symbol yourself, switch to the symbol view (`V`, or the
   Schematic/Symbol switch beside the path) and edit it. Once you do, your drawing is
   kept; new ports still get pins, which you can then move.

**Simulating**

Circuiter has its own simulator, written in Rust and built into the app, so there is
nothing to install. It speaks SPICE and is checked against ngspice.

| Action | How |
|---|---|
| Run | **▶ Run** (`Ctrl+R`) switches the circuit on (everything starts off, as with a real power switch) and watches it for a sensible time: three cycles of the slowest clock or signal, or 10 ms. Waveforms appear below the sheet, each wire is tagged with its final voltage, and overheated parts are marked |
| Operating point | `F9`, or Analyses → Operating Point. Every net is tagged with its voltage; select a part to see the current into each pin and the power it dissipates (or the total for a sub-circuit) |
| Transient | `F10`, or Analyses → Transient…: the stop time and resolution, and whether to start from power-on or from the steady state (SPICE's usual start) |
| AC sweep | Analyses → AC Sweep…: gain and phase over frequency (a Bode plot), driven by each source's AC value; a sine source's `ac` parameter is 1 V by default |
| Long runs | Simulations run in the background; the status bar shows progress and `Esc` cancels |
| Manufacturer models | File → Import SPICE Models… reads a vendor `.lib`/`.mod` file: each `.subckt` becomes a part with a generated symbol (rename its pins in the inspector), each diode or transistor `.model` a part drawn like the standard one. Models the simulator can't run yet are flagged on import |
| Plots | Select a wire, then Add to Plot; `✖` on a name removes it. Wheel zooms, drag pans, double-click shows everything; hover for values |
| Problems | No ground, nets with no DC path to ground and shorted voltage sources are reported under Problems. A circuit without ground still simulates, as a real one would, against board leakage |

Results go stale when you edit the circuit; run the analysis again to refresh them.

**Logic gates**

`std:not`, `and`, `or`, `nand`, `nor`, `xor` and `xnor` are real CMOS gates (74HC-like,
2 to 6 V) with the usual ANSI shapes. Their supply pins are hidden: they connect to the
VDD and GND nets wherever the gate is placed, so place a VDD symbol (`std:vdd`) on a
voltage source's positive terminal, or label that net `VDD`. Several waveforms are drawn
in lanes, as a logic analyser shows them.

**Heat and failure**

Parts have ratings and a thermal model, as real ones do: a resistor's power rating,
a capacitor's voltage rating, an LED's current, a fuse's melting energy, a
transistor's junction temperature, a CMOS gate oxide's breakdown voltage.

- An operating point lists every part it would destroy and how soon: "R3 would fail
  in about 1.858s: 1.44W in it would take it to 774 °C (destroyed at 250 °C)".
- A transient run heats every part second by second. When one is destroyed it fails
  as the real one would (a resistor burns open, a capacitor cracks shorted, an LED
  or fuse blows open), the simulation carries on with the damaged circuit, and the
  sheet marks what happened. Problems lists each failure with its cause.
- Select a part for its temperature, peak and limit; switch the plot to
  Temperatures to watch everything heat (and cool once destroyed).

**Symbol view**

| Action | How |
|---|---|
| Switch view | `V`, or Schematic/Symbol beside the path |
| Line | `L`, click points; click the start to close it, double-click to end |
| Rectangle, circle, arc | `B`, `C`, `A`, then click: corners; centre and radius; centre, start and end (counter-clockwise) |
| Text | `T`. `${name}` shows the reference (`U1`), `${param.X}` a parameter's value |
| Pin | `P` places a pin for each port that lacks one (`R` turns it) |
| Move, rotate, mirror, delete | Drag; `R`, `X`, `Delete`. Pins stay on the 100-mil grid, drawings snap to 50 mil |
| Pin and drawing properties | Select one: side, length, name display; fill, radius, angles, text, size, alignment |
| Regenerate | Generate (toolbar or inspector) replaces the drawing with a box from the ports; `Ctrl+Z` undoes it |

The left panel browses libraries (click a part to see its symbol). The right panel
inspects the selection: parameters, pin-to-net connections, and the problems on the
current sheet, such as unconnected pins.

## Project layout

| Path | Contents |
|---|---|
| `crates/circuiter-model` | Design data model: cells, views, libraries, projects, checks |
| `crates/circuiter-stdlib` | Built-in `std` component library (`library/` holds the cell files) |
| `crates/circuiter-scene` | Schematic display lists, colour themes, SVG export |
| `crates/circuiter-app` | The `circuiter` desktop app (egui on wgpu), the only binary |
| `examples/` | Example projects |
| `docs/` | Architecture and design notes |

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in Circuiter by you, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.
