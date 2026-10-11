# Circuiter architecture

This document records what Circuiter is, the decisions behind its design, and the
roadmap. Update it whenever a decision changes.

## Vision

Circuiter is a single program for designing a complete hardware product:

- **Components** built from scratch and hierarchically, from primitives (a MOSFET, a
  transformer) to gates, flip-flops and up to a full CPU. Each one is wrapped as a
  reusable component that behaves like the circuit inside it.
- **Simulation with real physics.** Sources (DC, AC sine, pulse, clock), analog and
  digital behaviour, and consequences: a circuit with no ground return doesn't work,
  and an overstressed part heats up and fails.
- **Firmware and logic**: Embedded C and assembly on simulated RISC-V cores, and
  SystemVerilog for custom logic, including custom RISC-V cores.
- **Physical design**: schematics, PCBs up to motherboard scale, wiring diagrams,
  a 3D renderer, enclosures and 3D-printable parts.
- **Manufacturing**: fabrication files, BOMs, and parts ordering.

The quality bar is that an electrical engineer should recognise what they see:
standard symbols (ANSI/IEEE 315 by default, IEC 60617 as an option), real instruments,
and behaviour that matches the bench.

## Principles

1. **One design model.** Schematic, simulation, PCB, 3D and manufacturing are views of
   one model, so firmware on a simulated core can drive an LED on a board inside an
   enclosure.
2. **Own the core, nothing to install.** The single binary must run without system
   packages, so engines are pure Rust: Circuiter's own analog simulator (SPICE
   compatible, validated against ngspice), the data model, the RISC-V
   instruction-set simulator, the co-simulation scheduler, and the physics and failure
   engine. Pure-Rust libraries are used where they are mature (e.g. linear algebra).
   Engines that need a system toolchain at run time (Verilator needs a C++ compiler)
   are revisited when their milestone comes.
3. **Built to last.** Each milestone ships at production quality. Simulation results
   are validated against analytical solutions and datasheet curves.
4. **Headless first, one binary.** Every capability lives in a library crate, or in the
   app's GUI-independent modules (`edit`, `workspace`...), before it reaches the
   screen, which keeps it testable. The only binary is the desktop app, `circuiter`.
   There is no separate command-line tool.

## Design model

### Sheet connectivity

`circuiter_model::connectivity` decides what is joined on a sheet, following common
schematic conventions:

- Items at the same point connect.
- A wire vertex, label or port on a wire's segment connects to that wire (a T).
- A pin connects to a wire only at the wire's ends or corners. A wire passing over a
  pin does not connect to it, and is reported, so routing past a part never shorts it.
