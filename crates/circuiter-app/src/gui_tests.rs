//! The real app driven with real pointer events, frame by frame, through egui_kittest:
//! what the workspace tests can't see, such as how mouse input becomes a drag.

use crate::Args;
use crate::app::CircuiterApp;
use circuiter_scene::Pt;
use eframe::egui::{self, Pos2, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::path::{Path, PathBuf};

fn example_copy(name: &str) -> tempfile::TempDir {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let p = entry.unwrap().path();
            let dest = to.join(p.file_name().unwrap());
            if p.is_dir() { copy(&p, &dest) } else { std::fs::copy(&p, &dest).map(drop).unwrap() }
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    copy(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name), tmp.path());
    tmp
}

fn open(project: PathBuf) -> Harness<'static, CircuiterApp> {
    let args = Args {
        project: Some(project),
        cell: None,
        theme: None,
        view: None,
        select: None,
        simulate: None,
        heat: false,
        program: None,
        live: false,
        results: None,
        screenshot: None,
    };
    let mut h = Harness::builder()
        .with_size(Vec2::new(1600.0, 1000.0))
        .build_eframe(move |cc| CircuiterApp::new(cc, args));
    h.run_steps(4);
    h
}

/// Where sheet point `p` is on screen.
fn screen(h: &Harness<'_, CircuiterApp>, p: Pt) -> Pos2 {
    let app = h.state();
    let ws = app.workspace().expect("open");
    let view = app.canvas_view().expect("drawn");
    ws.current().camera.expect("fitted").to_screen(view, p)
}

