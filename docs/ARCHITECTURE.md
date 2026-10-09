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
2. **Integrate proven engines, own the glue.** ngspice for analog, Verilator for
   SystemVerilog, an established geometry kernel for CAD. Circuiter writes its own code
   where it is core and tractable: the data model, the RISC-V instruction-set simulator,
   the co-simulation scheduler, and the physics and failure engine.
3. **Built to last.** Each milestone ships at production quality. Simulation results
   are validated against analytical solutions and datasheet curves.
4. **Headless first.** Every capability is exposed through the library and the CLI
   before it reaches the GUI, which keeps it testable and scriptable.

## Design model

### Sheet connectivity

`circuiter_model::connectivity` decides what is joined on a sheet, following common
schematic conventions:

- Items at the same point connect.
- A pin, wire vertex, label or port on a wire's segment connects to that wire (a T).
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

**View selection** decides how each instance is elaborated. The design has a view
order (default: schematic, then spice), and an instance can force a view. This is how
mixed fidelity works: simulate one gate at transistor level while the rest of the CPU
runs on fast digital models.

### Libraries and projects

- A **library** is a directory of cells (`cells/<name>/{cell,symbol,schematic,spice}.ron`).
- The built-in `std` library ships inside the binary and uses the same format.
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
schematic hierarchy ──► netlister ──► flat netlist ──┬─► ngspice (analog, XSPICE digital)
                       (view selection,              ├─► RISC-V ISS (firmware)       [M2]
                        ERC, A/D bridges)            └─► Verilator (SystemVerilog)   [M2]
                                                          ▲
                                     co-simulation scheduler (one timeline)
                                                          ▲
                              physics engine: power ► thermal RC ► stress ► failure
```

- **Netlister**: flattens the hierarchy, derives nets from wire geometry, runs ERC
  (floating nets, no DC path to ground, shorted sources), inserts A/D bridges at
  analog/digital boundaries, and emits SPICE.
- **ngspice** runs as a shared library. Its sync callbacks and `EXTERNAL` sources let
  other domains (the RISC-V ISS, Verilator) step in lockstep with the analog solver.
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
| `circuiter-cli` | `circuiter` command: `new`, `check`, `fmt`, `tree`, `nets`, `render`, `lib` | done |
| `circuiter-scene` | Schematic → display list (shared by screen, SVG and print), themes, SVG export | done |
| `circuiter-app` | Desktop GUI (egui on wgpu) | viewer done, editing next |
| `circuiter-netlist` | ERC, flattening, SPICE emission (sheet connectivity is in the model) | M1 |
| `circuiter-spice` | ngspice shared-library bindings | M1 |
| `circuiter-sim` | Co-simulation scheduler, waveforms, analyses | M1 |
| `circuiter-physics` | Ratings, thermal networks, stress and failure | M1 |
| `circuiter-render` | Custom wgpu renderer for very large sheets and 3D | when egui's painter runs out |
| `circuiter-rv` | RISC-V ISS (RV32IMC) with peripherals | M2 |
| `circuiter-hdl` | Verilator integration | M2 |
| `circuiter-pcb`, `circuiter-mfg` | Layout, DRC, Gerber/drill/BOM/pick-and-place | M3 |

## Roadmap

### Milestone 1: circuit lab

1. ~~Core model, project format, standard library, CLI~~ (done)
2. GUI, moved earlier so design work can be seen as it happens:
   - ~~viewer: canvas, ANSI symbols, themes, hierarchy navigation, inspector,
     problems~~ (done)
   - editor: place, wire, move, rotate, edit parameters, undo/redo, save
   - component builder: turn a schematic into a reusable cell with its own symbol
3. Netlister: ~~sheet connectivity (nets, junctions, unconnected pins)~~ (done), ERC,
   hierarchy flattening, SPICE emission
4. ngspice integration: operating point, DC sweep, AC, transient; waveform output
5. Standard library growth: BJTs, JFETs, coupled inductors and transformers (with
   nonlinear cores), op-amps, switches, fuses, XSPICE digital primitives (gates, D
   flip-flops), each with ratings and thermal data
6. Physics and failure engine
7. Virtual bench: oscilloscope, DMM, function generator, bench supply, logic analyzer,
   spectrum analyzer, thermal camera, listening to a node as audio
8. Characterization: transistor-level cell to digital model
9. Printing and PDF export with title blocks and standard sheet sizes

### Later milestones

| Milestone | Scope |
|---|---|
| M2 | RISC-V ISS running Embedded C/assembly; SystemVerilog through Verilator; custom cores running the same firmware |
| M3 | PCB layout, DRC, manufacturing outputs, 3D board view, B-rep CAD kernel, enclosures, STEP/STL/3MF |
| M4 | Board and enclosure thermal simulation |
| M5 | Mechanical and product simulation: rigid bodies, sensors, actuators |
| M6 | Signal integrity and EM: impedance, crosstalk, field solver |
| M7 | Acoustics (Thiele/Small driver models, for example headphones), parts ordering, KiCad import |
| Later | Physical-level component design: device parameters derived from geometry and materials (MOSFET dimensions and doping, transformer core and windings) |

## Licensing policy

Circuiter is dual-licensed MIT OR Apache-2.0. Dependencies must be compatible:

- Permissive (MIT, Apache, BSD, including ngspice) can be linked directly.
- LGPL (Verilator runtime, OpenCASCADE) only via dynamic linking or a separate process.
- GPL (e.g. openEMS) only as a separate process, never linked.
