//! The server driven as an assistant drives it: JSON-RPC messages in, results out.

use super::{Server, handle};
use serde_json::{Value, json};

fn request(server: &mut Server, method: &str, params: Value) -> Value {
    let line = json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params });
    let reply = handle(server, &line.to_string()).expect("an answer");
    assert_eq!(reply["id"], 7);
    reply
}

/// Calls a tool and returns its text, failing the test if the tool fails.
fn ok(server: &mut Server, tool: &str, args: Value) -> String {
    let (text, error) = run(server, tool, args);
    assert!(!error, "{tool} failed: {text}");
    text
}

/// Calls a tool and returns its text, failing the test if the tool succeeds.
fn refused(server: &mut Server, tool: &str, args: Value) -> String {
    let (text, error) = run(server, tool, args);
    assert!(error, "{tool} should have failed: {text}");
    text
}

fn run(server: &mut Server, tool: &str, args: Value) -> (String, bool) {
    let reply = request(server, "tools/call", json!({ "name": tool, "arguments": args }));
    let result = &reply["result"];
    let text = result["content"][0]["text"].as_str().expect("text").to_owned();
    (text, result["isError"].as_bool().expect("isError"))
}

/// A server on a new, empty project.
fn fresh() -> (tempfile::TempDir, Server) {
    let tmp = tempfile::tempdir().unwrap();
    let mut server = Server::new(Some(tmp.path().to_path_buf())).unwrap_or_else(|_| {
        // No project there yet: start without one and make it.
        Server::new(None).unwrap()
    });
    ok(&mut server, "create_project", json!({ "path": tmp.path().join("amp"), "name": "amp" }));
    (tmp, server)
}

#[test]
fn answers_the_handshake_and_lists_its_tools() {
    let (_tmp, mut server) = fresh();
    let init = request(&mut server, "initialize", json!({ "protocolVersion": "2025-03-26" }));
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(init["result"]["serverInfo"]["name"], "circuiter");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    let newer = request(&mut server, "initialize", json!({ "protocolVersion": "2099-01-01" }));
    assert_eq!(newer["result"]["protocolVersion"], super::PROTOCOL);
    // Notifications get no answer.
    let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    assert!(handle(&mut server, &note.to_string()).is_none());
    assert_eq!(request(&mut server, "ping", json!({}))["result"], json!({}));

    let list = request(&mut server, "tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().unwrap();
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.len() >= 25, "{names:?}");
    for t in tools {
        let schema = &t["inputSchema"];
        assert_eq!(schema["type"], "object", "{}", t["name"]);
        assert!(t["description"].as_str().is_some_and(|d| d.len() > 20), "{}", t["name"]);
        for required in schema["required"].as_array().unwrap() {
            let key = required.as_str().unwrap();
            assert!(schema["properties"].get(key).is_some(), "{}: {key}", t["name"]);
        }
    }
    names.sort();
    names.dedup();
    assert_eq!(names.len(), tools.len(), "names are unique");
    for tool in ["create_component", "place_parts", "connect", "simulate", "pcb_autoroute"] {
        assert!(names.contains(&tool), "{tool}");
    }
}

#[test]
fn reports_protocol_mistakes_as_errors() {
    let (_tmp, mut server) = fresh();
    let unknown = request(&mut server, "resources/list", json!({}));
    assert_eq!(unknown["error"]["code"], -32601);
    let no_tool = request(&mut server, "tools/call", json!({ "name": "fly" }));
    assert_eq!(no_tool["error"]["code"], -32602);
    let garbled = handle(&mut server, "{ not json").unwrap();
    assert_eq!(garbled["error"]["code"], -32700);
    let batch = json!([
        { "jsonrpc": "2.0", "id": 1, "method": "ping" },
        { "jsonrpc": "2.0", "method": "notifications/initialized" },
        { "jsonrpc": "2.0", "id": 2, "method": "ping" },
    ]);
    let replies = handle(&mut server, &batch.to_string()).unwrap();
    assert_eq!(replies.as_array().unwrap().len(), 2);
    // A tool that fails says why, as a result the assistant can read.
    let text = refused(&mut server, "get_cell", json!({ "cell": "nothing" }));
    assert!(text.contains("no cell `nothing`"), "{text}");
}

/// Places a 10 V source across two 10 kΩ resistors in series.
fn divider(server: &mut Server) {
    let placed = ok(
        server,
        "place_parts",
        json!({ "parts": [
            { "part": "std:vdc", "name": "V1", "params": { "V": "10" } },
            { "part": "std:resistor", "params": { "R": "10k" } },
            { "part": "std:resistor", "params": { "R": "10k", "package": "0805" } },
        ] }),
    );
    assert!(
        placed.contains("R1 (std:resistor)") && placed.contains("R2 (std:resistor)"),
        "{placed}"
    );
    assert!(placed.contains("Pins: a ("), "pins are listed: {placed}");
    ok(server, "connect", json!({ "net": "VIN", "pins": ["V1.p", "R1.a"] }));
    ok(server, "connect", json!({ "net": "VOUT", "pins": ["R1.b", "R2.a"] }));
    let gnd = ok(server, "connect", json!({ "net": "GND", "pins": ["V1.n", "R2.b"] }));
    assert!(gnd.contains("(global)"), "ground reaches every sheet: {gnd}");
}

