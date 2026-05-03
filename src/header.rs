//! DTS Core frame-header parsing.
//!
//! See `dts-trace-reverse-engineering.md` §2.2 for the field-by-field
//! breakdown. Round-1 supports the BE 16-bit packed sync word
//! (`0x7FFE_8001`) only; 14-bit and LE byte-swapped variants are
//! deferred.

use oxideav_core::{Error, Result};

use crate::bits::BitReader;
use crate::syncwords;
use crate::tables::{
    BITS_PER_SAMPLE, CHANNELS_BY_AMODE, CORE_BIT_RATES, CORE_SAMPLE_RATES,
};

/// Decoded DTS Core frame header (104 bits without the optional CRC,
/// 120 bits with it).
#[derive(Clone, Debug)]
pub struct CoreHeader {
    pub frame_type: u8,
    /// `DEFICIT_SAMPLES`: must be 32 (encoded as 31).
    pub deficit: u8,
    pub crc_present: bool,
    /// PCM sample blocks per channel (always a multiple of 8). The
    /// observed corpus has 16 → 16 × 32 = 512 PCM samples per frame.
    pub nblks: u8,
    /// Frame size in bytes (decoded value, not -1).
    pub frame_size: u16,
    pub amode: u8,
    /// Resolved primary-channel count (from CHANNELS_BY_AMODE). LFE
    /// adds one more channel; XCH/XXCH/X96 add channels too but are
    /// out of scope for round 1.
    pub primary_channels: u8,
    pub sample_rate: u32,
    pub bit_rate: u32,
    pub rate_code: u8,
    pub mix_down: bool,
    pub drc_flag: bool,
    pub ts_flag: bool,
    pub aux_flag: bool,
    pub hdcd: bool,
    pub ext_audio_type: u8,
    pub ext_audio_pres: bool,
    pub sync_ssf: bool,
    /// LFE state: 0 = none, 1 = 128× decim, 2 = 64× decim, 3 = invalid.
    pub lfe: u8,
    pub pred_hist: bool,
    pub filter_perfect: bool,
    pub encoder_rev: u8,
    pub copy_hist: u8,
    pub pcmr: u8,
    pub bits_per_sample: u8,
    pub sumdiff_front: bool,
    pub sumdiff_surround: bool,
    pub dialnorm: u8,
    /// Whether the lossless step-size LUT applies (RATE == 31).
    pub lossless_mode: bool,
    /// Number of bits consumed by the header (104 or 120).
    pub bits_consumed: usize,
}

/// Parse a DTS Core frame header. The packet must start with the
/// sync word `0x7FFE_8001`.
pub fn parse(data: &[u8]) -> Result<CoreHeader> {
    let mut r = BitReader::new(data);
    let sync = r.read(32)?;
    if sync != syncwords::CORE_BE {
        return Err(Error::invalid(format!(
            "dts: unsupported sync word 0x{sync:08X} (round-1 supports BE 16-bit only)"
        )));
    }
    let frame_type = r.read(1)? as u8;
    let deficit = r.read(5)? as u8;
    if deficit != 31 {
        return Err(Error::invalid(format!(
            "dts: DEFICIT must be 31 (got {deficit})"
        )));
    }
    let crc_present = r.read(1)? == 1;
    let nblks_m1 = r.read(7)? as u8;
    let nblks = nblks_m1 + 1;
    let frame_size = (r.read(14)? + 1) as u16;
    let amode = r.read(6)? as u8;
    if amode >= 16 {
        return Err(Error::invalid(format!(
            "dts: AMODE {amode} reserved (round-1 supports 0..15)"
        )));
    }
    let primary_channels = CHANNELS_BY_AMODE[amode as usize];
    let sfreq = r.read(4)? as usize;
    let sample_rate = CORE_SAMPLE_RATES[sfreq];
    if sample_rate == 0 {
        return Err(Error::invalid(format!(
            "dts: SFREQ {sfreq} reserved"
        )));
    }
    let rate_code = r.read(5)? as u8;
    let bit_rate = CORE_BIT_RATES[rate_code as usize];
    let mix_down = r.read(1)? == 1;
    if mix_down {
        return Err(Error::invalid("dts: reserved MIX_DOWN bit set"));
    }
    let drc_flag = r.read(1)? == 1;
    let ts_flag = r.read(1)? == 1;
    let aux_flag = r.read(1)? == 1;
    let hdcd = r.read(1)? == 1;
    let ext_audio_type = r.read(3)? as u8;
    let ext_audio_pres = r.read(1)? == 1;
    let sync_ssf = r.read(1)? == 1;
    let lfe = r.read(2)? as u8;
    if lfe == 3 {
        return Err(Error::invalid("dts: LFE = 3 reserved"));
    }
    let pred_hist = r.read(1)? == 1;
    if crc_present {
        let _hcrc = r.read(16)?;
    }
    let filter_perfect = r.read(1)? == 1;
    let encoder_rev = r.read(4)? as u8;
    let copy_hist = r.read(2)? as u8;
    let pcmr = r.read(3)? as u8;
    let bits_per_sample = BITS_PER_SAMPLE[pcmr as usize];
    if bits_per_sample == 0 {
        return Err(Error::invalid(format!("dts: PCMR {pcmr} reserved")));
    }
    let sumdiff_front = r.read(1)? == 1;
    let sumdiff_surround = r.read(1)? == 1;
    let dialnorm = r.read(4)? as u8;

    let bits_consumed = r.bit_pos();
    Ok(CoreHeader {
        frame_type,
        deficit,
        crc_present,
        nblks,
        frame_size,
        amode,
        primary_channels,
        sample_rate,
        bit_rate,
        rate_code,
        mix_down,
        drc_flag,
        ts_flag,
        aux_flag,
        hdcd,
        ext_audio_type,
        ext_audio_pres,
        sync_ssf,
        lfe,
        pred_hist,
        filter_perfect,
        encoder_rev,
        copy_hist,
        pcmr,
        bits_per_sample,
        sumdiff_front,
        sumdiff_surround,
        dialnorm,
        lossless_mode: rate_code == 31,
        bits_consumed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_sync() {
        let data = [0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 0];
        assert!(parse(&data).is_err());
    }
}
