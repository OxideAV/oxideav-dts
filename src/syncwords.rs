//! DTS sync-word constants (from `dts-core-tables.md` §1).
//!
//! All values are 32-bit big-endian on the wire. The four core
//! variants cover BE/LE × 16-bit/14-bit packings (only `CORE_BE` is
//! supported by the round-1 decoder; the 14-bit variants and the LE
//! cell-swap variant are documented for future rounds).

#![allow(dead_code)]

pub const CORE_BE: u32 = 0x7FFE_8001;
pub const CORE_LE: u32 = 0xFE7F_0180;
pub const CORE_14B_BE: u32 = 0x1FFF_E800;
pub const CORE_14B_LE: u32 = 0xFF1F_00E8;

pub const XCH: u32 = 0x5A5A_5A5A;
pub const XXCH: u32 = 0x4700_4A03;
pub const X96: u32 = 0x1D95_F262;
pub const XBR: u32 = 0x655E_315E;
pub const LBR: u32 = 0x0A80_1921;
pub const XLL: u32 = 0x41A2_9547;
pub const EXSS: u32 = 0x6458_2025;
pub const SUBSTREAM_CORE: u32 = 0x02B0_9261;
pub const REV1AUX: u32 = 0x9A11_05A0;
