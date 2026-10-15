//! Keeps one resampler alive across tracks that share a sample rate so the
//! sinc delay does not insert a gap. A rate change flushes, then rebuilds.

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

const CHUNK: usize = 512;

pub struct StereoResampler {
    inner: Option<SincFixedIn<f32>>,
    in_rate: u32,
    out_rate: u32,
    pending_l: Vec<f32>,
    pending_r: Vec<f32>,
}

impl StereoResampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Result<Self, String> {
        let in_rate = in_rate.max(1);
        let out_rate = out_rate.max(1);
        let inner = if in_rate == out_rate {
            None
        } else {
            Some(make(in_rate, out_rate)?)
        };
        Ok(Self {
            inner,
            in_rate,
            out_rate,
            pending_l: Vec::new(),
            pending_r: Vec::new(),
        })
    }

    pub fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
        if self.inner.is_none() {
            return interleaved.to_vec();
        }
        for frame in interleaved.chunks_exact(2) {
            self.pending_l.push(frame[0]);
            self.pending_r.push(frame[1]);
        }
        self.drain_chunks()
    }

    /// Pad and emit the leftover input. The padded tail is trimmed so a rate
    /// change does not append a block of silence.
    pub fn flush(&mut self) -> Vec<f32> {
        let Some(resampler) = self.inner.as_mut() else {
            self.pending_l.clear();
            self.pending_r.clear();
            return Vec::new();
        };
        let have = self.pending_l.len();
        if have == 0 {
            resampler.reset();
            return Vec::new();
        }
        let remainder = have % CHUNK;
        let pad = if remainder == 0 { 0 } else { CHUNK - remainder };
        self.pending_l.extend(std::iter::repeat_n(0.0, pad));
        self.pending_r.extend(std::iter::repeat_n(0.0, pad));
        let mut out = self.drain_chunks();
        self.pending_l.clear();
        self.pending_r.clear();
        if let Some(resampler) = self.inner.as_mut() {
            resampler.reset();
        }
        let ratio = f64::from(self.out_rate) / f64::from(self.in_rate.max(1));
        let trim = (pad as f64 * ratio).round() as usize;
        let frames = out.len() / 2;
        if trim > 0 && trim < frames {
            out.truncate((frames - trim) * 2);
        }
        out
    }

    pub fn reset(&mut self) {
        self.pending_l.clear();
        self.pending_r.clear();
        if let Some(resampler) = self.inner.as_mut() {
            resampler.reset();
        }
    }

    pub fn set_input_rate(&mut self, rate: u32) -> Result<Vec<f32>, String> {
        let flushed = self.flush();
        self.in_rate = rate.max(1);
        self.inner = if self.in_rate == self.out_rate {
            None
        } else {
            Some(make(self.in_rate, self.out_rate)?)
        };
        Ok(flushed)
    }

    fn drain_chunks(&mut self) -> Vec<f32> {
        let mut out = Vec::new();
        while self.pending_l.len() >= CHUNK {
            let left: Vec<f32> = self.pending_l.drain(..CHUNK).collect();
            let right: Vec<f32> = self.pending_r.drain(..CHUNK).collect();
            let Some(resampler) = self.inner.as_mut() else {
                break;
            };
            if let Ok(planes) = resampler.process(&[left, right], None) {
                interleave(&planes, &mut out);
            }
        }
        out
    }
}

fn make(in_rate: u32, out_rate: u32) -> Result<SincFixedIn<f32>, String> {
    let ratio = f64::from(out_rate) / f64::from(in_rate.max(1));
    let params = SincInterpolationParameters {
        sinc_len: 128,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 128,
        window: WindowFunction::BlackmanHarris2,
    };
    SincFixedIn::<f32>::new(ratio, 2.0, params, CHUNK, 2)
        .map_err(|err| format!("无法建立重采样器：{err}"))
}

fn interleave(planes: &[Vec<f32>], out: &mut Vec<f32>) {
    if planes.len() < 2 {
        return;
    }
    let frames = planes[0].len().min(planes[1].len());
    out.reserve(frames * 2);
    for index in 0..frames {
        out.push(planes[0][index]);
        out.push(planes[1][index]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_rate_is_passthrough() {
        let mut resampler = StereoResampler::new(44_100, 44_100).unwrap();
        let input = vec![0.25, -0.25, 0.5, -0.5];
        assert_eq!(resampler.process(&input), input);
        assert!(resampler.flush().is_empty());
    }

    #[test]
    fn different_rate_emits_samples() {
        let mut resampler = StereoResampler::new(44_100, 48_000).unwrap();
        let mut input = Vec::new();
        for index in 0..2048 {
            let sample = (index as f32 * 0.01).sin() * 0.2;
            input.push(sample);
            input.push(sample);
        }
        let output = resampler.process(&input);
        assert!(output.len() > input.len());
        assert_eq!(output.len() % 2, 0);
    }
}
