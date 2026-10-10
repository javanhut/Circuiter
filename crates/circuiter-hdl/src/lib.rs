//! Verilog and SystemVerilog for Circuiter: describe a part in HDL instead of drawing
//! it, and it runs in the digital simulator like any built-in part.
//!
//! The synthesizable subset people write for logic and processors: modules with
//! parameters, `input`/`output` ports, `wire`/`reg`/`logic`, `assign`, `always_comb`
//! and `always @(*)`, `always_ff @(posedge clk or posedge rst)` and `always
//! @(posedge clk)` with non-blocking assignments, `if`/`else`, `case`/`casez`, bit and
//! part selects, concatenation and replication, every arithmetic, logical, bitwise,
//! reduction, shift and comparison operator, `?:`, `$signed`, `$unsigned` and
//! `$clog2`. Values are four-state and up to 64 bits wide. Not (yet): instances of
//! other modules, loops, functions, tasks, `initial`.

pub mod parse;
pub mod sim;

pub use parse::{Dir, HdlError};
pub use sim::{PortInfo, create, interface};

#[cfg(test)]
mod tests {
    use super::*;
    use circuiter_logic::{Circuit, Simulator, Value, ns, prims};
    use std::collections::BTreeMap;

    fn none() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn konst(width: u32, value: u64) -> Box<dyn circuiter_logic::Primitive> {
        let p = prims::params(&[("width", &width.to_string()), ("value", &value.to_string())]);
        prims::create("const", &p, None).unwrap()
    }

    fn clock(period_ns: f64) -> Box<dyn circuiter_logic::Primitive> {
        let p = prims::params(&[("period", &format!("{period_ns}n"))]);
        prims::create("clock", &p, None).unwrap()
    }

    /// Runs `src` with inputs from constants and clocks, for `t` ns.
    fn run(
        src: &str,
        inputs: Vec<(&str, Box<dyn circuiter_logic::Primitive>)>,
        t: f64,
    ) -> (Simulator, BTreeMap<String, usize>) {
        let mut c = Circuit::new();
        let part = create(src, None, &none(), ns(1.0)).unwrap();
        let mut by_name = BTreeMap::new();
        let nets: Vec<usize> = part
            .ports()
            .iter()
            .map(|p| {
                let n = c.net(&p.name, p.width);
                by_name.insert(p.name.clone(), n);
                n
            })
            .collect();
        c.add("U1", part, &nets).unwrap();
        for (port, d) in inputs {
            c.add(&format!("in_{port}"), d, &[by_name[port]]).unwrap();
        }
        let mut sim = Simulator::new(c);
        sim.run_until(ns(t)).unwrap();
        (sim, by_name)
    }

    #[test]
    fn a_counter_with_reset_and_enable() {
        let src = "
            module counter #(parameter W = 8) (
                input  logic clk, rst, en,
                output logic [W-1:0] q
            );
                always_ff @(posedge clk or posedge rst)
                    if (rst) q <= '0 ;
                    else if (en) q <= q + 1;
            endmodule";
        let ports = interface(src, None, &none()).unwrap();
        assert_eq!(
            ports.iter().map(|p| (p.name.as_str(), p.width)).collect::<Vec<_>>(),
            [("clk", 1), ("rst", 1), ("en", 1), ("q", 8)]
        );
        let (sim, n) =
            run(src, vec![("clk", clock(10.0)), ("rst", konst(1, 0)), ("en", konst(1, 1))], 102.0);
        // Rising edges at 5, 15, … 95 ns: but q starts unknown, so reset it first.
        assert!(!sim.value(n["q"]).is_known(), "never reset: unknown");
        let (sim, n) = run(
            src,
            vec![
                ("clk", clock(10.0)),
                (
                    "rst",
                    prims::create("pulse", &prims::params(&[("length", "12n")]), None).unwrap(),
                ),
                ("en", konst(1, 1)),
            ],
            102.0,
        );
        // Reset until 12 ns; counting edges at 15, 25, … 95: nine.
        assert_eq!(sim.value(n["q"]), Value::new(8, 9));
    }

    #[test]
    fn combinational_logic_with_case_and_assign() {
        let src = "
            module alu (input [3:0] op, input [7:0] a, b, output reg [7:0] y, output zero);
                assign zero = (y == 0);
                always @(*) begin
                    case (op)
                        4'd0: y = a + b;
                        4'd1: y = a - b;
                        4'd2: y = a & b;
                        4'd3: y = {a[3:0], b[7:4]};
                        default: y = ~a;
                    endcase
                end
            endmodule";
        for (op, expect) in
            [(0, 0x37 + 0x05), (1, 0x37 - 0x05), (2, 0x37 & 0x05), (3, 0x70), (9, 0xC8)]
        {
            let (sim, n) = run(
                src,
                vec![("op", konst(4, op)), ("a", konst(8, 0x37)), ("b", konst(8, 0x05))],
                10.0,
            );
            assert_eq!(sim.value(n["y"]), Value::new(8, expect), "op {op}");
        }
        let (sim, n) =
            run(src, vec![("op", konst(4, 1)), ("a", konst(8, 9)), ("b", konst(8, 9))], 10.0);
        assert_eq!(sim.value(n["zero"]), Value::bit(true));
    }

    #[test]
    fn carries_widths_and_signed_values() {
        let src = "
            module add (input [7:0] a, b, input cin, output [7:0] sum, output cout,
                        output lt_signed, output [7:0] shr);
                assign {cout, sum} = a + b + cin;
                assign lt_signed = $signed(a) < $signed(b);
                assign shr = $signed(a) >>> 2;
            endmodule";
        let (sim, n) = run(
            src,
            vec![("a", konst(8, 0xF0)), ("b", konst(8, 0x20)), ("cin", konst(1, 1))],
            10.0,
        );
        assert_eq!(sim.value(n["sum"]), Value::new(8, 0x11));
        assert_eq!(sim.value(n["cout"]), Value::bit(true), "the carry out of eight bits");
        assert_eq!(sim.value(n["lt_signed"]), Value::bit(true), "-16 < 32");
        assert_eq!(sim.value(n["shr"]), Value::new(8, 0xFC), "arithmetic shift keeps the sign");
    }

    #[test]
    fn casez_matches_wildcards() {
        let src = "
            module prio (input [3:0] req, output reg [1:0] grant);
                always_comb
                    casez (req)
                        4'b1???: grant = 3;
                        4'b01??: grant = 2;
                        4'b001?: grant = 1;
                        default: grant = 0;
                    endcase
            endmodule";
        for (req, g) in [(0b1010, 3), (0b0110, 2), (0b0011, 1), (0b0001, 0)] {
            let (sim, n) = run(src, vec![("req", konst(4, req))], 10.0);
            assert_eq!(sim.value(n["grant"]), Value::new(2, g), "{req:04b}");
        }
    }

    #[test]
    fn errors_name_their_lines() {
        let e =
            create("module m (input a, output y);\n  assign y = b;\nendmodule", None, &none(), 0)
                .unwrap_err();
        assert!(e.contains("line 2") && e.contains("`b`"), "{e}");
        let e =
            create("module m (input a, output y);\n  assign y = a\nendmodule", None, &none(), 0)
                .unwrap_err();
        assert!(e.contains("line 3") && e.contains("`;`"), "{e}");
    }
}
