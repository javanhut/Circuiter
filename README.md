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
cargo run --release -p circuiter-app -- examples/devboard
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

The development board is a microcontroller board as a board house would build it: a
32-bit microcontroller in an LQFP-48 with its decoupling, USB Micro-B (D+ and D− routed
as a differential pair), a 3.3 V regulator, an 8 MHz crystal, reset and boot buttons,
LEDs, an I²C EEPROM and its GPIO on pin headers, 42 parts on a four-layer board with
rounded corners: signals on the outer layers, ground and 3.3 V planes inside, ground
poured over both sides and stitched, every connection routed and the rules check clean.

```sh
cargo run --release -p circuiter-app -- examples/devboard --view pcb
cargo run --release -p circuiter-app -- examples/devboard --view board3d
```

Start your own with File → New Project… (or the welcome screen): pick an empty folder
and the project is named after it.

**The window**

The title bar switches between four **workspaces**, each a place of its own with only
the panels it needs. Each keeps its panels' tabs, sizes and what's shown while you're
in another.

| Workspace | What's there |
|---|---|
| **Schematic** | The sheet, with **Schematic**, **Symbol** and **Netlist** (the SPICE deck the simulator runs) above it; **Components** to place; **Problems** below |
| **PCB** | The board editor, its layers, and below it the design rules, nets, net classes, pairs and stack-up |
| **3D** | Part bodies to model, the sheet's parts in 3D, the board in its case |
| **Simulation** | The sheet with **Simulation** (run, results, plotted signals) and **Physics** (ratings, temperatures, what failed) beside it, and **Waveforms**, **Thermal**, **Problems** and **Oscilloscope** below. Starting a run from the schematic brings it forward |

- Left: **Project** and **Libraries** trees (folders and cells), and in Schematic and
  Simulation, **Components**: every part by kind, with a search box. Click a part, then
  the sheet; or drag it onto the sheet.
- Middle: open cells as **tabs**, each remembering where it was left. The column of
  common parts beside the sheet works like Components.
- Right: **Properties** of what's selected (or the cell).
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

Port names start with a letter and use letters, digits, `_` and `'`, so an inverted
output can be called `Q'`. Cell and instance names keep to letters, digits and `_`,
since they become folder names.

**Giving a component a footprint**

A component with a footprint is a part on the board; one without is a circuit whose
parts go on the board instead. Select nothing (so Properties shows the cell) and
scroll to **Footprint**:

1. **Choose a package…**: chip resistors and capacitors (0603, 0805, 1206), SOD-123,
   SOT-23, SOT-223, SOIC, DIP, axial and radial parts, discs, LEDs, TO-92, TO-220,
   bridges, crystals, pin headers, terminal blocks, relays, pots, switches,
   transformers, batteries and more. Its pads, silkscreen, courtyard and fab drawing
   are made for it, matching the 3D body of the same package, and Properties shows its
   name in KiCad's libraries (`Package_DIP:DIP-8_W7.62mm`).
2. **Its size**: packages that come in sizes show their parameters: **pins** for a DIP,
   SOIC, header or terminal block; **pitch**, **length**, **diameter** and **drill**
   for an axial part (and **style** `diode` for a cathode band); **pitch** and
   **diameter** for a radial one. Leave one blank for its default (shown faintly in
   the field), or write `${param.NAME}` to take it from the part's own parameter, so
   each placed copy can differ.
3. **Pads for each port**: every port with the pad it's joined to. Ports go to pads in
   order (the first port to pad 1, and so on) unless you type another number; type
   `2,4` to join a port to two pads, as a regulator's tab is joined to its middle pin.
   A pad that doesn't exist is flagged in red ("no pad 9"), and pads joined to no port
   are listed.
4. **The drawing** below shows the footprint as it will go on the board: silkscreen,
   copper pads (pad 1 square on through-hole parts) with their holes, each pad's
   number on it and its port above it.

Changes are made as you leave a field and can be undone. **Remove Footprint** takes it
off again. On disk the footprint is `footprint.ron` in the cell's folder:

```ron
(package: "dip", params: {"pins": "8"}, pins: {"vcc": "8", "gnd": "1"})
```

**Drawing a footprint of your own**

