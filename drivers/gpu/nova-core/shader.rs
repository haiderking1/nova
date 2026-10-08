// SPDX-License-Identifier: GPL-2.0

//! Encoder of the shader instructions of GA10x GPUs (SM86), for the shaders Nova runs itself.
//!
//! An instruction is 128 bits wide: its operation and operands, then scheduling bits telling the
//! GPU how long to wait before issuing the next instruction. Nothing waits for a result unless the
//! scheduling bits say so, so each instruction must be given the [`Deps`] its successors need.
//!
//! Bit positions and opcodes are those of Mesa's NAK compiler (`sm70_encode.rs`), latencies those
//! of its Ampere tables (`sm80_instr_latencies.rs`).

/// Whether instructions for GPUs of shader model `sm` (as in `sm_86`) can be encoded: those of
/// Ampere and Ada, SM 8.x, which the latencies are of.
pub(crate) const fn supports(sm: u8) -> bool {
    matches!(sm, 80..=89)
}

/// A general purpose register.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Reg(pub(crate) u8);

/// The register reading as zero.
pub(crate) const RZ: Reg = Reg(255);

/// A predicate register.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Pred(pub(crate) u8);

/// The predicate reading as true.
pub(crate) const PT: Pred = Pred(7);

/// Special registers [`s2r`] reads, numbered as in NAK's `nak_sv`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub(crate) enum SpecialReg {
    /// X coordinate of the thread in its block.
    TidX = 0x21,
    /// Y coordinate of the thread in its block.
    TidY = 0x22,
    /// X coordinate of the block in the grid.
    CtaidX = 0x25,
    /// Y coordinate of the block in the grid.
    CtaidY = 0x26,
}

/// The second source of an ALU instruction: a register or a 32-bit immediate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Src {
    Reg(Reg),
    Imm(u32),
}

/// How [`isetp`] combines its comparison with its accumulated predicate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub(crate) enum PredOp {
    And = 0,
    Or = 1,
}

/// Comparisons of [`isetp`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub(crate) enum Cmp {
    Ge = 6,
}

/// Truth tables of [`lop3`], over its sources `a`, `b` and `c`.
pub(crate) mod lut {
    /// Truth table of source `a`.
    pub(crate) const A: u8 = 0xf0;
    /// Truth table of source `b`.
    pub(crate) const B: u8 = 0xcc;

    /// `a & b`.
    pub(crate) const AND: u8 = A & B;
    /// `a ^ b`.
    pub(crate) const XOR: u8 = A ^ B;
}

/// Scheduling of an instruction: what it waits for before issuing, and how long the next
/// instruction waits after it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Deps {
    /// Cycles to stall before issuing the next instruction, up to 15.
    pub(crate) delay: u8,
    /// Scoreboard this instruction releases once its variable-latency result is written.
    pub(crate) signal: Option<u8>,
    /// Scoreboards to wait on before issuing, as a mask of their indices.
    pub(crate) wait: u8,
}

impl Deps {
    /// Stall `delay` cycles after the instruction, without scoreboards.
    ///
    /// The instruction never yields: with yield set, the GPU may issue the next instruction
    /// before the stall is over.
    pub(crate) const fn stall(delay: u8) -> Self {
        Self {
            delay,
            signal: None,
            wait: 0,
        }
    }

    /// Additionally release `scoreboard` once the instruction's result is written.
    pub(crate) const fn signal(self, scoreboard: u8) -> Self {
        Self {
            signal: Some(scoreboard),
            ..self
        }
    }

    /// Additionally wait on `scoreboard` before issuing the instruction.
    pub(crate) const fn wait(self, scoreboard: u8) -> Self {
        Self {
            wait: self.wait | 1 << scoreboard,
            ..self
        }
    }
}

// Latencies of GA10x, from NAK's `sm80_instr_latencies.rs`, as cycles between issuing an
// instruction and issuing one reading its result.

/// The longest latency of the results of fixed-latency integer instructions (`MOV`, `IMAD`,
/// `IMAD.WIDE`, `LOP3`, `ISETP`) read as registers by the next instruction.
pub(crate) const ALU_LATENCY: u8 = 6;
/// Latency of a predicate written by `ISETP` and read as the guard of the next instruction.
pub(crate) const GUARD_LATENCY: u8 = 13;