- Ports may be hidden and joined to a global net (logic gates' VDD and GND pins); a
  label named like a global net joins it too.
- Wires that cross without a vertex do not connect.
- Same-named labels connect, and global symbols such as ground connect across the sheet.
- Junction dots appear where three or more connections meet.

The GUI, the `nets` command and (next) the netlister all use it.

### Cells and views

The unit of reuse is the **cell**: a component with a fixed interface (ports and
parameters) and any number of **views** that implement it.

| View | Purpose | Status |
|---|---|---|
| `symbol` | Schematic graphic and pin positions | done |
| `schematic` | Implementation from instances of other cells | done |
| `spice` | SPICE element or subcircuit template | done |
| `digital` | Event-driven gate/behavioural model (XSPICE, characterized) | M1 |
| `hdl` | SystemVerilog module, run through Verilator | M2 |
| `footprint`, `model3d` | PCB land pattern and 3D body | M3 |

A cell's schematic can instantiate other cells. A transistor-level inverter, a D
flip-flop built from inverters, and a CPU built from flip-flops are all just cells.

**The component builder** (`circuiter_model::builder`) turns an interface into a box
symbol following IEEE 315 block conventions: inputs on the left, outputs and
bidirectional pins on the right, supplies on top, grounds (by name: `gnd`, `vss`,
`vee`...) at the bottom, passive pins shared between left and right. Every pin sits on
the 100-mil grid, and the body is sized so facing pin names never collide. A generated
symbol is marked `generated: true` and is regenerated whenever the ports change, so a
new cell becomes a placeable part as soon as it has ports and stays in step with them.
Editing the drawing clears the mark. From then on the drawing is kept, pins of removed
ports are dropped and new ports get a pin on their usual side. The mark is explicit
rather than inferred by comparison, so improving the generator never turns existing
generated symbols into custom ones.

**View selection** decides how each instance is elaborated. The design has a view
order (default: schematic, then spice), and an instance can force a view. This is how
mixed fidelity works: simulate one gate at transistor level while the rest of the CPU
runs on fast digital models.

### Libraries and projects

- A **library** is a directory of cells (`cells/<folder>/<name>/{cell,symbol,schematic,spice}.ron`).
  A directory holding a `cell.ron` is a cell, any other is a folder, and folders nest
  (`analog/filters`). Folders only organise: cell names are unique within a library and
  references don't mention folders, so moving a cell breaks nothing.
- The built-in `std` library ships inside the binary and uses the same format, one
  folder per kind of part: `gates/`, `resistors/`, `capacitors/`, `inductors/`,
  `diodes/`, `transistors/`, `sources/`, `power/`, `protection/`.
- A directory without a `cell.ron` is a folder, kept even when empty; a library also
  records folders in its own right (`Library::folders`), so an empty folder made in the
  app survives a save.
- A **project** is a directory with `circuiter.toml` plus its own library, named after
  the project. It can reference other libraries by path.
- References are written `library:cell`, or just `cell` for the same library.

### File format

- Design files are RON, with a canonical layout: a bracketed group stays on one line if
  it fits in 100 columns, otherwise it breaks one item per line. Fields equal to their
  default are omitted.
- Saving an unchanged design produces byte-identical files, so diffs stay minimal in
  any version control system. `circuiter fmt` rewrites a project canonically, and tests
  enforce canonical form for the standard library and examples.
- Comments are not preserved; files are machine-written.
- Coordinates are integer nanometres in memory and millimetres on disk. The schematic
  grid is 100 mil (2.54 mm).
- Quantities use SI prefixes: `m` is milli, `M` (or `meg`) is mega, and RKM notation
  like `4k7` is accepted. The netlister converts values to plain numbers for SPICE,
  which reads `M` as milli.

## Simulation architecture (Milestone 1 onward)

```
schematic hierarchy ──► netlister ──► flat netlist ──┬─► circuiter-sim (analog)
                       (view selection,              └─► circuiter-logic (digital)
                        ERC, analog and                       ▲ parts: built-in models,
                        digital halves)                       │ Verilog modules (circuiter-hdl),
                                                              │ memories loaded with programs
                                                              │ (circuiter-riscv: asm, C, Rust)
                     co-simulation (app/cosim.rs): one timeline, converters at the boundary
                                                          ▲
                              physics engine: power ► thermal RC ► stress ► failure
                                                          ▲
                     live sessions (app/live.rs): a run that goes on, operated as it runs
```

- **Digital simulation** (done): a design whose every part has a digital view (or a
  schematic of such parts) is flattened by `Netlist::build_digital` and runs in
  `circuiter-logic`: event-driven, four-state, buses up to 64 bits, delays per part,
  delta cycles for zero-delay logic, GND and VDD as constant 0 and 1. Port widths may
  come from instance parameters (`width_of`). Memories load programs named by a
  parameter: assembled, compiled with the system's clang or cargo, or read from ELF.

- **Mixed simulation** (done): `Netlist::build_mixed` splits a design into an analog
  half (SPICE deck) and a digital half; an instance's view comes from its "Simulate
  as" choice, else digital where a digital view exists. `cosim::Cosim` runs the logic
  simulator as an observer of the analog transient. Digital outputs on analog nets
  are voltage sources behind 50 Ω (opened to 1 GΩ for Z) whose levels are external
  inputs of the analog solver (`Options::inputs`); analog nets read by digital parts
  pass thresholds at 30 % and 70 % of VDD with hysteresis, the crossing time
  interpolated between steps and injected as an input event. The observer's
  `next_event` makes the analog solver land a step on each scheduled digital change
  of a bridged net, and `take_discontinuity` restarts integration after it.

- **Live sessions** (done): `live::Session` runs analog (`Transient::advance`, the
  resumable transient), digital or mixed designs on a thread, paced to a chosen speed
  (simulated seconds per real second), until stopped. Interactive parts are external
  inputs: a switch or pushbutton is a resistance (closed or open), a dial a source
  level, a potentiometer the two resistances of its wiper, a logic toggle or button
  an input part poked in the logic simulator. The UI shares voltages, logic values,
  meter readings, oscilloscope traces and the parts' states with the thread through
  `live::Shared`; symbols draw the state they are in (`Symbol::states`).

- **Instruments** (done): the multimeter and oscilloscope are cells marked `probe`,
  whose pins join wire interiors, so leads clip anywhere. The multimeter's SPICE
  template is chosen by its mode (`SpiceView::choose`): 10 MΩ across for volts, a
  0 V source in series for amps, a 1 mA source for ohms. The oscilloscope adds
  nothing to the circuit; its channels are read from the run (live or recorded).

- **Netlister**: flattens the hierarchy, derives nets from wire geometry, runs ERC
  (floating nets, no DC path to ground, shorted sources), inserts A/D bridges at
  analog/digital boundaries, and emits SPICE.
- **The simulator** (decided 2026-10-09, replacing the plan to embed ngspice) is
  Circuiter's own, in pure Rust, so the one binary needs nothing installed. It reads
  the SPICE deck the netlister emits, the same deck ngspice reads, which keeps vendor
  `.model`/`.subckt` files usable and makes ngspice a ready test oracle. Modified
  nodal analysis, Newton-Raphson with junction voltage limiting and gmin and source
  stepping for hard operating points, trapezoidal integration with truncation-error
  step control and source breakpoints. Owning the solver lets the physics engine and
  other domains step in lockstep with it, and lets failures swap elements mid-run.
- **Validation**: tests compare results with analytical solutions and, when ngspice is
  installed (developers only; it is never a dependency of the app), with ngspice on
  the same deck.
- **Characterization**: simulate a transistor-level cell once, extract delay, power and
  switching thresholds, and generate its fast digital model. The digital gate then
  behaves like the user's design, not a generic one.
- **Physics and failure engine**: cells carry absolute maximum ratings, safe operating
  areas, thermal parameters (junction-to-ambient resistance and capacitance) and failure
  modes. During a transient run:
  1. power dissipation drives each part's thermal network;
  2. temperature feeds back into temperature-dependent models;
  3. when a limit is exceeded the part fails (short, open, or degraded), its element is
     swapped for the failure mode, and simulation continues on the new topology.

  The UI shows heat maps, damage effects and a root-cause report.
- **Missing ground**: realistic leakage paths keep the solver running so the circuit
  behaves as a real one would, i.e. it doesn't work. The user gets a diagnosis instead
  of a "singular matrix" error.

## Crates

| Crate | Responsibility | Status |
|---|---|---|
| `circuiter-model` | Data model, file format, libraries, projects, hierarchy, structural checks | done |
| `circuiter-stdlib` | Built-in `std` library | done (grows each milestone) |
| `circuiter-scene` | Schematic → display list (shared by screen, SVG and print), themes, SVG export | done |
| `circuiter-app` | The one binary, `circuiter`. Desktop GUI (egui on wgpu): headless operations (`edit` for schematics, `symbol_edit`, `interface`), `workspace` (session, tools, undo), `canvas`, `style` (colours, icons, shared widgets), `app` with `chrome` (title bar, toolbar, tabs), `explorer` (trees, components), `panels` (properties, simulation, physics, results), `pcb` (the board editor), `viewer3d` and `body_editor` (3D, the board in its case, modelling) | schematic, symbol, board and body editors done |
| `circuiter-netlist` | Hierarchy flattening (for the analog and digital simulators, and the physical flattening a board is made from), net naming, SPICE emission with merged model cards, whole-circuit checks, and the electrical rules check (`erc`: KiCad's pin-type rules on the flattened circuit as it will run, sheet checks for labels and no-connect flags) | done |
| `circuiter-sim` | Pure-Rust SPICE-compatible simulator: deck parser (R, C, L, K, V, I, E, F, G, H incl. `POLY`, S, W switches, D, MOSFET Level 1 with Meyer capacitance, Gummel-Poon BJT; `.subckt` with parameters, `.param`, `{}` expressions with functions; DC, AC, PULSE, SIN, PWL sources; JFETs (Shichman-Hodges); behavioural `B` sources whose expressions of node voltages, branch currents and time are differentiated exactly for Newton), sparse LU (KLU-style: minimum-degree ordering, threshold pivoting, refactorisation), operating point (Newton, dynamic gmin and source stepping), DC sweep, transient (switch event location), AC; progress and cancellation; model-library scanning for import; later the co-simulation scheduler | done for Milestone 1's analyses |
| `circuiter-physics` | Per-part thermal model (exact RC update each step), instant limits (voltage, current, I²t), failure modes swapped into the running simulation, events with causes, operating-point assessment; an optional case: an air node warmed by every part and cooled through the case to the room, which is then each part's ambient | done (first version) |
| `circuiter-render` | Custom wgpu renderer for very large sheets and 3D | when egui's painter runs out |
| `circuiter-logic` | Digital simulator: four-state values up to 64 bits, event queue with delta cycles and propagation delays, net resolution of several drivers, history of every net; parts from gates to register files, memories and the RV32I control unit | done (first version) |
| `circuiter-riscv` | RV32I assembler (matches LLVM's encodings), ELF loader, reference instruction-set simulator used to check processors built from parts | done for RV32I |
| `circuiter-hdl` | Verilog/SystemVerilog: lexer, parser and an interpreter for the synthesizable subset, running each module as a digital part. Instances of other modules are flattened into their parent when compiled (connected through continuous assignments), `for` and `generate` loops are unrolled, functions run on values of their own | done (no tasks, `while`, `initial`) |
| `circuiter-3d` | 3D geometry: meshes with normals and colours; boxes, cylinders, lathes, extrusions, wires; solid booleans by BSP trees (csg.js's method, in double precision); part bodies generated from packages and parameters; modelled bodies evaluated from feature histories with expressions; sheet assemblies; polygons with holes triangulated in slabs; inside tests for closed meshes (ray parity over a grid); sections; outlines from DXF and SVG drawings; STL export. Rendered in the app with wgpu (offscreen, multisampled, then composited into egui) | first version (meshes, not yet B-rep: no STEP or fillets of arbitrary edges) |
| `circuiter-pcb` | Printed circuit boards: footprints generated from packages (pads, silkscreen, courtyard, fab; KiCad's library names where the same), boards resolved against their schematics (`layout`), copper as polygons (`geometry`, booleans and offsets by `i_overlay`), connectivity and the ratsnest (union-find, then Prim between islands), pours (clearance, thermal spokes, minimum width, island removal, priority), the design rule check, fabrication (Gerber RS-274X/X2 per layer, Excellon, placement, BOM, an SVG plot with a stroke font), KiCad netlists, update from the schematic and automatic placement, track editing (segments dragged keeping their neighbours' angles, corners, vias, tracks following parts, clean-up), the autorouter (`route`: a grid of cells marked with the net each may carry, A* over position, layer and heading with costs for length, bends, vias and a layer's preferred direction, paths pulled taut at 45° multiples, rip-up-free with a second, cheaper-via pass), the board in 3D, and cases made to fit with fit checks and thermal figures | first version (no push-and-shove, no rip-up and reroute, through-hole footprints from generators) |

## Boards and enclosures

A board belongs to the cell whose schematic it lays out (`board.ron` beside
`schematic.ron`): the outline, placed footprints keyed by instance path (`R1`,
`X1.R2`), tracks, vias, zones, design rules and the case. Nothing on it is derived:
pads, nets and pours are worked out each time from the schematic (the physical
flattening: a cell with a footprint is a part on the board, a cell without one is
opened, anything else, like a source or a meter, is off the board), so the board can't
disagree with the schematic about connectivity. Annotation renames footprints with
their parts; Update from Schematic places new parts in free space and removes orphans.

Copper is polygons in millimetres. Clearances, shorts and edge distances are measured
between polygons; pours are boolean differences of the zone outline and everything
else grown by its clearance, with spokes added back and an opening by half the minimum
width. The same polygons feed the Gerber writer (pours as regions with clear holes,
pads flashed where an aperture fits) and the 3D board. Body and footprint conventions
agree (a test checks that every part's leads go into its pads' holes): Y down on the
board is −Y in 3D, the board's top face at z = 0.

A case made to fit is a feature history (shell, cavity, standoffs, openings, lid with
lip and vents), so it can be handed to the body editor as two ordinary 3D parts. The
fit check casts rays through the case's mesh to find part vertices inside its walls.
Its thermal model is lumped: convection and radiation outside, conduction through the
wall, still air inside, vents halving the resistance; the heat capacity of the air and
the shell. Simulations of a cell whose board has a case run with that air node.

## Roadmap

### Milestone 1: circuit lab

1. ~~Core model, project format, standard library~~ (done; the early CLI was later
   folded away so the desktop app is the only binary)
2. GUI, moved earlier so design work can be seen as it happens:
   - ~~viewer: canvas, ANSI symbols, themes, hierarchy navigation, inspector,
     problems~~ (done)
   - ~~editor: place, wire, label, port, move (wires follow), rotate, mirror, delete,
     box select, parameter editing, undo/redo, new cell, save~~ (done)
   - ~~component builder: symbols generated from ports and kept in step with them;
     symbol editor (lines, rectangles, circles, arcs, text, pins; move, rotate,
     mirror); interface editor (ports, kinds, order, parameters, prefix,
     description)~~ (done)
   - copy/paste, multi-sheet designs, a test harness that drives the GUI headlessly
   - visual redesign (requested: make it prettier); styling is kept centralised in the
     scene themes and the app's visuals setup to make this cheap. Includes keeping a
     rotated part's reference and value text clear of its body, as KiCad does with
     field justification
3. ~~Netlister: sheet connectivity, hierarchy flattening, SPICE emission, ERC (no
   ground, single-pin nets, floating nodes, voltage-source loops)~~ (done)
4. Simulator (pure Rust), validated against ngspice:
   - ~~operating point (Newton-Raphson with junction limiting and device current
     checks; gmin and source stepping), DC sweep, transient (trapezoidal, truncation
     error control, source breakpoints); in the app: voltage tags, pin currents and
     power, waveform plot~~ (done)
   - ~~MOSFET intrinsic (Meyer) and junction capacitance, Gummel-Poon BJTs, coupled
     inductors, AC analysis with Bode plots, background runs with progress and
     cancel~~ (done; every model checked against ngspice)
   - ~~sparse solver (10 000-node mesh in 0.26 s, 1 000-MOSFET transient in 0.6 s),
     controlled and `POLY` sources, `.subckt` with parameters (the published µA741
     macromodel matches ngspice), voltage- and current-controlled switches, importing
     manufacturer model libraries as parts~~ (done)
   - later: higher MOSFET levels (BSIM), behavioural sources, `.include`/`.lib`
5. Standard library growth: ~~NPN and PNP (2N3904/2N3906), transformer (linear,
   coupled windings), LED, fuse, VDD rail, CMOS logic gates (NOT, AND, OR, NAND, NOR,
   XOR, XNOR) with hidden supply pins~~ (done); ~~op amps, comparators, regulators,
   references, the 555, zener/Schottky/rectifier/bridge diodes, JFETs, power MOSFETs,
   Darlingtons, thyristors, triacs, optocouplers, sensors, relays, motors, lamps,
   speakers, electrolytics, crystals, varistors, batteries, dependent sources~~ (done,
   built on behavioural sources); next nonlinear transformer cores, 7-segment displays
   and glowing LEDs in live runs, more logic ICs
6. ~~Physics and failure engine~~ (done, first version): cells carry `physics` (thermal
   resistance and capacity, destruction temperature, voltage/current/I²t limits,
   failure mode and effect) in terms of their parameters; the simulator calls an
   observer at every accepted step, which can replace devices, and places the step
   at the failure; standard parts carry realistic data; a fuse part. Next: thermal
   coupling between parts and to a board, temperature-dependent device models,
   electrolytic capacitors (polarity, venting), visual effects (smoke, flash)
7. Virtual bench: ~~oscilloscope, DMM, live runs with switches, buttons and knobs~~
   (done); next a function generator, bench supply, logic analyzer, spectrum
   analyzer, thermal camera, listening to a node as audio
8. Characterization: transistor-level cell to digital model
9. Printing and PDF export with title blocks and standard sheet sizes

### Later milestones

| Milestone | Scope |
|---|---|
| M2 | ~~RISC-V ISS running C, Rust and assembly; a processor built from parts, and from gates alone, running the same firmware; SystemVerilog parts~~ (done, RV32I); next: pipelined cores, interrupts, peripherals, SystemVerilog through Verilator for speed |
| M3 | ~~PCB layout (placement, 45° routing with vias, pours), DRC, Gerber/Excellon/placement/BOM, KiCad netlists; 3D part bodies, a 3D view, a feature-based modeller for parts and enclosures, STL; the board in 3D; cases made to fit, fit checks; DXF/SVG import; ERC, annotation, global labels, power flags, fields~~ (done, first versions); next SMD footprints and a footprint editor, push-and-shove routing, differential pairs, length tuning, rip-up and reroute in the autorouter, ODB++/IPC-2581, a B-rep kernel (fillets and chamfers of any edge, STEP export), sketch constraints beyond dimensions, 3MF, buses with KiCad's `D[0..7]` syntax, PDF schematics with title blocks |
| M4 | ~~The air in a product's case, coupled to every part's temperature~~ (done, lumped); next board and enclosure thermal fields (copper spreading heat, conduction to the case) |
| M5 | Mechanical and product simulation: rigid bodies, sensors, actuators (first steps: a case's lid opening, and the board's switches, pots and buttons moving in 3D as a live circuit runs) |
| M6 | Signal integrity and EM: impedance, crosstalk, field solver |
| M7 | Acoustics (Thiele/Small driver models, for example headphones), parts ordering, KiCad import |
| Later | Physical-level component design: device parameters derived from geometry and materials (MOSFET dimensions and doping, transformer core and windings) |

## Licensing policy

Circuiter is dual-licensed MIT OR Apache-2.0. Dependencies must be compatible:

- Permissive (MIT, Apache, BSD) can be linked directly. Code ported from BSD-licensed
  SPICE sources (e.g. device models) keeps its notices.
- LGPL (Verilator runtime, OpenCASCADE) only via dynamic linking or a separate process.
- GPL (e.g. openEMS) only as a separate process, never linked.