#[test]
fn builds_simulates_and_lays_out_a_circuit() {
    let (_tmp, mut server) = fresh();
    let found = ok(&mut server, "search_parts", json!({ "query": "voltage source", "limit": 1 }));
    assert!(found.starts_with("- std:vdc"), "{found}");
    divider(&mut server);
    let sheet = ok(&mut server, "describe_schematic", json!({}));
    assert!(sheet.contains("- VOUT: R1.b, R2.a"), "{sheet}");
    assert!(sheet.contains("- GND [global]: V1.n, R2.b"), "{sheet}");
    assert!(!sheet.contains("Unconnected"), "{sheet}");
    let taken = refused(&mut server, "connect", json!({ "net": "VIN", "pins": ["R2.a"] }));
    assert!(taken.contains("already on net VOUT"), "{taken}");
    let checked = ok(&mut server, "check", json!({}));
    assert!(checked.starts_with("No problems"), "{checked}");

    let op = ok(&mut server, "simulate", json!({ "analysis": "op" }));
    assert!(op.contains("- VOUT = 5V"), "{op}");
    let tran = ok(
        &mut server,
        "simulate",
        json!({ "analysis": "tran", "tstop": "1m", "probes": ["vout"] }),
    );
    assert!(tran.contains("VOUT: from 5V to 5V"), "{tran}");
    let ac = ok(&mut server, "simulate", json!({ "analysis": "ac", "probes": ["VOUT"] }));
    assert!(ac.contains("Nothing drives the circuit"), "a DC source has no AC: {ac}");

    // Labels made by connect go with a part that moves.
    ok(
        &mut server,
        "edit_parts",
        json!({ "parts": [{ "name": "R2", "at": [60.96, 30.48], "rotation": 90 }] }),
    );
    let moved = ok(&mut server, "describe_schematic", json!({}));
    assert!(moved.contains("- VOUT: R1.b, R2.a"), "{moved}");
    assert!(moved.contains("R2: std:resistor at (60.96, 30.48) rotated 90"), "{moved}");

    let laid = ok(&mut server, "arrange_sheet", json!({}));
    assert!(laid.contains("wires"), "{laid}");
    let drawn = ok(&mut server, "describe_schematic", json!({}));
    assert!(drawn.contains("- VOUT: R1.b, R2.a"), "the nets stay: {drawn}");
    assert!(drawn.contains("std:gnd"), "ground symbols are drawn: {drawn}");
    let op = ok(&mut server, "simulate", json!({ "analysis": "op", "probes": ["VOUT"] }));
    assert!(op.contains("- VOUT = 5V"), "{op}");
    // A part moved off drawn wires stays on its nets.
    ok(&mut server, "edit_parts", json!({ "parts": [{ "name": "R1", "at": [-50.8, 50.8] }] }));
    let moved = ok(&mut server, "describe_schematic", json!({}));
    assert!(
        moved.contains("- VOUT: R1.b, R2.a") || moved.contains("- VOUT: R2.a, R1.b"),
        "{moved}"
    );
    assert!(moved.contains("- VIN: V1.p, R1.a") || moved.contains("- VIN: R1.a, V1.p"), "{moved}");
    let op = ok(&mut server, "simulate", json!({ "analysis": "op", "probes": ["VOUT"] }));
    assert!(op.contains("- VOUT = 5V"), "{op}");
    // A pin taken off its net leaves it.
    ok(&mut server, "disconnect", json!({ "pins": ["R2.a"] }));
    let off = ok(&mut server, "describe_schematic", json!({}));
    assert!(off.contains("Unconnected pins: R2.a"), "{off}");
    ok(&mut server, "connect", json!({ "net": "VOUT", "pins": ["R2.a"] }));

    ok(&mut server, "set_frame", json!({ "title": "Divider", "revision": "B" }));
    let framed = ok(&mut server, "describe_schematic", json!({}));
    assert!(framed.contains("and a frame"), "{framed}");
    ok(&mut server, "delete_parts", json!({ "names": ["R2"] }));
    let op = ok(&mut server, "simulate", json!({ "analysis": "op", "probes": ["VOUT"] }));
    assert!(op.contains("- VOUT = 10V"), "nothing loads it now: {op}");
}