/// An encoded instruction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Instr([u32; 4]);

impl Instr {
    /// Instruction `opcode`, running unconditionally, without scheduling.
    fn new(opcode: u16) -> Self {
        Self([0; 4])
            .field(0, 12, opcode.into())
            .guard(PT, false)
            .deps(Deps::stall(0))
    }

    /// Sets the `len` bits starting at bit `start` to `value`.
    fn field(mut self, start: usize, len: usize, value: u32) -> Self {
        for bit in 0..len {
            let (word, shift) = ((start + bit) / 32, (start + bit) % 32);
            self.0[word] &= !(1 << shift);
            self.0[word] |= (value >> bit & 1) << shift;
        }
        self
    }

    fn bit(self, bit: usize, value: bool) -> Self {
        self.field(bit, 1, value.into())
    }

    fn dst(self, rd: Reg) -> Self {
        self.field(16, 8, rd.0.into())
    }

    fn src0(self, ra: Reg) -> Self {
        self.field(24, 8, ra.0.into())
    }

    /// Instruction of the ALU `opcode`, with sources `a`, `b` and `c`. Sources the operation
    /// doesn't have are left zero, as NAK does.
    fn alu(opcode: u16, rd: Reg, ra: Option<Reg>, b: Src, rc: Option<Reg>) -> Self {
        let (b, form) = match b {
            Src::Reg(rb) => (u32::from(rb.0), 1),
            Src::Imm(imm) => (imm, 4),
        };

        Self::new(opcode)
            .field(0, 9, opcode.into())
            .field(9, 3, form)
            .dst(rd)
            .field(24, 8, ra.map_or(0, |ra| ra.0.into()))
            .field(32, 32, b)
            .field(64, 8, rc.map_or(0, |rc| rc.0.into()))
    }

    /// Runs the instruction only where `pred` is true, or false if `negate`.
    pub(crate) fn guard(self, pred: Pred, negate: bool) -> Self {
        self.field(12, 3, pred.0.into()).bit(15, negate)
    }

    /// Schedules the instruction as `deps` says.
    pub(crate) fn deps(self, deps: Deps) -> Self {
        // Delay in bits 108:105, yield in bit 109, then the scoreboard to release on writing
        // (112:110) and on reading (115:113) the registers, 7 meaning none, and those to wait on
        // (121:116).
        self.field(105, 4, deps.delay.into())
            .bit(109, false)
            .field(110, 3, deps.signal.unwrap_or(7).into())
            .field(113, 3, 7)
            .field(116, 6, deps.wait.into())
    }

    /// The instruction's words, as the GPU reads them.
    pub(crate) fn words(&self) -> &[u32; 4] {
        &self.0
    }
}

/// `MOV rd, imm`.
pub(crate) fn mov(rd: Reg, imm: u32) -> Instr {
    // All lanes of the quad (bits 75:72).
    Instr::alu(0x002, rd, None, Src::Imm(imm), None).field(72, 4, 0xf)
}

/// `S2R rd, sr`: reads a special register, with a variable latency, so the instruction reading
/// `rd` must wait on a scoreboard this one releases.
pub(crate) fn s2r(rd: Reg, sr: SpecialReg) -> Instr {
    Instr::new(0x919).dst(rd).field(72, 8, sr as u32)
}

/// `IMAD.U32 rd, ra, b, rc`: `rd = ra * b + rc`.
pub(crate) fn imad(rd: Reg, ra: Reg, b: Src, rc: Reg) -> Instr {
    // No predicate output (83:81).
    Instr::alu(0x024, rd, Some(ra), b, Some(rc)).field(81, 3, PT.0.into())
}

/// `IMAD.WIDE.U32 rd, ra, b, rc`: `rd = ra * b + rc`, with `rd` and `rc` 64-bit register pairs.
pub(crate) fn imad_wide(rd: Reg, ra: Reg, b: Src, rc: Reg) -> Instr {
    Instr::alu(0x025, rd, Some(ra), b, Some(rc)).field(81, 3, PT.0.into())
}