For a pad layout no package makes (a CPU socket, an odd connector, a module), choose
**Draw my own…** at the top of the package list, or **Edit Pads One by One…** under a
package's drawing to start from its pads. The footprint editor opens in a window of its
own, with the footprint as it stands, kept apart from the part until you **Save** it
(**Cancel** leaves the part as it was; saving can be undone like any edit).

| Action | How |
|---|---|
| Place pads | The **Pad** tool: click to place a pad like the one set under **New pads** (size, shape, through-hole, surface mount or an unplated hole, drill, turn), numbered after the highest number so far |
| Arrays | Under **Array**: rows, columns, the pitch across and down, and how they're numbered: **along rows** (1 2 3 / 4 5 6), **down columns** (1 3 5 / 2 4 6, as a double-row header) or **A1, A2…** as a ball grid (a letter per row, I, O, Q, S, X and Z left out, AA after Y). **Place Array** puts them centred on the origin, selected, ready to drag where they go |
| Select | The **Select** tool: click a pad (Shift adds or takes away), or drag a box round several; click a line to select it |
| Move | Drag the selected pads; they move on the grid (0.05 mm to 2.54 mm) |
| Change | With one pad selected: its number and its X and Y. With any selected: size, shape, kind, drill and turn, set on all of them at once. `R` turns them 90° |
| Silkscreen | The **Line** and **Rectangle** tools: click where a line starts and ends, or two opposite corners. Select one to put it on the silkscreen, fab or courtyard layer, or change its width |
| Courtyard | **Courtyard Round the Pads** draws a rectangle 0.25 mm clear of the pads and the drawing, replacing any other; one is added on saving if there's none, since the rules check needs it |
| Delete, undo | `Delete` removes what's selected; `Ctrl+Z` and `Ctrl+Y` (or the arrows at the top) undo and redo within the editor; `Esc` goes back to Select |
| View | The wheel zooms about the pointer, right or middle drag pans; the pointer's X and Y show below. The origin (the cross) is where the part is placed on the board |