/// Press at `from`, move in small steps to `to`, release: what a hand on a mouse does.
fn drag(h: &mut Harness<'_, CircuiterApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.step();
    h.drag_at(from);
    h.step();
    for k in 1..=10 {
        h.hover_at(from + (to - from) * (k as f32 / 10.0));
        h.step();
    }
    h.event(egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    h.step();
}

#[test]
fn dragging_a_part_with_the_mouse_moves_it() {
    let tmp = example_copy("cmos_inverter");
    let mut h = open(tmp.path().to_path_buf());
    let (index, before, body) = {
        let ws = h.state().workspace().unwrap();
        let s = ws.schematic().unwrap();
        let i = s.instances.iter().position(|i| i.name == "CL").unwrap();
        (i, s.instances[i].placement.position, ws.scene.instance_bounds[i].center())
    };
    let from = screen(&h, body);
    drag(&mut h, from, from + Vec2::new(120.0, 0.0));
    let ws = h.state().workspace().unwrap();
    let after = ws.schematic().unwrap().instances[index].placement.position;
    assert_ne!(after, before, "the part didn't move");
    assert!(after.x > before.x && after.y == before.y, "{before} → {after}");
}

#[test]
fn every_part_in_every_example_can_be_dragged() {
    for name in ["cmos_inverter", "ce_amplifier", "logic", "overstress"] {
        let tmp = example_copy(name);
        let mut h = open(tmp.path().to_path_buf());
        let n = h.state().workspace().unwrap().schematic().unwrap().instances.len();
        let mut stuck = Vec::new();
        for i in 0..n {
            let (inst, before, body) = {
                let ws = h.state().workspace().unwrap();
                let s = ws.schematic().unwrap();
                let inst = &s.instances[i];
                (inst.name.clone(), inst.placement.position, ws.scene.instance_bodies[i].center())
            };
            let from = screen(&h, body);
            drag(&mut h, from, from + Vec2::new(0.0, 80.0));
            let after =
                h.state().workspace().unwrap().schematic().unwrap().instances[i].placement.position;
            if after == before {
                stuck.push(inst);
            } else {
                h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
                h.run_steps(2);
            }
        }
        assert!(stuck.is_empty(), "{name}: couldn't drag {stuck:?}");
    }
}

/// Places a resistor the way a person does (A, type its name, Enter, click), then drags it.
#[test]
fn a_part_just_placed_can_be_dragged() {
    let tmp = example_copy("cmos_inverter");
    let mut h = open(tmp.path().to_path_buf());
    let before_count = h.state().workspace().unwrap().schematic().unwrap().instances.len();
    h.key_press(egui::Key::A);
    h.run_steps(2);
    h.event(egui::Event::Text("resistor".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    let (spot, _) = on_sheet(&h, 100.0, 120.0);
    h.hover_at(spot);
    h.step();
    h.drag_at(spot);
    h.step();
    h.event(egui::Event::PointerButton {
        pos: spot,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(3);
    let (i, before, body) = {
        let ws = h.state().workspace().unwrap();
        let s = ws.schematic().unwrap();
        assert_eq!(s.instances.len(), before_count + 1, "placed");
        assert_eq!(ws.tool.name(), "Select", "placing one part returns to Select");
        let i = s.instances.len() - 1;
        (i, s.instances[i].placement.position, ws.scene.instance_bodies[i].center())
    };
    let from = screen(&h, body);
    drag(&mut h, from, from + Vec2::new(100.0, 0.0));
    let ws = h.state().workspace().unwrap();
    let s = ws.schematic().unwrap();
    assert_eq!(s.instances.len(), before_count + 1, "the drag placed nothing more");
    let after = s.instances[i].placement.position;
    assert!(after.x > before.x && after.y == before.y, "{before} → {after}");
}

/// Deleting the part under the mouse: the status bar described it from the last frame's
/// hover after it was gone.
#[test]
fn deleting_the_part_under_the_mouse() {
    let tmp = example_copy("cmos_inverter");
    let mut h = open(tmp.path().to_path_buf());
    let n = h.state().workspace().unwrap().schematic().unwrap().instances.len();
    for left in (0..n).rev() {
        // The last part, so its index is past the end once it's gone.
        let body = h.state().workspace().unwrap().scene.instance_bodies[left].center();
        let at = screen(&h, body);
        h.hover_at(at);
        h.step();
        h.drag_at(at);
        h.step();
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.step();
        h.key_press(egui::Key::Delete);
        h.run_steps(3);
        let ws = h.state().workspace().unwrap();
        assert_eq!(ws.schematic().unwrap().instances.len(), left, "deleted");
    }
}

/// Press on `from`, move to `to` in steps and let go, for drags that start outside the
/// sheet (the library panel, the parts strip).
fn drag_across(h: &mut Harness<'_, CircuiterApp>, from: Pos2, to: Pos2) {
    drag(h, from, to);
}

/// A point on the sheet's visible area, `dx`, `dy` pixels from its centre, and the
/// grid point under it.
fn on_sheet(h: &Harness<'_, CircuiterApp>, dx: f32, dy: f32) -> (Pos2, circuiter_model::Point) {
    let app = h.state();
    let view = app.canvas_view().unwrap();
    let at = view.center() + Vec2::new(dx, dy);
    let world = app.workspace().unwrap().current().camera.unwrap().to_world(view, at);
    let grid = crate::edit::snap(crate::workspace::to_point(world), crate::edit::GRID);
    (at, grid)
}

fn folder(name: &str) -> String {
    crate::app::folder_label(name)
}

fn instance_count(h: &Harness<'_, CircuiterApp>) -> usize {
    h.state().workspace().unwrap().schematic().unwrap().instances.len()
}

#[test]
fn parts_can_be_dragged_onto_the_sheet() {
    let tmp = example_copy("cmos_inverter");
    let mut h = open(tmp.path().to_path_buf());
    let before = instance_count(&h);
    // From the parts strip…
    let from = h.get_by_label("Place capacitor").rect().center();
    let (to, grid) = on_sheet(&h, 120.0, 150.0);
    drag_across(&mut h, from, to);
    assert_eq!(instance_count(&h), before + 1);
    {
        let ws = h.state().workspace().unwrap();
        let s = ws.schematic().unwrap();
        let c = s.instances.last().unwrap();
        assert_eq!(c.cell.to_string(), "std:capacitor");
        assert_eq!(c.placement.position, grid, "where it was let go");
        assert_eq!(ws.tool.name(), "Select");
    }
    // …and from the components panel, out of a category.
    h.get_by_label(&folder("Gates")).click();
    h.run_steps(3);
    let from = h.get_by_label("Add nand").rect().center();
    let (to, _) = on_sheet(&h, -150.0, 150.0);
    drag_across(&mut h, from, to);
    assert_eq!(instance_count(&h), before + 2);
    let ws = h.state().workspace().unwrap();
    let last = ws.schematic().unwrap().instances.last().unwrap();
    assert_eq!(last.cell.to_string(), "std:nand");
    assert_eq!(ws.current().cell.cell, "testbench", "still on the same sheet");
}

#[test]
fn dropping_a_part_off_the_sheet_places_nothing() {
    let tmp = example_copy("cmos_inverter");
    let mut h = open(tmp.path().to_path_buf());
    let before = instance_count(&h);
    let from = h.get_by_label("Place resistor").rect().center();
    let over = screen(&h, Pt::new(0.0, 0.0));
    // Over the sheet, then back off it onto the inspector before letting go.
    let off = Pos2::new(1500.0, 500.0);
    h.hover_at(from);
    h.step();
    h.drag_at(from);
    h.step();
    for k in 1..=8 {
        h.hover_at(from + (over - from) * (k as f32 / 8.0));
        h.step();
    }
    assert_eq!(h.state().workspace().unwrap().tool.name(), "Place", "previewing");
    drag(&mut h, over, off);
    assert_eq!(instance_count(&h), before);
    assert_eq!(h.state().workspace().unwrap().tool.name(), "Select");
}

#[test]
fn a_cell_dragged_onto_a_folder_moves_into_it() {
    let tmp = example_copy("cmos_inverter");
    let mut h = open(tmp.path().to_path_buf());
    let lib = h.state().workspace().unwrap().project.name().to_owned();
    h.state_mut().workspace_mut().unwrap().new_folder(&lib, "logic").unwrap();
    h.run_steps(3);
    let from = h.get_by_label("inverter").rect().center();
    let to = h.get_by_label(&folder("logic")).rect().center();
    drag_across(&mut h, from, to);
    let ws = h.state().workspace().unwrap();
    let cell = ws.design.cell(&circuiter_model::CellId::new(&lib, "inverter")).unwrap();
    assert_eq!(cell.folder, "logic");
    assert_eq!(ws.current().cell.cell, "testbench", "dragging didn't open it");
}

#[test]
fn back_returns_from_a_library_symbol() {
    let tmp = example_copy("cmos_inverter");
    let mut h = open(tmp.path().to_path_buf());
    h.get_by_label("Libraries").click();
    h.run_steps(3);
    // The library is long: scroll down to the transistors with the wheel, then open.
    let wheel = |h: &mut Harness<'_, CircuiterApp>| {
        h.hover_at(Pos2::new(120.0, 300.0));
        h.run_steps(2);
        for _ in 0..4 {
            h.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -200.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            });
            h.run_steps(4);
        }
    };
    wheel(&mut h);
    h.get_by_label(&folder("transistors")).click();
    h.run_steps(3);
    wheel(&mut h);
    h.get_by_label("nmos").click();
    h.run_steps(3);
    assert_eq!(h.state().workspace().unwrap().current().cell.cell, "nmos");
    h.get_all_by_label_contains("Back to").next().unwrap().click();
    h.run_steps(3);
    assert_eq!(h.state().workspace().unwrap().current().cell.cell, "testbench");
    // The mouse's back button works too.
    h.get_by_label("nmos").click();
    h.run_steps(3);
    let at = Pos2::new(800.0, 500.0);
    h.event(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Extra1,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(3);
    assert_eq!(h.state().workspace().unwrap().current().cell.cell, "testbench");
}

#[test]
fn copy_and_paste_with_the_keyboard() {
    let tmp = example_copy("cmos_inverter");
    let mut h = open(tmp.path().to_path_buf());
    let before = instance_count(&h);
    let body = {
        let ws = h.state().workspace().unwrap();
        let i = ws.schematic().unwrap().instances.iter().position(|i| i.name == "CL").unwrap();
        ws.scene.instance_bodies[i].center()
    };
    let at = screen(&h, body);
    h.hover_at(at);
    h.step();
    h.drag_at(at);
    h.step();
    h.event(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(2);
    h.event(egui::Event::Copy);
    h.run_steps(2);
    let text = h.state_mut().workspace_mut().unwrap().copy().unwrap();
    h.hover_at(screen(&h, Pt::new(40.64, 30.48)));
    h.step();
    h.event(egui::Event::Paste(text));
    h.run_steps(2);
    assert_eq!(instance_count(&h), before + 1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::D);
    h.run_steps(2);
    assert_eq!(instance_count(&h), before + 2);
    let ws = h.state().workspace().unwrap();
    let names: Vec<&str> =
        ws.schematic().unwrap().instances[before..].iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["C1", "C2"]);
}

/// Where pin `port` of part `name` is on screen.
fn pin_on_screen(h: &Harness<'_, CircuiterApp>, name: &str, port: &str) -> Pos2 {
    let ws = h.state().workspace().unwrap();
    let s = ws.schematic().unwrap();
    let i = s.instances.iter().position(|inst| inst.name == name).unwrap();
    let pin = ws.scene.connectivity.pins.iter().find(|p| p.instance == i && p.port == port);
    screen(h, Pt::from(pin.unwrap().at))
}

fn type_and_enter(h: &mut Harness<'_, CircuiterApp>, text: &str) {
    h.run_steps(2);
    h.event(egui::Event::Text(text.into()));
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
}

fn click_at(h: &mut Harness<'_, CircuiterApp>, at: Pos2) {
    h.hover_at(at);
    h.step();
    h.drag_at(at);
    h.step();
    h.event(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(3);
}

/// Everything by hand: a new cell, parts dragged in, wired pin to pin, ports added,
/// then the new part dragged onto another sheet.
#[test]
fn a_new_component_built_by_hand() {
    let tmp = example_copy("cmos_inverter");
    let mut h = open(tmp.path().to_path_buf());
    h.get_all_by_label("New cell  (Ctrl+N)").next().unwrap().click();
    type_and_enter(&mut h, "pair");
    assert_eq!(h.state().workspace().unwrap().current().cell.cell, "pair");

    for dx in [-120.0, 120.0] {
        let from = h.get_by_label("Place resistor").rect().center();
        let (to, _) = on_sheet(&h, dx, 0.0);
        drag(&mut h, from, to);
    }
    assert_eq!(instance_count(&h), 2);
    let (from, to) = (pin_on_screen(&h, "R1", "b"), pin_on_screen(&h, "R2", "a"));
    drag(&mut h, from, to);
    {
        let ws = h.state().workspace().unwrap();
        assert_eq!(ws.schematic().unwrap().wires.len(), 1, "wired pin to pin");
        assert_eq!(ws.scene.connectivity.dangling_pins.len(), 2);
    }
    // A port on each free end: P, a name, Enter, click the pin's end.
    for (pin, name) in [(("R1", "a"), "x"), (("R2", "b"), "y")] {
        let at = pin_on_screen(&h, pin.0, pin.1);
        h.key_press(egui::Key::P);
        type_and_enter(&mut h, name);
        click_at(&mut h, at);
    }
    {
        let ws = h.state().workspace().unwrap();
        let def = ws.current_def().unwrap();
        let ports: Vec<&str> = def.cell.ports.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(ports, ["x", "y"]);
        assert!(def.symbol.is_some(), "a symbol was generated");
        assert!(ws.scene.connectivity.dangling_pins.is_empty(), "{:?}", ws.problems());
    }

    // Back on the testbench, the new part is in Components, ready to drag on.
    h.get_all_by_label("testbench").next().unwrap().click();
    h.run_steps(3);
    let before = instance_count(&h);
    let from = h.get_by_label("Add pair").rect().center();
    let (to, _) = on_sheet(&h, 150.0, 150.0);
    drag(&mut h, from, to);
    assert_eq!(instance_count(&h), before + 1);
    let ws = h.state().workspace().unwrap();
    assert_eq!(ws.schematic().unwrap().instances.last().unwrap().cell.to_string(), "pair");
}

/// A program edited in its tab and run on the processor on the sheet.
#[test]
fn a_program_runs_on_the_cpu_from_its_editor() {
    let tmp = example_copy("riscv");
    let mut h = open(tmp.path().to_path_buf());
    h.get_by_label("fib.s").click();
    h.run_steps(3);
    // Change how many numbers it makes: 12 becomes 10, so a0 ends as F(10) = 34.
    {
        let app = h.state_mut();
        let tab = app.code_tab_mut("programs/fib.s").expect("open");
        tab.text = tab.text.replace("li   t3, 12", "li   t3, 10");
    }
    h.run_steps(2);
    h.get_by_label_contains("Run on cpu").click();
    for _ in 0..2000 {
        h.step();
        let busy = h.state().workspace().unwrap().sim.as_ref().is_some_and(|s| s.job.is_some());
        if !busy {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    h.run_steps(2);
    let saved = std::fs::read_to_string(tmp.path().join("programs/fib.s")).unwrap();
    assert!(saved.contains("li   t3, 10"), "saved before running");
    let ws = h.state().workspace().unwrap();
    let sim = ws.sim.as_ref().expect("ran");
    assert_eq!(sim.error, None);
    let Some(crate::simulation::Results::Logic(run)) = &sim.results else {
        panic!("no digital run")
    };
    assert!(run.halted);
    assert_eq!(run.memories["RF"][10].known(), Some(34));
}

/// Running live, a click on a switch flips it (and doesn't select it).
#[test]
fn clicking_a_switch_while_running_live_flips_it() {
    let tmp = example_copy("bench");
    let mut h = open(tmp.path().to_path_buf());
    h.get_by_label_contains("Run Live").click();
    h.run_steps(4);
    assert!(h.state().workspace().unwrap().live.is_some(), "running live");
    let body = {
        let ws = h.state().workspace().unwrap();
        let i = ws.schematic().unwrap().instances.iter().position(|i| i.name == "SW1").unwrap();
        ws.scene.instance_bodies[i].center()
    };
    let at = screen(&h, body);
    h.hover_at(at);
    h.run_steps(2);
    h.drag_at(at);
    h.step();
    h.event(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(3);
    let mut flipped = false;
    for _ in 0..100 {
        h.step();
        let ws = h.state().workspace().unwrap();
        if ws.live_states().get("SW1").map(String::as_str) == Some("true") {
            flipped = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(flipped, "the switch closed");
    let ws = h.state().workspace().unwrap();
    assert!(ws.selection.is_empty(), "operating a part doesn't select it");
    // Under load a frame can go by before the click lands: try a few times.
    for _ in 0..5 {
        h.get_by_label_contains("Stop Live").click();
        h.run_steps(3);
        if h.state().workspace().unwrap().live.is_none() {
            break;
        }
    }
    assert!(h.state().workspace().unwrap().live.is_none(), "stopped");
}

/// Modelling a body in the 3D view: add a hole from the menu, undo it, hide a feature.
#[test]
fn modelling_a_body_adds_undoes_and_hides_features() {
    let tmp = example_copy("analog_lab");
    let args = Args {
        project: Some(tmp.path().to_owned()),
        cell: Some("case_lid".into()),
        theme: None,
        view: Some("3d".into()),
        select: None,
        simulate: None,
        heat: false,
        program: None,
        live: false,
        results: None,
        screenshot: None,
    };
    let mut h = Harness::builder()
        .with_size(Vec2::new(1600.0, 1000.0))
        .build_eframe(move |cc| CircuiterApp::new(cc, args));
    h.run_steps(4);
    let features = |h: &Harness<'_, CircuiterApp>| {
        let ws = h.state().workspace().expect("open");
        ws.current_def().and_then(|d| d.model3d.clone()).expect("a body").features
    };
    let before = features(&h).len();
    h.get_by_label(&format!("{}  Add feature", crate::style::icon::PLUS)).click();
    h.run_steps(2);
    h.get_by_label("Hole").click();
    h.run_steps(3);
    let after = features(&h);
    assert_eq!(after.len(), before + 1);
    let hole = after.last().unwrap();
    assert_eq!(hole.name, "Hole");
    assert_eq!(hole.operation, circuiter_model::model3d::Operation::Cut);
    // Undo takes it away again.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(3);
    assert_eq!(features(&h).len(), before);
    // The eye beside the first feature leaves it out.
    h.get_all_by_label(crate::style::icon::EYE).next().expect("an eye").click();
    h.run_steps(3);
    assert!(features(&h)[0].hidden, "the plate is left out");
}