/// `ISETP.cmp.U32.op pd, PT, ra, b, acc`: `pd = (ra cmp b) op acc`.
pub(crate) fn isetp(pd: Pred, cmp: Cmp, ra: Reg, b: Src, op: PredOp, acc: Pred) -> Instr {
    // No destination register. The low comparison input (70:68) true, unsigned comparison
    // (bit 73), then the second predicate output (86:84) unused.
    Instr::alu(0x00c, Reg(0), Some(ra), b, None)
        .field(68, 3, PT.0.into())
        .field(73, 1, 0)
        .field(74, 2, op as u32)
        .field(76, 3, cmp as u32)
        .field(81, 3, pd.0.into())
        .field(84, 3, PT.0.into())
        .field(87, 3, acc.0.into())
}

/// `LOP3.LUT rd, ra, b, rc, lut`: bitwise `lut` of `ra`, `b` and `rc`.
pub(crate) fn lop3(rd: Reg, ra: Reg, b: Src, rc: Reg, lut: u8) -> Instr {
    // Truth table in bits 79:72, then no predicate output (83:81) and the predicate input (89:87)
    // false: PT negated (bit 90).
    Instr::alu(0x012, rd, Some(ra), b, Some(rc))
        .field(72, 8, lut.into())
        .field(81, 3, PT.0.into())
        .field(87, 3, PT.0.into())
        .bit(90, true)
}

/// `STG.E [ra.64 + offset], rdata`: stores 32 bits to the global memory address in the register
/// pair `ra`.
pub(crate) fn stg(ra: Reg, offset: u32, rdata: Reg) -> Instr {
    // 64-bit address register (bit 90), uniform address register URZ (69:64), 32 bits stored
    // (75:73), weak ordering (80:77), normal eviction priority (86:84), and the uniform address
    // register in use (bit 91), which makes `.E` apply to it.
    Instr::new(0x986)
        .src0(ra)
        .field(32, 8, rdata.0.into())
        .field(40, 24, offset)
        .field(64, 6, 63)
        .bit(72, true)
        .field(73, 3, 4)
        .field(84, 3, 1)
        .bit(90, true)
        .bit(91, true)
}

/// `EXIT`: ends the thread.
pub(crate) fn exit() -> Instr {
    // Predicate input (89:87) true.
    Instr::new(0x94d).field(87, 3, PT.0.into())
}

/// `BRA .`: loops on itself, after the last `EXIT`, as compilers end programs.
pub(crate) fn bra_self() -> Instr {
    // Offset -16 bytes from the next instruction, sign extended (bits 81:34), predicate input
    // (89:87) true.
    Instr::new(0x947)
        .field(32, 32, 0xffff_fff0)
        .field(64, 18, 0x3ffff)
        .field(87, 3, PT.0.into())
}

// Programs, with registers and scheduling assigned by hand.

/// Number of registers per thread to have the GPU allocate for a program using registers up to
/// R`highest`: two more, which the hardware reserves, in multiples of 8.
pub(crate) const fn registers(highest: u8) -> u8 {
    (highest + 1 + 2).next_multiple_of(8)
}

/// Number of instructions of [`store`].
pub(crate) const STORE_LEN: usize = 6;

/// Number of registers [`store`] uses: up to R5.
pub(crate) const STORE_REGISTERS: u8 = registers(5);

/// Program storing `value` at GPU virtual address `addr`, from every thread.
///
/// It uses [`STORE_REGISTERS`] registers.
// CAST: the address is split into its low and high 32 bits.
pub(crate) fn store(addr: u64, value: u32) -> [Instr; STORE_LEN] {
    let alu = Deps::stall(ALU_LATENCY);
    [
        mov(Reg(2), addr as u32).deps(alu),
        mov(Reg(3), (addr >> 32) as u32).deps(alu),
        mov(Reg(5), value).deps(alu),
        stg(Reg(2), 0, Reg(5)).deps(Deps::stall(1)),
        exit().deps(Deps::stall(5)),
        bra_self(),
    ]
}

