// The RV32I control unit in SystemVerilog: the same decoder as the built-in
// std:control, written as a case on the opcode. Open this file and press Make Part
// to place it; swap it for the CPU's CTRL to run the processor on it.

module control (
    input  logic [31:0] instr,
    output logic        reg_write,
    output logic [1:0]  alu_a,      // 0 rs1, 1 pc, 2 zero
    output logic        alu_b,      // 0 rs2, 1 immediate
    output logic [3:0]  alu_op,     // {funct7[5], funct3}
    output logic        mem_write,
    output logic [1:0]  wb_sel,     // 0 ALU, 1 memory, 2 pc + 4
    output logic        branch,
    output logic        jump,
    output logic        jalr,
    output logic        halt
);
    logic [6:0] opcode;
    logic [2:0] funct3;
    logic       f7b5;
    assign opcode = instr[6:0];
    assign funct3 = instr[14:12];
    assign f7b5   = instr[30];

    always_comb begin
        reg_write = 0; alu_a = 0; alu_b = 0; alu_op = 0; mem_write = 0;
        wb_sel = 0; branch = 0; jump = 0; jalr = 0; halt = 0;
        case (opcode)
            7'b0110011: begin                       // R-type
                reg_write = 1;
                alu_op = {f7b5, funct3};
            end
            7'b0010011: begin                       // I-type arithmetic
                reg_write = 1; alu_b = 1;
                alu_op = {funct3 == 3'b101 ? f7b5 : 1'b0, funct3};
            end
            7'b0000011: begin                       // loads
                reg_write = 1; alu_b = 1; wb_sel = 1;
            end
            7'b0100011: begin                       // stores
                mem_write = 1; alu_b = 1;
            end
            7'b1100011: begin                       // branches
                branch = 1; alu_op = 4'b1000;
            end
            7'b1101111: begin                       // JAL
                reg_write = 1; wb_sel = 2; jump = 1;
            end
            7'b1100111: begin                       // JALR
                reg_write = 1; alu_b = 1; wb_sel = 2; jalr = 1;
            end
            7'b0110111: begin                       // LUI
                reg_write = 1; alu_a = 2; alu_b = 1;
            end
            7'b0010111: begin                       // AUIPC
                reg_write = 1; alu_a = 1; alu_b = 1;
            end
            7'b1110011: halt = 1;                   // ECALL, EBREAK
            default: ;
        endcase
    end
endmodule
