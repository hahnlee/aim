//! Original Java Animation samples, evaluated on the display's vsync clock.
use std::io;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::wire::{self, Request};
use aim_hostcall::display::{Connect, Event, event};

/// Matrix (dsdx, dtdx, dtdy, dsdy), translation, alpha, clip (LTRB).
pub const STRIDE: usize = 11;
pub const IDENTITY: [f32; STRIDE] = [1., 0., 1., 0., 0., 0., 1., 0., 0., -1., -1.];

pub struct Animation(pub Vec<f32>);
impl Animation {
    pub fn new(samples: Vec<f32>) -> io::Result<Self> {
        if samples.len() % STRIDE != 0
            || samples.len() > 3001 * STRIDE
            || samples.iter().any(|v| !v.is_finite())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "animation samples",
            ));
        }
        Ok(Self(samples))
    }

    /// One sample per millisecond, including the endpoint; no wall-clock timer.
    pub fn at(&self, elapsed_ns: i64) -> ([f32; STRIDE], bool) {
        if self.0.is_empty() {
            return (IDENTITY, false);
        }
        let last = self.0.len() / STRIDE - 1;
        let ms = elapsed_ns.max(0) as f64 / 1_000_000.;
        let i = (ms as usize).min(last);
        let j = (i + 1).min(last);
        let t = (ms - i as f64).min(1.) as f32;
        (
            std::array::from_fn(|k| self.0[i * STRIDE + k] * (1. - t) + self.0[j * STRIDE + k] * t),
            ms < last as f64,
        )
    }
}

/// A dedicated observer: connecting it must not reset HWC layer attribution.
pub fn vsyncs(socket: &Path) -> io::Result<UnixStream> {
    let stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    wire::send(
        stream.as_fd(),
        wire::bytes(&Request {
            op: wire::OP_VSYNC_OBSERVER,
            id: wire::VERSION,
            ..Default::default()
        }),
        None,
    )?;
    wire::recv_record::<Connect>(stream.as_fd())?.ok_or(io::ErrorKind::UnexpectedEof)?;
    wire::send(
        stream.as_fd(),
        wire::bytes(&Request {
            op: wire::OP_SET_VSYNC,
            flag: 1,
            ..Default::default()
        }),
        None,
    )?;
    Ok(stream)
}

pub fn next_vsync(stream: &UnixStream) -> io::Result<i64> {
    loop {
        let event =
            wire::recv_record::<Event>(stream.as_fd())?.ok_or(io::ErrorKind::UnexpectedEof)?;
        if event.kind == event::VSYNC {
            return Ok(event.timestamp_ns);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compositor_transforms_advance_on_vsync_and_reach_the_endpoint() {
        let mut samples = Vec::new();
        for ms in 0..=100 {
            let mut s = IDENTITY;
            s[4] = 100. - ms as f32;
            s[6] = ms as f32 / 100.;
            samples.extend(s);
        }
        let animation = Animation::new(samples).unwrap();
        for period in [16_666_667, 8_333_333] {
            let mut previous = 101.;
            for tick in 0..=14 {
                let (frame, running) = animation.at(tick * period);
                assert!(frame[4] <= previous);
                assert_eq!(running, tick * period < 100_000_000);
                previous = frame[4];
            }
        }
        assert_eq!(animation.at(100_000_000), (IDENTITY, false));
        assert!(Animation::new(vec![f32::NAN; STRIDE]).is_err());
    }
}
