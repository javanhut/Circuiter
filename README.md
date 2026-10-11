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
> built-in circuit simulator (operating point, DC sweep, transient, AC), a physics
> engine in which overstressed parts heat up and fail, a digital simulator with the
> parts to build processors (and an example RISC-V CPU, also made entirely of gates),
> mixed analog and digital circuits, live runs you can operate with switches, buttons
> and knobs while multimeters and an oscilloscope watch, programs in RISC-V assembly,
> C, Rust and SystemVerilog, 3D bodies for parts, a CAD modeller for your own
> parts and enclosures, an electrical rules check and annotation as in KiCad, a
> printed circuit board editor (placement, 45° routing with vias, copper pours,
> design rule checks, Gerber and drill files, KiCad netlists), and cases made to fit
> a board, checked for fit and simulated thermally with the circuit inside. See
> [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the design and roadmap.

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
cargo run --release -p circuiter-app -- examples/riscv
cargo run --release -p circuiter-app -- examples/bench
cargo run --release -p circuiter-app -- examples/analog_lab
```

The examples are a CMOS inverter built from two MOSFETs and driven by a clock, a
one-transistor audio amplifier (a 2N3904 common-emitter stage, about 44 dB of gain)
with a power LED, a half adder built from logic gates, an overstress lab where an
LED, a fuse and a resistor are pushed past their limits, a RISC-V processor that runs
programs in assembly, C and Rust (built from digital parts, and again from gates
alone), an electronics bench to run live (a supply to turn, switches, a button, LEDs,
a counter, multimeters and an oscilloscope), and an analog lab of everyday circuits: an
op-amp amplifier, a 555 blinker, a mains power supply with a 7805, a light switch, a
motor driver and a relay driver. The blinker is laid out on a board (PCB view) with a
ground pour and every connection autorouted, in a vented case (3D view).

```sh
cargo run --release -p circuiter-app -- examples/analog_lab --cell blinker --view pcb
cargo run --release -p circuiter-app -- examples/analog_lab --cell blinker --view board3d --select section
```

Start your own with File → New Project… (or the welcome screen): pick an empty folder
and the project is named after it.

**The window**

- Left: **Project** and **Libraries** trees (folders and cells), and below them
  **Components**: every part by kind, with a search box. Click a part, then the sheet;
  or drag it onto the sheet.
- Middle: open cells as **tabs**, each remembering where it was left. Above the sheet,
  switch between **Schematic**, **Symbol**, **PCB** (the board), **3D** and **Netlist**
  (the SPICE deck the simulator runs), and pick drawing tools and zoom. The column of common parts beside
  the sheet works like Components.
- Right: **Properties** of what's selected (or the cell), **Simulation** (run, results,
  plotted signals) and **Physics** (ratings, temperatures, what failed).
- Bottom: **Waveforms**, **Thermal**, **Problems** and **Oscilloscope**. Hide it with
  its ×; Run brings it back.
- Help → Keyboard Shortcuts lists every key.

**Viewing**

| Action | How |
|---|---|
| Zoom | Mouse wheel (about the cursor), or pinch |
| Pan | The hand tool (`H`, or the hand in the toolbar), then drag; or middle or right drag, or Space + left drag, with any tool |
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
| Draw a wire | Drag out of a pin's end or a wire's end (it's marked when the pointer is over it) and let go on another pin or wire, or click the end and click onwards: that's how a loose wire is joined up. Double-click a wire to start a branch off it there. Or `W`, then click. Wires route at right angles (`/` flips the corner) and finish when they reach a pin or wire, or on double-click. A wire connects to a pin only at the wire's end or a corner: running past a pin doesn't connect it (Problems points it out). The status bar says what each edit connected or disconnected |
| Net label | `L`, type a name, click on a wire. Same-named labels on a sheet are connected; tick Global (or Place → Global Label…) to join the net on every sheet of the design, drawn as a flag |
| No-connect flag | `Q`, then click a pin meant to be left open: it isn't reported as unconnected (click again to take the flag off) |
| Power | `G` ground, `Shift+V` VDD; Place → Power Rail for any named rail (`+5V`, `+3V3`, `-12V`: set its net), joined everywhere by name; Place → Power Flag to tell the rules check a net is supplied through a switch, fuse or connector it can't see through |
| Port | `P`, name and direction, then click. Ports are how a cell connects to the sheets that use it |
| Select | Click; Shift+click adds; drag across empty space to box-select. Selecting a wire highlights its whole net |
| Move | Drag a part (whatever tool is active): its wires stay attached, re-routing at right angles around it. Drag a wire to move the segment under the pointer sideways, as in other EDA tools: a horizontal run up or down, a vertical one left or right, the segments either side stretching to follow and a jog added where it ends on a pin. Whatever joins it part-way (a T, a label) stays joined. Select several things to move them together |
| Rotate, mirror, delete | `R`, `X`, `Delete` or `Backspace` |
| Copy, cut, paste, duplicate | `Ctrl+C`, `Ctrl+X`, `Ctrl+V` (at the pointer), `Ctrl+D`. Parts come with the wires between them and get fresh names; paste works across sheets and between Circuiter windows |
| Edit values | Change the name or any parameter in the inspector (`4k7`, `100n`, `2u`…); invalid values are refused. ↺ resets to the default |
| Undo, redo | `Ctrl+Z`, `Ctrl+Y` (per sheet) |
| Annotate | Tools → Annotate Schematic…: every part gets its prefix and the next number in reading order (across rows or down columns), keeping numbers already given or starting over; the board's footprints follow their parts |
| Fields | Select a part: Fields shows the value and the KiCad footprint it will use, and holds its MPN, manufacturer and datasheet (and any field you add) for the bill of materials; Do not populate leaves it off the assembled board |
| New cell | `Ctrl+N`: an empty schematic to build a new component in, optionally in a folder |
| Folders | Cells sit in folders, as deep as you like (`cpu/alu/adders`). Right-click a folder (or the project) for New Folder, New Cell Here, Rename and Delete (when empty); drag a cell onto a folder to move it, or set its folder in Properties. Folders stay until deleted, even empty, and on disk they're ordinary directories. The standard library is sorted the same way (`gates/`, `resistors/`, `transistors/`…); imported models go in `imported/` |
| Save | `Ctrl+S`. The title bar shows ● while there are unsaved changes, and you're asked before anything would discard them |

**Building a component**

A component is a cell built from other parts, which then becomes a part itself. A
transistor-level gate, a filter, a whole CPU block: all are built the same way, and
nest as deep as you like.

1. `Ctrl+N` (or the toolbar's new-cell button) makes a new cell, in a tab of its own.
   The empty sheet shows these steps.
2. Drag parts onto it from Components or the column beside the sheet, and wire them by
   dragging from one pin's end to another.
3. Place a port (`P`, a name and a direction, then click a pin's end or a wire) for
   each connection the component should have. As soon as it has ports, a symbol is
   generated: inputs on the left, outputs on the right, supplies on top, grounds at the
   bottom.
4. It's now a part: it's listed first in Components (This project), ready to drag onto
   other sheets. Double-click a placed one to go inside it.
5. With nothing selected, Properties edits the interface: description, reference
   prefix, ports (rename, change direction, reorder, remove) and parameters. A
   generated symbol follows every change.
6. To draw the symbol yourself, switch to the Symbol view and edit it. Once you do,
   your drawing is kept; new ports still get pins, which you can then move.

Or build and test the circuit on a sheet first, select it, and choose **Make Component
from Selection** (Edit menu, the puzzle-piece button, or Properties). The parts and the
wires between them become a new cell; every connection to the rest of the sheet becomes
a port (named after its net label when it has one), while GND and VDD stay connected by
name. The sheet itself is left as it was.

**Simulating**

Circuiter has its own simulator, written in Rust and built into the app, so there is
nothing to install. It speaks SPICE and is checked against ngspice.

| Action | How |
|---|---|
| Run | **▶ Run** (`Ctrl+R`) switches the circuit on (everything starts off, as with a real power switch) and watches it for a sensible time: three cycles of the slowest clock or signal, or 10 ms; a processor runs until its program ends (the toolbar's Time then reads "until ECALL"). Click Time to choose another length. Waveforms appear below the sheet, each wire is tagged with its final voltage, and overheated parts are marked |
| Run Live | Keeps the circuit running until you press Stop, so you can operate it as it runs; see below |
| Operating point | `F9`, or Analyses → Operating Point. Every net is tagged with its voltage; select a part to see the current into each pin and the power it dissipates (or the total for a sub-circuit) |
| Transient | `F10`, or Analyses → Transient…: the stop time and resolution, and whether to start from power-on or from the steady state (SPICE's usual start) |
| AC sweep | Analyses → AC Sweep…: gain and phase over frequency (a Bode plot), driven by each source's AC value; a sine source's `ac` parameter is 1 V by default |
| Long runs | Simulations run in the background; the status bar shows progress and `Esc` cancels |
| Manufacturer models | File → Import SPICE Models… reads a vendor `.lib`/`.mod` file: each `.subckt` becomes a part with a generated symbol (rename its pins in the inspector), each diode or transistor `.model` a part drawn like the standard one. Models the simulator can't run yet are flagged on import |
| Plots | Select a wire, then Add to Plot; `✖` on a name removes it. Wheel zooms, drag pans, double-click shows everything; hover for values |
| Problems | No ground, nets with no DC path to ground and shorted voltage sources are reported under Problems. A circuit without ground still simulates, as a real one would, against board leakage |

Results go stale when you edit the circuit; run the analysis again to refresh them.

**Electrical rules check**

As you edit, the rules check reads what each pin is (an input, an output, a supply,
passive…) and reports under Problems, marked ERC:

- outputs driving against each other, and an output shorted to a supply;
- inputs nothing drives;
- a chip's supply pin on a rail no source or regulator feeds (put a Power Flag on a
  rail fed through a switch, fuse or connector, as in KiCad);
- labels joined to nothing, global labels used once, and no-connect flags on pins that
  are connected after all;

alongside the sheet's own checks (unconnected pins and wire ends, wires passing over
pins, conflicting net names). Big designs are checked in the background. Tools →
Electrical Rules Check opens Problems.

**Boards**

Open a cell's **PCB** view and press Create Board from Schematic: a two-layer board
sized for the parts, every part with a footprint placed on it, joined by the ratsnest
(thin lines showing what's still to connect). Layers are drawn in KiCad's colours, the
active one on top.

| Action | How |
|---|---|
| Move a part | Drag it (snaps to the grid); the tracks on its pads come with it. `R` turns it 90°, `F` flips it to the back (tracks follow both), `L` locks it, `Delete` takes it off |
| Rearrange tracks | Drag a track: it slides square to itself and the tracks either side stretch to meet it at their own angles (a new leg joins it to a pad). Drag a corner where tracks meet, or a via, and the tracks there follow. `Delete` removes the piece selected |
| Autoroute | Autoroute → Route All turns every ratsnest line into tracks: shortest first, at 0°, 45° and 90°, keeping each net's clearance, across every copper layer with vias where it must change side (the front prefers to run across, the back up and down), each path pulled as straight as it can go. Route the Selection's Nets does only the nets of the selected part, track or via. Whatever can't get through stays on the ratsnest, named |
| Tidy up | Autoroute → Clean Up Tracks joins segments in a line and takes off stubs and duplicates; Unroute takes a net's tracks off, or all of them |
| Route | `X` (or the route tool), click a pad, then click to lay each corner: tracks run at 0°, 45° and 90° (`/` swaps which leg comes first), at the net's width, and a track that would come too close to other copper shows red with its clearance outline. Click a pad, track or via of the same net to finish. `V` drops a via and carries on on the other side; picking another layer (panel, toolbar, `Page Up`/`Page Down`) mid-route drops a via and carries on on that one, inner layers included; `Backspace` takes back the last leg; `Esc` stops |
| Layers | The Layers panel beside the board lists every layer in its colour: click a copper layer to work on it, the eye to show or hide any layer (silkscreen, courtyards, fab drawings, the edge), and choose 2, 4 or 6 copper layers. Pours, ratsnest and rule markers switch on and off there too, and Dim other layers fades everything but the layer in hand. `Page Up` / `Page Down` step through the copper layers |
| Copper pour | The zone tool, pick its net (GND by default), click its corners, double-click to finish. It fills to the board's edge, keeps the clearance from other nets, joins its own pads through thermal spokes and drops islands; select it to set priority, pad connection, relief gap and spoke width |
| Board shape | Width and height in Properties, the outline tool to draw any shape, or Import Outline… from a DXF or SVG drawing |
| Rules | Properties → Design rules: clearance, track width, via size, the board house's minimums, edge clearance, hole spacing |
| Update | Update from Schematic places parts new to the schematic and takes off those deleted from it |
| Undo | `Ctrl+Z`, as anywhere |

The design rule check runs as you go and lists, under Design Rules below the board,
shorts, clearances, narrow tracks, small drills, thin annular rings, copper too near
the edge, holes too close, overlapping courtyards and unrouted connections; click one
to go there. The status line counts what's unrouted and what's wrong.

**Fabrication files** writes, into a folder you choose: a Gerber file per layer
(RS-274X with X2 attributes: copper, solder mask, paste, silkscreen, board outline),
Excellon drill files for plated and unplated holes, the placement file for the
assembly machine, the bill of materials (grouped, with MPN and manufacturer, DNP parts
apart), and an SVG picture of the board. Tools → Export KiCad Netlist… writes the
design as a KiCad netlist with KiCad's library footprints where they're the same, to
lay out in Pcbnew instead; Export Bill of Materials… writes the BOM on its own.

**Cases and assemblies**

The board's **3D** view (or 3D in the PCB toolbar) shows it as it'll be made: green
mask over the copper, tinned pads, white silkscreen, every part standing in its holes.
Make a Case gives it a base and lid made to fit: walls following the board's outline
at a gap, standoffs under its mounting holes or corners tall enough to clear the leads,
openings in the walls for connectors at the edge and in the lid for buttons, switches,
pots and LEDs, a lip that locates the lid, and vent slots if you want them.

- **Closed**, **Open** (the lid lifted off, by a slider), **Section** (cut through the
  middle) or **No case**.
- **Fit**: every part's body is checked against the case's walls, floor and lid, and
  the board's edge against the walls: "Everything fits", or which parts don't and by
  how much.
- **Heat**: the case's walls, size and material (plastic or aluminium, with or without
  vents) set how much the air inside warms per watt. Simulations run with the circuit
  inside it: every part's power warms the air, the air warms every part, and Thermal
  plots the air in the case with the parts. Turn Simulate in it off to run in the room.
- **Your own case**: Make Editable 3D Parts turns the case into two 3D parts to change
  in the body editor; or pick any 3D part (and lid) of the project, with where the
  board sits in it, and it's checked the same way.
- **Running it**: while the circuit runs live, the 3D board follows it: switch levers
  flip, pots turn, buttons press, and LEDs light.

The case's settings are in the PCB view's Properties, under Case.

**Running live**

**Run Live** (beside Run) switches the circuit on and keeps it running, like a circuit
on the bench, until you press Stop. A bar above the sheet shows the simulated time,
Pause/Resume, Stop and the **speed**: simulated time per real second, from 1 ns/s (to
watch a fast processor step) up to 2 s/s; 1 s/s is real time. It starts at a speed
that shows the design's activity. If the circuit needs more computing than a speed
allows, the bar says so. While it runs, wires show their voltages (or logic values) as
they change, LEDs light, meters read and the oscilloscope sweeps. Edit the circuit and
the bar offers Restart.

Parts you can operate (folder `interactive/`):

| Part | What it is | Live |
|---|---|---|
| `switch` | Switch (SPST), open or closed | Click it to flip it |
| `pushbutton` | Closed only while held | Hold the mouse on it |
| `dial` | Adjustable DC supply between min and max | Drag its knob in Properties |
| `potentiometer` | A resistor with a sliding tap | Drag its knob in Properties |
| `switch_spdt` | Changeover switch: com to a, or to b when thrown | Click it to throw it |
| `toggle` | Logic switch: a steady 0 or 1 | Click it to flip it |
| `button` | Logic button: 1 while held | Hold the mouse on it |
| `high`, `low` | Steady logic 1 and 0 | |

Sensors take sliders too: light on a light-dependent resistor, photodiode or
phototransistor (lux), sunlight on a solar panel (W/m²), and the temperature of a
thermistor or LM35. Outside a live run, their parameters (closed, pressed, on, value,
position, lux, temp) set how they start.

**Instruments**

Instruments are parts (folder `instruments/`) with leads that clip onto any wire: drop
one on the sheet and drag its leads to the wires to measure. They change nothing in
the circuit except what a real one would.

| Instrument | Use |
|---|---|
| `multimeter` | Mode V, A or Ω in Properties. **V**: red lead and COM across a part (10 MΩ input). **A**: in series, in place of a wire (0 Ω): break the wire and put the leads on its ends. **Ω**: across an unpowered part (1 mA test current). The reading shows on the meter on the sheet and in Properties, live or after a run |
| `oscilloscope` | Four channels, each the voltage at its probe against COM (or ground when COM is left open). The Oscilloscope tab shows the traces with a front panel: channels on and off, volts per division for each, time per division, trigger (channel, level, rising or falling, auto), Run/Stop, Autoscale, and measurements per channel (peak-to-peak, mean, frequency). Live it sweeps continuously; after a Run it shows the recorded run |

**Lights**

LEDs and lamps glow on the sheet in their colour, brighter with more current: live as
the circuit runs (averaged over each frame, so a dimmed or fast-blinking LED looks dim
rather than flickering), and after a Run, as they were at its end. An LED's `color`
(red, orange, yellow, green, blue, white) sets its forward voltage too: about 2 V for
red, 3.1 V for blue and white at 20 mA.

**Parts for analog circuits**

Besides resistors, capacitors, inductors, transformers, diodes, LEDs, bipolar and MOS
transistors, sources and fuses, the standard library has:

| Folder | Parts |
|---|---|
| `amplifiers/` | `opamp` (set by its datasheet figures: gain, gain-bandwidth, slew rate, output swing, output resistance and current limit, supply current, offset), `lm358`, `tl072`, `ne5532`, `ua741` (the published Boyle macromodel), `lm393` comparator (open collector) |
| `regulators/` | `lm7805` (any 78xx: set vout), `ams1117` low-dropout, `lm317` adjustable, `tl431` shunt reference. Their dropout and current limit are modelled, and the heat they burn shows in Physics |
| `timers/` | `ne555` |
| `diodes/` | `zener`, `schottky`, `rectifier` (1N4007), `bridge` rectifier, `photodiode` |
| `transistors/` | `njf` and `pjf` JFETs, `nmos_power` and `pmos_power` MOSFETs (with body diodes), `darlington`, `scr` thyristor, `triac`, `phototransistor` |
| `optocouplers/` | `optocoupler` (LED and phototransistor, set by its current transfer ratio) |
| `sensors/` | `thermistor` (NTC), `ldr`, `solar_cell` panel, `lm35` temperature sensor |
| `electromechanical/` | `relay` (coil with inductance, pull-in and drop-out currents, changeover contacts), `dc_motor` (winding, back-EMF, inertia, friction and load torque), `speaker`, `lamp` (incandescent, with its cold-start surge) |
| `capacitors/`, `crystals/`, `inductors/`, `protection/` | `electrolytic` (polarised: reverse it and it heats and bursts), `crystal`, `transformer_ct` (centre-tapped), `varistor` |
| `sources/`, `instruments/` | `battery` (with internal resistance), `isine`, `function_generator` (sine, square or triangle) |
| `controlled/` | `vcvs`, `vccs`, `cccs`, `ccvs`: the four dependent sources, for building models |

Parts carry ratings like the others: a regulator with too much to drop overheats, a
reversed electrolytic bursts, a motor stalled too long burns out.

**3D**

The **3D** view shows a part's physical body, or, on a sheet, every part's body placed
where its symbol is, on a board (a preview until boards are laid out). Drag to orbit,
right-drag or Shift-drag to pan, wheel to zoom, double-click (or `F`) to fit; Top,
Front, Side and Iso set the angle. **Export STL…** saves what's shown for a slicer or
another CAD program.

Every physical standard part has a body made from its parameters: an axial resistor
shows its value's colour bands, a DIP its pin count, a toggle switch's lever leans the
way the switch is set; TO-92, TO-220, SOT-223, LEDs, electrolytic cans, relays, motors,
transformers, crystals, batteries, solar panels and more. Sources, instruments and
logic blocks have none.

**Modelling parts and enclosures**

Your own cells can have bodies too, modelled in the 3D view's **Body** panel as in a
CAD program. Start a new one with the cube button beside New Cell in the Project tree
(or File → New 3D Part…, or New 3D Part Here… on a folder); for an existing cell,
Properties has a **3D body** section with Model a 3D Body. A body is a history of
features, each adding to, cutting from or intersecting the body made so far.

| Feature | What it makes |
|---|---|
| Box | A box, optionally with rounded vertical edges |
| Cylinder, cone, sphere | The solids, a cone possibly truncated |
| Extrude | An outline you draw, raised to a height |
| Revolve | An outline turned about the vertical axis, all the way or part way |
| Hole | A cylinder that cuts |
| Copies, Ring, Mirror | Copies of an earlier feature in a row, around the axis, or reflected |

Each feature has a placement (where, and turned about each axis) and a colour, and can
be hidden, reordered or deleted. Dimensions are expressions in millimetres that may
use the cell's parameters (`width - 2*wall`), so changing a parameter reshapes the
part. Outlines are drawn in a sketch editor: drag corners (snapping to 0.5 mm), click
an edge's + to add a corner, right-click one to remove it, start from a rectangle,
circle, slot or hexagon, or type the corners, with arcs between them. Each edge shows
its length; under Dimensions, type an edge's length and direction (the corners after
it follow) or the outline's overall width and height. Every change can be undone.

**From 2D**: Import Outline (DXF, SVG)… extrudes a drawing (lines, polylines with arcs,
circles, rectangles, paths), its inner outlines cut through as holes; From the Symbol
makes a body of the part's symbol outline, its inner drawings raised on top. A cell with a standard package can switch to a modelled body (Model it
yourself instead), and a new cell can start from either.

`examples/analog_lab` has an enclosure modelled this way (`case` and `case_lid`): a
shell with screw posts, holes for an LED, a switch and a DC jack, vents, and a lid with
countersunk screw holes, all following its width, depth, height and wall parameters.

**Mixed analog and digital circuits**

A circuit can mix analog parts (resistors, LEDs, transistors, sources) with digital
ones (gates, counters, processors). The two simulators run together on one timeline:
where a digital output drives an analog net it becomes a voltage source (0 V or the
supply, VDD if the design has one, else 5 V) behind 50 Ω, opened when the output is
undriven; where a digital input reads an analog net, the voltage is read through
thresholds at 30 % and 70 % of the supply (with hysteresis between them), and the time
it crosses is found between the analog steps. So a counter can blink an LED through
a resistor, or a sine wave can clock a counter. Only single wires can join the two
worlds, not buses.

Parts with both a digital and a transistor-level model (the gates, for instance) run
digitally in digital and mixed designs and at transistor level in analog ones. To
choose for one part, select it and set **Simulate as** in Properties: Automatic,
Digital model (fast), Transistor level (SPICE), or its schematic.

**Logic gates**

`std:not`, `and`, `or`, `nand`, `nor`, `xor` and `xnor` are real CMOS gates (74HC-like,
2 to 6 V) with the usual ANSI shapes. Their supply pins are hidden: they connect to the
VDD and GND nets wherever the gate is placed, so place a VDD symbol (`std:vdd`) on a
voltage source's positive terminal, or label that net `VDD`. Several waveforms are drawn
in lanes, as a logic analyser shows them.

**Digital design and processors**

A design made only of digital parts runs in the digital simulator: event-driven,
four-state (0, 1, X unknown, Z undriven), buses up to 64 bits, propagation delays. It is
fast enough to run a processor for thousands of cycles in a fraction of a second. Bus
wires carry several bits (a port's width is set per instance by parameters, so one
register part serves 8 or 32 bits). After a run, wires show their final values in hex,
and Waveforms shows bits as square waves and buses as boxes holding their values; the
pointer reads every signal at its time.

The digital parts in the standard library:

| Folder | Parts |
|---|---|
| `gates/` | not, and, or, nand, nor, xor, xnor (also simulated at transistor level when analog) |
| `flipflops/` | dff, register (with reset and enable), pc |
| `counters/` | counter |
| `muxes/` | mux2, mux4, mux8, decoder |
| `arithmetic/` | add, adder (with carries), alu (the RV32I operations), compare |
| `buses/` | const, slice, concat, extend (zero or sign) |
| `memory/` | regfile (32 × 32, x0 is zero), rom (instruction memory), dmem (RISC-V loads and stores) |
| `riscv/` | immgen, control (the RV32I decoder), halt (ECALL ends the run) |
| `sources/` | clock (also analog), reset |
| `interactive/` | toggle, button, high, low (and the analog switch, pushbutton, dial, potentiometer) |

`examples/riscv` is a single-cycle RV32I processor built from these parts on one sheet
(`cpu`), with its branch unit as a component of its own (a comparator, a multiplexer
and gates), and a 4-bit adder made from gate-level full adders. Press Run: the program
in its instruction memory runs until ECALL; the register file and data memory are
checked against a reference instruction-set simulator in the tests.

`cpu_gates` is the same processor with every block made of logic gates, which you can
open level by level: the register file is 31 registers of 32 flip-flops, a decoder and
two trees of multiplexers; each flip-flop is two latches of NAND gates; the ALU is a
ripple-carry adder of full adders, two barrel shifters and rows of AND, OR and XOR
gates; the control unit and immediate generator decode the opcode with AND gates. The
gates themselves open to their CMOS transistors. About 40,000 gates in all; only the
program ROM and data RAM are memory arrays, as they would be on a chip, and bus parts
(slice, concat, extend, const) only route bits. Its clock is 2 MHz, since a cycle has
to cover the gate delays of 32-bit ripple-carry addition; it runs the same programs
with the same results, though a run takes seconds (in a release build) where `cpu`'s
takes a fraction of one.

**Programs**

Programs live in the project's `programs/` folder, listed under the cells. Click one to
edit it in a tab: syntax colouring, line numbers, Check (builds it and marks the lines
with errors), Save (`Ctrl+S`), and Run on the open sheet, which loads the program into
the processor's memories and runs it. New Program makes one from a template; the file
ending picks the language:

| Language | How it's built |
|---|---|
| RISC-V assembly (`.s`) | Circuiter's own RV32I assembler: labels, the usual pseudo-instructions, data directives. Byte-for-byte the same as LLVM's |
| C (`.c`) | clang for `riscv32` with lld, linked for the processor's memory map, with a small runtime for multiply and divide |
| Rust (`.rs`) | cargo for `riscv32i-unknown-none-elf`; a `no_std` file with `#[unsafe(no_mangle)] pub extern "C" fn main() -> u32`. Without that target installed, Rust's `core` is built from source once |
| Executables (`.elf`) | Loaded as they are |

Code goes from address 0; data, constants and the stack are in the 4 KiB data memory
at `0x1000_0000`; `main`'s return value ends in register a0.

**Parts in Verilog and SystemVerilog**

Describe a part in HDL instead of drawing it: write a module in a `.v` or `.sv` file
under `programs/`, open it, and press Make Part. Its ports (with their widths) become
the part's pins, and it runs in the digital simulator from the source file, so editing
the file changes the part. The synthesizable subset is supported: `assign`,
`always_comb`, `always_ff @(posedge clk or posedge rst)`, non-blocking assignments,
`if`/`case`/`casez`, every operator, parameters, `$signed`/`$clog2`; instances of other
modules in the file (connections by name or position, `#(...)` parameters); `for`
loops in always blocks and `generate for`/`if` (unrolled, so their bounds must be
constants); and functions, with `return` or by assigning the function's name. A file
with several modules makes a part of its last one (the top). Not supported: tasks,
`while`/`repeat`, `initial`, inout ports of instances.
`examples/riscv/programs/control.sv` is the processor's control unit in SystemVerilog.

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
| `crates/circuiter-netlist` | Flattening designs for the analog and digital simulators |
| `crates/circuiter-sim` | The analog circuit simulator (SPICE-compatible, pure Rust) |
| `crates/circuiter-physics` | Heating, limits and failures of parts |
| `crates/circuiter-logic` | The digital simulator and its parts |
| `crates/circuiter-riscv` | RV32I assembler, ELF loader, reference instruction-set simulator |
| `crates/circuiter-hdl` | Verilog/SystemVerilog parser and interpreter |
| `crates/circuiter-3d` | Meshes, solid booleans, part bodies from parameters, modelled features, inside tests, DXF/SVG import, STL |
| `crates/circuiter-pcb` | Footprints, boards linked to schematics, copper geometry, connectivity and ratsnest, pours, design rule checks, Gerber/Excellon/BOM/placement files, KiCad netlists, the board in 3D, cases made to fit |
| `crates/circuiter-app` | The `circuiter` desktop app (egui on wgpu), the only binary |
| `examples/` | Example projects |
| `docs/` | Architecture and design notes |

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in Circuiter by you, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.