Two pads with the same number are joined (as a regulator's tab is to its pin); the
editor warns of numbers used twice in case that isn't meant. Once saved, the ports are
joined to the pads by number in the Footprint section, as for a package.

**Saving and reusing components**

`Ctrl+S` saves every changed cell into the project folder, one folder per cell under
`cells/` (folders you make in the Project tree become folders there):
`cell.ron` (ports and parameters), `symbol.ron`, `schematic.ron`, `footprint.ron`,
`model3d.ron` (its 3D body) and `board.ron` (its board, if it has one). Your cells are
listed first in Components, ready to place in any sheet of the project.

To use them in other projects, keep them in a library: a folder with its own `cells/`
inside, named in each project's `circuiter.toml`:

```toml
[libraries]
mylib = { path = "../my-parts" }
```

Its parts then appear under **Libraries** and are placed as `mylib:part`.

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
| Select several | Drag a box round parts, or Shift+click them, and drag them together. Tools → Align lines them up by their left, right, top or bottom edges or centres, or spaces them evenly across or down |
| Packages | Resistors, capacitors, diodes, LEDs, transistors, op-amps, the 555 and logic gates each have a Package parameter: through-hole (axial, disc, TO-92, DIP) or surface mount (0603, 0805, 1206, SOD-123, SOT-23, SOIC), with KiCad's footprint names and 3D bodies that sit on their pads |
| Keepouts | The keepout tool draws an area that bars tracks, vias, pours or parts, on chosen layers or all; the autorouter goes round it and the rules check flags anything inside |
| Rearrange tracks | Drag a track: it slides square to itself and the tracks either side stretch to meet it at their own angles (a new leg joins it to a pad). Drag a corner where tracks meet, or a via, and the tracks there follow. `Delete` removes the piece selected |
| Routing modes | As in Altium: `Shift+R` cycles Push and shove (the default: other nets' tracks in the way are pushed aside, each pushing the next, just far enough to keep their clearance, and shown where they'll go as you move; where nothing can be pushed, as when crossing a track or next to a pad, it walks round instead), Walkaround (the track finds its way round other nets' copper as you move, live), Stop at obstacle (it stops short of the first thing in the way) and Ignore (straight over; the rules check flags it). `Shift+Space` cycles 45°, 90° and any-angle corners, `Space` flips which leg comes first, `3` cycles the track width (minimum, preferred, double). **Ctrl+click** finishes the connection by itself, through vias if it must. Finishing a route that makes a second way between two points takes the old way off (loop removal). The status line gives the route's length |
| Autoroute | Autoroute → Route All turns every ratsnest line into tracks: shortest first, at 0°, 45° and 90°, keeping each net's clearance, across every copper layer with vias where it must change side (the front prefers to run across, the back up and down), each path pulled as straight as it can go. Route the Selection's Nets does only the nets of the selected part, track or via. Whatever can't get through stays on the ratsnest, named |
| Tidy up | Autoroute → Clean Up Tracks joins segments in a line and takes off stubs and duplicates; Unroute takes a net's tracks off, or all of them |
| Route | `X` (or the route tool), click a pad, then click to lay each corner: tracks run at 0°, 45° and 90° (`/` swaps which leg comes first), at the net's width, and a track that would come too close to other copper shows red with its clearance outline. Click a pad, track or via of the same net to finish. `V` (or the toolbar's Via) drops a via and carries on on the other side; clicking another layer's button in the toolbar (or the panel, or `Page Up`/`Page Down`) mid-route drops a via and carries on on that one, inner layers included; `Backspace` takes back the last leg; `Esc` stops |
| Layers | The toolbar has a button for each copper layer in its colour (click to work on it), **All / Dim / Only** for how the other layers show, and **Realistic**: the board as it's made, green solder mask over the copper, gold pads and a white legend, the side being worked on. The Layers panel beside the board lists every layer in its colour: click a copper layer to work on it, the eye to show or hide any layer (silkscreen, courtyards, fab drawings, the edge), and choose 2, 4 or 6 copper layers. Pours, ratsnest and rule markers switch on and off there too, and Dim other layers fades everything but the layer in hand. `Page Up` / `Page Down` step through the copper layers |
| Copper pour | The zone tool, pick its net (GND by default), click its corners, double-click to finish. It fills to the board's edge, keeps the clearance from other nets, joins its own pads through thermal spokes and drops islands; select it to set priority, pad connection, relief gap and spoke width |
| Board shape | Width and height in Properties, the outline tool to draw any shape, or Import Outline… from a DXF or SVG drawing. With the select tool the outline has handles: drag a corner's square to move it, drag the dot mid-edge to bend that edge into an arc, double-click the edge to add a corner, select a corner and `Delete` to remove it (Properties shows its X, Y and the radius of the edge after it). **Round** gives every corner a radius, **Straighten** takes the arcs out |
| Cutouts and holes | The cutout tool draws an opening of any shape through the board; the hole tool drills round mounting holes (3.2 mm by default, for M3). Both are Edge.Cuts, so pours, tracks and the router keep clear of them, the rules check flags copper crossing them, and they're in the Gerbers and the 3D board. Select one to move it, change a hole's diameter, or delete it |
| Rules | Properties → Design rules: clearance, track width, via size, the board house's minimums, edge clearance, hole spacing |
| Nets | The Nets tab under the board lists every net, as Altium's PCB panel does: its colour (click the swatch; `F5` shows net colours on the routing), class, pads, routed length (tracks plus the board's thickness per via, yellow when shorter than its class allows, red when longer), vias and connections still to route. Click a net to show it |
| Net classes | The Net Classes tab: classes with their own clearance, track width, via, minimum and maximum routed length and impedance profile; put a net in one from the Nets tab. Selecting a part on the schematic selects it on the board, and the other way round |
| Differential pairs | The Diff Pairs tab: two nets carrying one signal (USB's D+/D−, LVDS), routed side by side at a set width and gap. From Net Names finds them by name (`X_P`/`X_N`, `X+`/`X-`, `XP`/`XN`); Route routes a pair as one, from one pair of pads to the other; the tab shows each half's length and the skew between them, and the rules check flags skew over the pair's limit |
| Stack-up | The Stack-up tab (Altium's Layer Stack Manager): each layer's material, thickness and dielectric constant, starting from a standard FR-4 stack; impedance profiles (50 Ω single, 100 Ω pairs, any layer) work out the track width they need by IPC-2141's formulas, microstrip outside and stripline inside, and size a net class's tracks |
| Tools | Optimise Placement moves and turns the unlocked parts to shorten their connections. Add Teardrops widens every track where it meets a pad or via (left out where it would break a clearance, and following the routing as it changes). Via Stitching adds vias of a net on a grid wherever they fit in its pours. Tune Length lays an accordion of mitered bumps along the selected segment to bring its net to a length. Measure (`Ctrl+M`) shows a distance and its X and Y. Power Analysis (below). Others: `Shift+S` shows the working layer alone |
| Power analysis | Tools → Power Analysis…, after an operating point (`F9`), as Altium's PDN Analyzer: each net's copper becomes resistors (tracks by length, width and copper weight, vias by their plated barrels, pours as sheets), the simulated current of every pin goes in at its pad, and solving it gives every track's current and the voltage lost on the way to each pad. Nets are listed worst drop first with their peak current and the tracks carrying more than IPC-2221 allows for the rise you set; the shown net's tracks are coloured cool to hot by how hard they're worked, and Widen sizes the overloaded ones. A calculator gives the width a current needs, outside and in |
| Variants | Properties → Variants: the same board built differently (a cheaper model, a test build). Pick the variant being built, then select a part to untick Fitted or give it another value. The BOM, placement file, 3D view and assembly drawing follow it, and Fabrication Files writes each variant's BOM and placement file too |
| Panel | Properties → Panel: copies across and down, rails top and bottom with three fiducials and four tooling holes, separated by V-scores (rectangular boards, butted) or mouse bites (a routed gap held by drilled tabs). Fabrication Files then writes the panel's own Gerbers, drills and placement file, with a picture of it |
| Update | Update from Schematic opens the Engineering Change Order, as in Altium: the parts to add, the footprints whose parts are gone, the routing of nets that no longer exist, each ticked to make or unticked to leave |
| Undo | `Ctrl+Z`, as anywhere |

The design rule check runs as you go and lists, under Design Rules below the board,
shorts, clearances, narrow tracks, small drills, thin annular rings, copper too near
the edge, holes too close, overlapping courtyards, parts' 3D bodies running into each
other (a tall capacitor leaning over its neighbour, though their footprints keep
clear), lengths outside a class's limits, pair skew, silkscreen on pads, anything in a
keepout, and unrouted connections; click one to go there. The status line counts what's unrouted and what's wrong.

**Fabrication files** writes, into a folder you choose: a Gerber file per layer
(RS-274X with X2 attributes: copper, solder mask, paste, silkscreen, board outline),
Excellon drill files for plated and unplated holes, the placement file for the
assembly machine, the bill of materials (grouped, with MPN and manufacturer, DNP parts
apart), an SVG picture of the board, the IPC-D-356 netlist for the bare-board
electrical test, and two drawings as Altium's Draftsman makes them: the fabrication
drawing (the board dimensioned, every hole marked with its tool's symbol, the drill
table, the layer stack and the board house's notes, in a title block) and the assembly
drawing (each side's parts by outline and reference, pin 1 marked, parts left off
dashed, beside the bill of materials). A panel and variants add their files. Tools → Export KiCad Netlist… writes the
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
| Extrude | An outline you draw, raised to a height (negative goes the other way); its top and bottom edges can be filleted or chamfered |
| Revolve | An outline turned about the vertical axis, all the way or part way |
| Sweep | An outline carried along a path you draw |
| Loft | One outline at the bottom joined to another at a height |
| Shell | An earlier feature hollowed to a wall thickness, open at the top, bottom or both, or closed |
| Hole | A cylinder that cuts |
| Copies, Ring, Mirror | Copies of an earlier feature in a row, around the axis, or reflected |

Each feature has a placement (where, and turned about each axis) and a colour, and can
be hidden, reordered or deleted. Dimensions are expressions in millimetres that may
use the cell's parameters (`width - 2*wall`), so changing a parameter reshapes the
part. Every change can be undone.

**Templates**: the Body panel's **Templates** menu starts a part from a ready-made one,
replacing what's there: DIP-8, SOIC-8, QFP-44, a 121-ball BGA, 1×8 and 2×8 pin headers,
a PCIe x16 slot, a DDR4 DIMM slot, a USB-A receptacle, a TO-220, an electrolytic
capacitor and a heatsink. They're ordinary features, so everything stays editable: a
lead is a feature called Lead (its side profile, bent down through the board or
gull-winged onto it, extruded to the lead's width), Leads is its copies (the count is
the pins per side, the step the pitch), and the "other side" features mirror them.
Parts are centred on the origin with their rows of pins along Y, standing on the
board's top, as the footprints have them.

**Sketching**: an extrusion, revolve, sweep or loft is drawn in a sketch editor that
takes most of the panel while it's open (**Narrow** keeps it beside the model). It
works as a 2D CAD program does, from its tool buttons or by typing AutoCAD's commands
at the command line under the drawing:

| Tools | |
|---|---|
| Draw | Line `L` (`C` closes, Enter ends), arc through three points `A`, arc from its centre `AC`, circle `C`, rectangle `REC`, polygon `POL`, construction line `XL` |
| Change | Trim `TR`, extend `EX`, offset `O`, mirror `MI`, fillet `F` and chamfer `CHA` where two lines meet, dimension `DIM`; drag points, `Delete` removes what's selected |
| Aids | Object snaps (`F3`: ends, middles, centres, crossings, nearest), grid (`F9`), ortho (`F8`), polar tracking (`F10`, `polar 30` for another step), `Z` fits the view |
| Typed points | `x,y`, `@dx,dy`, `@length<angle`, or just a length along the pointer's direction; numbers may use the cell's parameters |
| Constraints | With things selected: horizontal, vertical, parallel, perpendicular, tangent, equal, coincident or on a curve, fixed; dimensions of length, radius, angle and distances, as numbers or expressions. They're shown on the drawing and listed to change or remove, and the part follows them |

Closed outlines make the profile; outlines inside others are holes.

**Sketching on the body**: **Sketch on Face** in the 3D view's toolbar, then click any
face of the body (the face under the pointer is marked): a sketch starts on that face,
a rectangle to redraw, raised 2 mm from it. **Cut into Face** does the same but cuts
into the body, for pockets, slots and holes. `Esc` stops picking.

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

**AI assistants (MCP)**

`circuiter mcp [PROJECT]` serves the Model Context Protocol on stdin and stdout, so an
assistant can design with you: make components, place and wire parts, simulate, and
lay out boards. Register it once, for Claude Code:

```sh
claude mcp add circuiter -- circuiter mcp /path/to/project
```

Leave the project out to have the assistant open or create one (`open_project`,
`create_project`). Its tools:

| Tools | What they do |
|---|---|
| `project_info`, `search_parts`, `get_cell` | The project's cells; parts to use, with their ports and parameters; any cell's views as the RON its files hold |
| `create_component`, `update_component`, `delete_cell`, `import_spice`, `set_top` | Components from ports and parameters, with a symbol drawn from them and any view given as RON (SPICE model, footprint, 3D body); manufacturer SPICE models |
| `place_parts`, `connect`, `disconnect`, `edit_parts`, `delete_parts`, `draw` | Parts on a sheet; pins joined into named nets (`GND` and supply names go global); wires, labels, ports and notes drawn directly |
| `arrange_sheet`, `set_frame`, `describe_schematic` | The sheet laid out as the Diagram view does, with wires and power symbols; a frame and title block; the sheet as parts and nets |
| `check`, `simulate` | Cell checks and ERC; operating point, transient (logic runs for digital designs) and AC sweeps |
| `pcb_new_board`, `pcb_update`, `pcb_place`, `pcb_autoroute`, `pcb_pour`, `pcb_check` | A board placed from the schematic, routed, poured and checked |
| `list_files`, `read_file`, `write_file` | Programs and HDL sources in the project; design files are checked and put back if they would break it |

Every change is saved at once. An open Circuiter window watches the project's files
and takes in what changes, cell by cell: the status bar says what came in, and
`Ctrl+Z` puts a cell back. A cell you have also changed and not saved keeps your
version, which Save writes over the file.

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
| `crates/circuiter-app` | The `circuiter` desktop app (egui on wgpu), the only binary; `circuiter mcp` serves the design tools to AI assistants |
| `examples/` | Example projects |
| `docs/` | Architecture and design notes |

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in Circuiter by you, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.