/// A pitch-linear surface of 32-bit pixels, at a GPU virtual address.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PixelSurface {
    pub(crate) addr: u64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Bytes from one line to the next.
    pub(crate) pitch: u32,
}

/// Width of the blocks of threads running [`pattern`], in pixels.
pub(crate) const PATTERN_BLOCK_WIDTH: u16 = 32;
/// Height of the blocks of threads running [`pattern`], in pixels.
pub(crate) const PATTERN_BLOCK_HEIGHT: u16 = 8;

/// Number of registers [`pattern`] uses: up to R9.
pub(crate) const PATTERN_REGISTERS: u8 = registers(9);

/// Number of instructions of [`pattern`].
pub(crate) const PATTERN_LEN: usize = 22;

/// Program drawing a pattern on `surface` in XRGB8888, each thread computing one pixel from its
/// coordinates `x` and `y`: red `x`, green `y` and blue `x ^ y`, all modulo 256.
///
/// It runs as blocks of [`PATTERN_BLOCK_WIDTH`] by [`PATTERN_BLOCK_HEIGHT`] threads, as many as
/// needed to cover the surface, and uses [`PATTERN_REGISTERS`] registers.
// CAST: the address is split into its low and high 32 bits.
pub(crate) fn pattern(surface: &PixelSurface) -> [Instr; PATTERN_LEN] {
    let alu = Deps::stall(ALU_LATENCY);
    // Scoreboard of the special registers read.
    let sr = 0;
    let read_sr = Deps::stall(1).signal(sr);
    let (tid_x, tid_y, ctaid_x, ctaid_y) = (Reg(0), Reg(1), Reg(2), Reg(3));
    let (x, y, color, channel, addr) = (Reg(4), Reg(5), Reg(6), Reg(7), Reg(8));
    let outside = Pred(0);
    let byte = Src::Imm(0xff);

    [
        s2r(tid_x, SpecialReg::TidX).deps(read_sr),
        s2r(tid_y, SpecialReg::TidY).deps(read_sr),
        s2r(ctaid_x, SpecialReg::CtaidX).deps(read_sr),
        s2r(ctaid_y, SpecialReg::CtaidY).deps(read_sr),
        // x = ctaid.x * block width + tid.x, y alike.
        imad(x, ctaid_x, Src::Imm(PATTERN_BLOCK_WIDTH.into()), tid_x).deps(alu.wait(sr)),
        imad(y, ctaid_y, Src::Imm(PATTERN_BLOCK_HEIGHT.into()), tid_y).deps(alu),
        // Threads of blocks past the right or bottom edge have nothing to draw.
        isetp(
            outside,
            Cmp::Ge,
            x,
            Src::Imm(surface.width),
            PredOp::And,
            PT,
        )
        .deps(alu),
        isetp(
            outside,
            Cmp::Ge,
            y,
            Src::Imm(surface.height),
            PredOp::Or,
            outside,
        )
        .deps(Deps::stall(GUARD_LATENCY)),
        exit().guard(outside, false).deps(Deps::stall(1)),
        // Blue.
        lop3(color, x, Src::Reg(y), RZ, lut::XOR).deps(alu),
        lop3(color, color, byte, RZ, lut::AND).deps(alu),
        // Green.
        lop3(channel, y, byte, RZ, lut::AND).deps(alu),
        imad(color, channel, Src::Imm(1 << 8), color).deps(alu),
        // Red.
        lop3(channel, x, byte, RZ, lut::AND).deps(alu),
        imad(color, channel, Src::Imm(1 << 16), color).deps(alu),
        // addr = surface + y * pitch + x * 4.
        mov(addr, surface.addr as u32).deps(alu),
        mov(Reg(addr.0 + 1), (surface.addr >> 32) as u32).deps(alu),
        imad_wide(addr, x, Src::Imm(4), addr).deps(alu),
        imad_wide(addr, y, Src::Imm(surface.pitch), addr).deps(alu),
        stg(addr, 0, color).deps(Deps::stall(1)),
        exit().deps(Deps::stall(5)),
        bra_self(),
    ]
}
