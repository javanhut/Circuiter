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

/// The cube beside New Cell makes a 3D part and opens it in the 3D view to model.
#[test]
fn a_new_3d_part_opens_in_the_modeller() {
    let tmp = example_copy("cmos_inverter");
    let mut h = open(tmp.path().to_path_buf());
    h.get_by_label("New 3D part: a body to model, as in a CAD program").click();
    h.run_steps(2);
    h.event(egui::Event::Text("bracket".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let ws = h.state().workspace().unwrap();
    assert_eq!(ws.current().cell.cell, "bracket");
    let body = ws.current_def().and_then(|d| d.model3d.clone()).expect("a body to shape");
    assert_eq!(body.features.len(), 1);
    assert!(h.state().showing_3d(), "it opens in the 3D view");
    // Its Body panel is there to add to.
    h.get_by_label(&format!("{}  Add feature", crate::style::icon::PLUS));
}

/// With the hand (H), a left drag moves the view, not the parts.
#[test]
fn the_hand_tool_drags_the_view() {
    let tmp = example_copy("cmos_inverter");
    let mut h = open(tmp.path().to_path_buf());
    let before = h.state().workspace().unwrap().current_def().unwrap().clone();
    let center = h.state().workspace().unwrap().current().camera.expect("fitted").center;
    h.get_by_label(
        "Hand  (H): drag to move the view. Middle or right drag, or Space+drag, also do",
    )
    .click();
    h.run_steps(2);
    let (from, _) = on_sheet(&h, 200.0, 200.0);
    drag(&mut h, from, from + Vec2::new(120.0, 40.0));
    let ws = h.state().workspace().unwrap();
    let after = ws.current().camera.expect("fitted").center;
    assert!(after.x < center.x - 1.0 && after.y < center.y - 0.3, "{center:?} → {after:?}");
    assert_eq!(ws.current_def().unwrap(), &before, "nothing on the sheet moved");
}

/// Where board point `q` (millimetres) is on screen in the PCB view.
fn on_board(h: &Harness<'_, CircuiterApp>, q: [f64; 2]) -> Pos2 {
    let view = h.state().pcb_view();
    view.camera.expect("fitted").to_screen(view.rect.expect("drawn"), Pt::new(q[0], q[1]))
}

fn board(h: &Harness<'_, CircuiterApp>) -> circuiter_model::board::Board {
    let ws = h.state().workspace().expect("open");
    ws.current_def().and_then(|d| d.board.clone()).expect("a board")
}

/// A board made from the blinker's schematic: routed with a click per corner, a via
/// to the back, parts moved, turned and flipped, and undone.
#[test]
fn a_board_is_made_routed_and_rearranged() {
    let tmp = example_copy("analog_lab");
    let args = Args {
        project: Some(tmp.path().to_owned()),
        cell: Some("blinker".into()),
        theme: None,
        view: Some("pcb".into()),
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
    // Start from no board (the example comes with one).
    h.state_mut()
        .workspace_mut()
        .unwrap()
        .edit_board(|_, _, b| {
            *b = None;
            Ok(())
        })
        .unwrap();
    h.run_steps(3);
    h.get_by_label(&format!("{}  Create Board from Schematic", crate::style::icon::SPARKLE))
        .click();
    h.run_steps(4);
    let made = board(&h);
    assert_eq!(made.footprints.len(), 8, "every part with a footprint is placed");
    let unrouted = |h: &Harness<'_, CircuiterApp>| {
        h.state()
            .pcb_view()
            .analysis
            .as_ref()
            .and_then(|a| a.checked.as_ref())
            .expect("checked")
            .connectivity
            .unrouted()
    };
    let before = unrouted(&h);
    assert!(before > 5);

    // Route the first ratsnest line: X, click its start, click its end.
    let (net, a, b) = h
        .state()
        .pcb_view()
        .analysis
        .as_ref()
        .unwrap()
        .checked
        .as_ref()
        .unwrap()
        .connectivity
        .ratsnest[0]
        .clone();
    h.hover_at(on_board(&h, a));
    h.step();
    h.key_press(egui::Key::X);
    h.run_steps(2);
    let start = on_board(&h, a);
    click_at(&mut h, start);
    assert!(
        matches!(&h.state().pcb_view().tool, crate::app::pcb::PcbTool::Route { net: n, .. } if *n == net),
        "routing {net}"
    );
    let b_screen = on_board(&h, b);
    h.hover_at(b_screen);
    h.run_steps(2);
    click_at(&mut h, b_screen);
    let routed = board(&h);
    assert!(!routed.tracks.is_empty() && routed.tracks.iter().all(|t| t.net == net));
    assert_eq!(unrouted(&h), before - 1, "the line is routed");
    assert!(
        matches!(&h.state().pcb_view().tool, crate::app::pcb::PcbTool::Route { points, .. } if points.is_empty()),
        "the route finished on the pad"
    );

    // Another: start, then V for a via and the back layer.
    let (_, c, _) = h
        .state()
        .pcb_view()
        .analysis
        .as_ref()
        .unwrap()
        .checked
        .as_ref()
        .unwrap()
        .connectivity
        .ratsnest[0]
        .clone();
    let start = on_board(&h, c);
    click_at(&mut h, start);
    let mid = on_board(&h, [c[0] + 3.0, c[1] + 3.0]);
    h.hover_at(mid);
    h.run_steps(2);
    h.key_press(egui::Key::V);
    h.run_steps(2);
    assert_eq!(board(&h).vias.len(), 1);
    assert_eq!(h.state().pcb_view().layer, circuiter_model::board::Layer::BCu);
    h.key_press(egui::Key::Escape);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().pcb_view().tool, crate::app::pcb::PcbTool::Select);

    // Drag U1 by 10 mm, turn it, flip it; undo the flip.
    let u1 = board(&h).placed("U1").unwrap().clone();
    let at = [u1.at.x.mm(), u1.at.y.mm()];
    let (from, to) = (on_board(&h, at), on_board(&h, [at[0] + 10.0, at[1]]));
    drag(&mut h, from, to);
    let moved = board(&h).placed("U1").unwrap().clone();
    assert!((moved.at.x.mm() - at[0] - 10.0).abs() < 0.4, "{} → {}", at[0], moved.at.x.mm());
    h.key_press(egui::Key::R);
    h.run_steps(2);
    assert_eq!(board(&h).placed("U1").unwrap().rotation, 90.0);
    h.key_press(egui::Key::F);
    h.run_steps(2);
    assert!(board(&h).placed("U1").unwrap().back);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(3);
    let undone = board(&h).placed("U1").unwrap().clone();
    assert!(!undone.back && undone.rotation == 90.0, "undo takes back the flip only");
}

/// Autoroute from the toolbar, then rearrange: drag a routed segment and it slides
/// with its neighbours following, still connected; pick a layer in the Layers panel.
#[test]
fn boards_autoroute_and_tracks_rearrange() {
    let tmp = example_copy("analog_lab");
    let args = Args {
        project: Some(tmp.path().to_owned()),
        cell: Some("blinker".into()),
        theme: None,
        view: Some("pcb".into()),
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
    let checked = |h: &Harness<'_, CircuiterApp>| {
        h.state()
            .pcb_view()
            .analysis
            .as_ref()
            .and_then(|a| a.checked.as_ref())
            .map(|c| (c.connectivity.unrouted(), c.errors()))
            .expect("checked")
    };
    // Take the example's tracks off, then route it again.
    h.get_by_label(&format!("{}  Autoroute", crate::style::icon::FLOW_ARROW)).click();
    h.run_steps(2);
    h.get_by_label("Unroute All").click();
    h.run_steps(4);
    assert!(board(&h).tracks.is_empty());
    let (before, _) = checked(&h);
    assert!(before > 0, "connections to route");
    h.get_by_label(&format!("{}  Autoroute", crate::style::icon::FLOW_ARROW)).click();
    h.run_steps(2);
    h.get_by_label("Route All").click();
    h.run_steps(4);
    let (unrouted, errors) = checked(&h);
    assert_eq!((unrouted, errors), (0, 0), "{:?}", h.state().pcb_view().note);

    // A long straight routed segment: drag it square to itself.
    let b = board(&h);
    let k = b
        .tracks
        .iter()
        .position(|t| {
            let (a, c) = ([t.a.x.mm(), t.a.y.mm()], [t.b.x.mm(), t.b.y.mm()]);
            let long = (a[0] - c[0]).hypot(a[1] - c[1]) > 2.0;
            ((a[0] - c[0]).abs() < 1e-6 || (a[1] - c[1]).abs() < 1e-6) && long
        })
        .expect("a straight segment");
    let t = b.tracks[k].clone();
    let vertical = (t.a.x.mm() - t.b.x.mm()).abs() < 1e-6;
    let mid = [(t.a.x.mm() + t.b.x.mm()) / 2.0, (t.a.y.mm() + t.b.y.mm()) / 2.0];
    let shift = if vertical { [1.27, 0.0] } else { [0.0, 1.27] };
    let layer = t.layer;
    // Work on its layer, so it's picked before anything on the other.
    h.state_mut().pcb_view_mut().layer = layer;
    h.run_steps(2);
    let (from, to) = (on_board(&h, mid), on_board(&h, [mid[0] + shift[0], mid[1] + shift[1]]));
    drag(&mut h, from, to);
    let after = board(&h);
    let moved = after.tracks.iter().any(|t| {
        if vertical {
            (t.a.x.mm() - mid[0] - 1.27).abs() < 0.01 && (t.b.x.mm() - mid[0] - 1.27).abs() < 0.01
        } else {
            (t.a.y.mm() - mid[1] - 1.27).abs() < 0.01 && (t.b.y.mm() - mid[1] - 1.27).abs() < 0.01
        }
    });
    assert!(moved, "the segment moved across");
    let (unrouted, _) = checked(&h);
    assert_eq!(unrouted, 0, "still connected");

    // The Layers panel: click B.Cu to work on it.
    h.get_by_label("B.Cu  (back)").click();
    h.run_steps(2);
    assert_eq!(h.state().pcb_view().layer, circuiter_model::board::Layer::BCu);
    h.get_by_label("F.Cu  (front)").click();
    h.run_steps(2);
    assert_eq!(h.state().pcb_view().layer, circuiter_model::board::Layer::FCu);
}

/// Altium-style board tools: Ctrl+click finishes a connection by itself, the Tools
/// menu optimises placement and adds teardrops, Update from Schematic says when there's
/// nothing to do, and Ctrl+M measures.
#[test]
fn board_tools_work_from_the_editor() {
    let tmp = example_copy("analog_lab");
    let args = Args {
        project: Some(tmp.path().to_owned()),
        cell: Some("blinker".into()),
        theme: None,
        view: Some("pcb".into()),
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
    let menu = |h: &mut Harness<'_, CircuiterApp>, menu: &str, item: &str| {
        h.get_by_label(menu).click();
        h.run_steps(2);
        h.get_by_label(item).click();
        h.run_steps(4);
    };
    let autoroute = format!("{}  Autoroute", crate::style::icon::FLOW_ARROW);
    let tools = format!("{}  Tools", crate::style::icon::GEAR_SIX);
    menu(&mut h, &autoroute, "Unroute All");
    let unrouted = |h: &Harness<'_, CircuiterApp>| {
        h.state()
            .pcb_view()
            .analysis
            .as_ref()
            .and_then(|a| a.checked.as_ref())
            .map(|c| (c.connectivity.unrouted(), c.errors()))
            .unwrap()
    };
    // Optimise first (nothing routed to lose), then teardrops on.
    menu(&mut h, &tools, "Optimise Placement");
    assert!(
        h.state().pcb_view().note.as_deref().is_some_and(|n| n.contains("shorter")),
        "{:?}",
        h.state().pcb_view().note
    );
    menu(&mut h, &tools, "Add Teardrops");
    assert!(board(&h).teardrops.is_some());

    // Ctrl+click a pad with a connection to make: X, then Ctrl+click.
    let (before, _) = unrouted(&h);
    let (_, a, _) = h
        .state()
        .pcb_view()
        .analysis
        .as_ref()
        .unwrap()
        .checked
        .as_ref()
        .unwrap()
        .connectivity
        .ratsnest[0]
        .clone();
    let at = on_board(&h, a);
    h.hover_at(at);
    h.step();
    h.key_press(egui::Key::X);
    h.run_steps(2);
    // Ctrl held through the click.
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
    h.step();
    h.event(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::COMMAND,
    });
    h.step();
    h.event(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::COMMAND,
    });
    h.step();
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.run_steps(4);
    let (after, errors) = unrouted(&h);
    assert_eq!(after, before - 1, "{:?}", h.state().pcb_view().note);
    assert_eq!(errors, 0);
    h.key_press(egui::Key::Escape);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);

    // Nothing for the schematic to change.
    h.get_by_label(&format!("{}  Update from Schematic", crate::style::icon::ARROWS_CLOCKWISE))
        .click();
    h.run_steps(3);
    assert_eq!(h.state().pcb_view().note.as_deref(), Some("The board matches the schematic"));

    // Ctrl+M, then two clicks.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::M);
    h.run_steps(2);
    let (p1, p2) = (on_board(&h, [10.0, 20.0]), on_board(&h, [20.0, 20.0]));
    for q in [p1, p2] {
        click_at(&mut h, q);
    }
    match &h.state().pcb_view().tool {
        crate::app::pcb::PcbTool::Measure { from: Some(f), to: Some(t) } => {
            assert!(((t[0] - f[0]) - 10.0).abs() < 0.7, "{f:?} {t:?}")
        }
        other => panic!("{other:?}"),
    }
}
