//! A Model Context Protocol server, so an AI assistant can work on a project: make
//! components, draw and wire schematics, check and simulate them, and lay out boards.
//!
//! `circuiter mcp [PROJECT]` speaks JSON-RPC 2.0 over stdin and stdout, one message per
//! line, as MCP's stdio transport does; anything else it has to say goes to stderr.
//! Every tool call reads the project from disk and writes back what it changed, so an
//! open Circuiter window, which watches the files, shows each change as it is made.

#[cfg(test)]
mod tests;
mod tools;

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;

pub use tools::Server;

/// The protocol revision answered when the client asks for one this doesn't know.
const PROTOCOL: &str = "2025-06-18";
const PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// What the assistant is told about the server when it connects.
const INSTRUCTIONS: &str = "Circuiter is an electronics design tool: schematics, \
simulation (SPICE-style analog, digital and mixed), PCB layout and 3D. A project holds \
cells; a cell is a component or a circuit, with views: its interface (ports and \
parameters), a symbol, a schematic, a SPICE model, a digital model, a footprint, a 3D \
body and a board. Parts are named `library:cell`: `std:resistor` from the standard \
library, or a project cell by its name alone.\n\
\n\
Sheet coordinates are millimetres on a 2.54 mm grid, +y down. To build a circuit: \
search_parts for what to use (create_component for anything missing), place_parts on \
the cell's schematic, connect pins into nets, then arrange_sheet to draw the wiring \
neatly, check for mistakes, and simulate. For a board: pcb_new_board, pcb_autoroute, \
pcb_check. get_cell shows any cell's views in the RON its files use: read a similar \
standard part before writing views of your own. Changes are saved at once, and an open \
Circuiter window shows them as they are made.";

/// Serves the protocol on stdin and stdout until stdin closes.
pub fn serve(project: Option<PathBuf>) -> anyhow::Result<()> {
    let mut server = Server::new(project)?;
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle(&mut server, &line) {
            writeln!(out, "{reply}")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// Answers one line: a request, a notification (no answer), or a batch of them.
pub fn handle(server: &mut Server, line: &str) -> Option<Value> {
    match serde_json::from_str::<Value>(line) {
        Ok(Value::Array(batch)) => {
            let replies: Vec<Value> = batch.iter().filter_map(|m| answer(server, m)).collect();
            (!replies.is_empty()).then_some(Value::Array(replies))
        }
        Ok(message) => answer(server, &message),
        Err(e) => Some(failure(Value::Null, -32700, &format!("not JSON: {e}"))),
    }
}

fn answer(server: &mut Server, message: &Value) -> Option<Value> {
    // Notifications and responses carry no id and get no answer.
    let id = message.get("id")?.clone();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Some(failure(id, -32600, "a request needs a `method`"));
    };
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
    let result = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools::list() })),
        "tools/call" => call(server, &params),
        _ => Err((-32601, format!("no method `{method}`"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => failure(id, code, &message),
    })
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked.filter(|v| PROTOCOLS.contains(v)).unwrap_or(PROTOCOL);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "circuiter",
            "title": "Circuiter",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    })
}

/// Runs a tool. A tool that fails says why in its result, flagged as an error, so the
/// assistant can read it and try again; only a call naming no tool is a protocol error.
fn call(server: &mut Server, params: &Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "tools/call needs the tool's `name`".to_owned()))?;
    if !tools::exists(name) {
        return Err((-32602, format!("no tool `{name}`")));
    }
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| server.call(name, &args)));
    let (text, is_error) = match ran {
        Ok(Ok(text)) => (text, false),
        Ok(Err(e)) => (e, true),
        Err(panic) => {
            let why = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            (format!("internal error in {name}: {why}"), true)
        }
    };
    Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
}

fn failure(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}
