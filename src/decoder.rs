//! DTS Core decoder front-end.
//!
//! Implements the [`oxideav_core::Decoder`] trait — accepts one
//! Core frame per `send_packet`, returns one [`AudioFrame`] per
//! `receive_frame`. The actual signal-processing pipeline lives in
//! the lower-level modules ([`crate::header`], [`crate::audblk`],
//! [`crate::pqf`]).

use oxideav_core::{
    AudioFrame, CodecId, CodecParameters, Decoder, Error, Frame, Packet, Result, TimeBase,
};

use crate::audblk::{decode_audio_payload, FrameOutput};
use crate::header;
use crate::pqf::{PqfSynth, SUBBANDS};

/// 16 PCM blocks per frame × 32 samples per band → 512 samples per channel.
pub const SAMPLES_PER_FRAME: u32 = 512;

pub fn make_decoder(params: &CodecParameters) -> Result<Box<dyn Decoder>> {
    Ok(Box::new(DtsDecoder {
        codec_id: params.codec_id.clone(),
        time_base: TimeBase::new(1, 48_000),
        pending: None,
        eof: false,
        synths: Vec::new(),
    }))
}

struct DtsDecoder {
    codec_id: CodecId,
    time_base: TimeBase,
    pending: Option<Packet>,
    eof: bool,
    /// One PqfSynth per primary channel (lazily resized on first
    /// frame).
    synths: Vec<PqfSynth>,
}

impl Decoder for DtsDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }

    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        if self.pending.is_some() {
            return Err(Error::other(
                "dts: receive_frame must be called before sending another packet",
            ));
        }
        self.pending = Some(packet.clone());
        Ok(())
    }

    fn receive_frame(&mut self) -> Result<Frame> {
        let pkt = match self.pending.take() {
            Some(p) => p,
            None => {
                return if self.eof {
                    Err(Error::Eof)
                } else {
                    Err(Error::NeedMore)
                }
            }
        };
        self.process_frame(&pkt)
    }

    fn flush(&mut self) -> Result<()> {
        self.eof = true;
        Ok(())
    }

    fn reset(&mut self) -> Result<()> {
        self.pending = None;
        self.eof = false;
        self.synths.clear();
        Ok(())
    }
}

impl DtsDecoder {
    fn process_frame(&mut self, pkt: &Packet) -> Result<Frame> {
        let data = &pkt.data[..];
        if data.len() < 16 {
            return Err(Error::invalid("dts: packet too short for sync+header"));
        }
        let hdr = header::parse(data)?;
        if (hdr.frame_size as usize) > data.len() {
            return Err(Error::invalid(format!(
                "dts: short packet ({} bytes < frame_size {})",
                data.len(),
                hdr.frame_size
            )));
        }
        self.time_base = TimeBase::new(1, hdr.sample_rate as i64);

        let nch = hdr.primary_channels as usize;
        if self.synths.len() != nch {
            self.synths = (0..nch).map(|_| PqfSynth::new()).collect();
        }

        // Decode subband samples for the entire frame (16 blocks × 32
        // bands × nch). For round 1 we treat parse failures past the
        // audio coding header as "best-effort": we still produce a
        // frame of zeros so the rest of the pipeline survives.
        let frame_bytes = &data[..hdr.frame_size as usize];
        let payload =
            decode_audio_payload(&hdr, frame_bytes).unwrap_or_else(|_| FrameOutput::silence(nch));

        // Run synthesis: 16 blocks × 32 samples → 512 PCM per channel.
        let samples_per_ch = SAMPLES_PER_FRAME as usize;
        let mut pcm_planar: Vec<f64> = vec![0.0; nch * samples_per_ch];
        for ch in 0..nch {
            for blk in 0..16 {
                let mut sb = [0.0f64; SUBBANDS];
                sb[..32].copy_from_slice(&payload.subband[ch][blk][..32]);
                let out = self.synths[ch].synth_block(&sb);
                let off = ch * samples_per_ch + blk * 32;
                pcm_planar[off..off + 32].copy_from_slice(&out[..32]);
            }
        }

        // Pack interleaved S16LE.
        let total_samples = samples_per_ch * nch;
        let mut bytes = vec![0u8; total_samples * 2];
        for n in 0..samples_per_ch {
            for ch in 0..nch {
                let v = pcm_planar[ch * samples_per_ch + n];
                let clamped = (v * 32767.0).clamp(-32768.0, 32767.0) as i16;
                let le = clamped.to_le_bytes();
                let i = n * nch + ch;
                bytes[i * 2] = le[0];
                bytes[i * 2 + 1] = le[1];
            }
        }

        Ok(Frame::Audio(AudioFrame {
            samples: SAMPLES_PER_FRAME,
            pts: pkt.pts,
            data: vec![bytes],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_builds() {
        let params = CodecParameters::audio(CodecId::new("dts"));
        let dec = make_decoder(&params).unwrap();
        assert_eq!(dec.codec_id().as_str(), "dts");
    }
}