#[test]
fn makes_and_changes_components() {
    let (tmp, mut server) = fresh();
    let made = ok(
        &mut server,
        "create_component",
        json!({
            "name": "shunt",
            "description": "A shunt reference",
            "prefix": "D",
            "folder": "references",
            "ports": [
                { "name": "k", "kind": "passive", "group": "Power" },
                { "name": "a" },
                { "name": "ref", "kind": "input" },
            ],
            "params": [{ "name": "vref", "unit": "V", "default": "2.5" }],
            "views": { "spice": "(template: \"R${inst} ${node.k} ${node.a} 10k\")" },
        }),
    );
    assert!(made.contains("Ports: k, a, ref (input)") && made.contains("vref (V) = 2.5"), "{made}");
    assert!(made.contains("Symbol pins"), "a symbol is drawn: {made}");
    assert!(!made.contains("Problems"), "{made}");
    let cell =
        ok(&mut server, "get_cell", json!({ "cell": "shunt", "views": ["spice", "symbol"] }));
    assert!(cell.contains("template: \"R${inst}"), "{cell}");
    assert!(cell.contains("generated: true"), "{cell}");
    let bad = refused(
        &mut server,
        "update_component",
        json!({ "cell": "shunt", "views": { "spice": "(templat: 1" } }),
    );
    assert!(bad.contains("spice.ron"), "{bad}");
    let wrong = refused(
        &mut server,
        "create_component",
        json!({ "name": "x", "ports": [{ "name": "p", "kind": "sideways" }] }),
    );
    assert!(wrong.contains("sideways"), "{wrong}");

    // A symbol drawn from the ports follows them.
    let changed =
        ok(&mut server, "update_component", json!({ "cell": "shunt", "ports": ["k", "a"] }));
    assert!(changed.contains("Ports: k, a. "), "{changed}");
    let symbol = ok(&mut server, "get_cell", json!({ "cell": "shunt", "views": ["symbol"] }));
    assert!(!symbol.contains("port: \"ref\""), "{symbol}");
    refused(
        &mut server,
        "update_component",
        json!({ "cell": "std:resistor", "description": "mine" }),
    );

    // A part in use isn't deleted without asking.
    ok(&mut server, "place_parts", json!({ "parts": [{ "part": "shunt" }] }));
    let used = refused(&mut server, "delete_cell", json!({ "cell": "shunt" }));
    assert!(used.contains("amp:top"), "{used}");
    ok(&mut server, "delete_cell", json!({ "cell": "shunt", "force": true }));
    assert!(!tmp.path().join("amp/cells/references/shunt").exists());
    refused(&mut server, "delete_cell", json!({ "cell": "top" }));

    // SPICE models come in as components.
    let imported = ok(
        &mut server,
        "import_spice",
        json!({ "text": ".subckt buf in out\nE1 out 0 in 0 1\n.ends\n" }),
    );
    assert!(imported.contains("→ amp:buf. Ports: in, out."), "{imported}");
    let info = ok(&mut server, "project_info", json!({}));
    assert!(info.contains("- buf [imported]"), "{info}");
}

#[test]
fn boards_are_made_routed_and_checked() {
    let (_tmp, mut server) = fresh();
    divider(&mut server);
    refused(&mut server, "pcb_check", json!({}));
    let board = ok(&mut server, "pcb_new_board", json!({}));
    assert!(board.contains("2 footprints placed"), "the source is off the board: {board}");
    refused(&mut server, "pcb_new_board", json!({}));
    ok(&mut server, "pcb_pour", json!({ "net": "GND" }));
    refused(&mut server, "pcb_pour", json!({ "net": "NOPE" }));
    let routed = ok(&mut server, "pcb_autoroute", json!({}));
    assert!(!routed.contains("couldn't"), "{routed}");
    let checked = ok(&mut server, "pcb_check", json!({}));
    assert!(checked.contains("0 rule violation(s), 0 connection(s) unrouted"), "{checked}");
    assert!(checked.contains("All clear"), "{checked}");
    ok(
        &mut server,
        "pcb_place",
        json!({ "parts": [{ "ref": "R1", "at": [10, 10], "rotation": 90 }] }),
    );
    let same = ok(&mut server, "pcb_update", json!({}));
    assert!(same.contains("already matches"), "{same}");
}

#[test]
fn files_stay_inside_the_project_and_keep_it_readable() {
    let (tmp, mut server) = fresh();
    let root = tmp.path().join("amp");
    ok(
        &mut server,
        "write_file",
        json!({ "path": "firmware/main.c", "text": "int main(void) { return 0; }\n" }),
    );
    assert!(root.join("firmware/main.c").is_file());
    let listed = ok(&mut server, "list_files", json!({}));
    assert!(
        listed.contains("firmware/main.c") && listed.contains("cells/top/cell.ron"),
        "{listed}"
    );
    let read = ok(&mut server, "read_file", json!({ "path": "firmware/main.c" }));
    assert!(read.starts_with("int main"));
    let escaped =
        refused(&mut server, "write_file", json!({ "path": "../outside.txt", "text": "x" }));
    assert!(escaped.contains("inside the project"), "{escaped}");
    assert!(!tmp.path().join("outside.txt").exists());
    let before = std::fs::read_to_string(root.join("cells/top/cell.ron")).unwrap();
    let broken = refused(
        &mut server,
        "write_file",
        json!({ "path": "cells/top/cell.ron", "text": "oops(" }),
    );
    assert!(broken.contains("put back"), "{broken}");
    assert_eq!(std::fs::read_to_string(root.join("cells/top/cell.ron")).unwrap(), before);
    // Another server opens the same project by any path inside it.
    let mut other = Server::new(None).unwrap();
    ok(&mut other, "open_project", json!({ "path": root.join("firmware") }));
    assert!(ok(&mut other, "project_info", json!({})).contains("Project `amp`"));
}
