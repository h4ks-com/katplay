//! The render scale governor: it watches how long each frame takes to reach
//! the terminal and picks the scale whose frames fit the frame budget, since
//! a frame's cost grows with the square of the scale.

use std::time::{Duration, Instant};

/// How many delivered frames we judge at once.
const SAMPLES: usize = 30;
/// How long a scale that proved too slow stays out of reach, so cheap
/// frames in a quiet moment do not grow us straight back into it.
const REMEMBER_TOO_SLOW: Duration = Duration::from_secs(30);
/// The share of the frame time we aim to spend delivering a frame.
const AIM: f64 = 0.6;
/// Above this share the terminal falls behind and we shrink.
const TOO_SLOW: f64 = 0.85;
/// Below this share there is room to grow.
const ROOM: f64 = 0.3;
/// The finest step between scales, so small swings do not resize the picture.
const STEP: f64 = 0.05;

#[derive(Debug)]
pub(crate) struct Governor {
    frame_time: Duration,
    scale: f64,
    min: f64,
    max: f64,
    samples: Vec<Duration>,
    too_slow: Option<(f64, Instant)>,
}

impl Governor {
    /// A governor for `frame_time` that starts at `max` and stays within
    /// `min..=max`.
    pub(crate) fn new(frame_time: Duration, min: f64, max: f64) -> Self {
        Self {
            frame_time,
            scale: max,
            min,
            max,
            samples: Vec::with_capacity(SAMPLES),
            too_slow: None,
        }
    }

    pub(crate) fn scale(&self) -> f64 {
        self.scale
    }

    /// The largest scale we may grow to at `now`.
    fn ceiling(&self, now: Instant) -> f64 {
        match self.too_slow {
            Some((scale, at)) if now.duration_since(at) < REMEMBER_TOO_SLOW => scale - STEP,
            _ => self.max,
        }
    }

    /// Record that one frame took `took` to reach the terminal, at `now`;
    /// the new scale when the frames so far call for one.
    pub(crate) fn delivered(&mut self, took: Duration, now: Instant) -> Option<f64> {
        self.samples.push(took);
        if self.samples.len() < SAMPLES {
            return None;
        }
        self.samples.sort_unstable();
        let median = self.samples.get(SAMPLES / 2).copied().unwrap_or_default();
        self.samples.clear();
        let share = median.as_secs_f64() / self.frame_time.as_secs_f64();
        if share <= TOO_SLOW && (share >= ROOM || self.scale >= self.max) {
            return None;
        }
        let wanted = self.scale * (AIM / share.max(0.01)).sqrt();
        let mut next = ((wanted / STEP).round() * STEP).clamp(self.min, self.max);
        if share > TOO_SLOW {
            self.too_slow = Some((self.scale, now));
        } else {
            next = next.min(self.ceiling(now)).max(self.scale);
        }
        if (next - self.scale).abs() < STEP / 2.0 {
            return None;
        }
        self.scale = next;
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Governor, REMEMBER_TOO_SLOW, SAMPLES};

    const FRAME: Duration = Duration::from_micros(16_667);

    fn feed(governor: &mut Governor, took: Duration, now: Instant) -> Option<f64> {
        (0..SAMPLES)
            .map(|_| governor.delivered(took, now))
            .last()
            .flatten()
    }

    #[test]
    fn a_slow_terminal_shrinks_the_scale_to_fit_the_budget() {
        let now = Instant::now();
        let mut governor = Governor::new(FRAME, 0.25, 1.0);
        let shrunk = feed(&mut governor, FRAME * 4, now).expect("four frame times is too slow");
        assert!((0.35..=0.45).contains(&shrunk), "{shrunk}");
        assert_eq!(
            feed(&mut governor, FRAME / 2, now),
            None,
            "half the budget fits"
        );
    }

    #[test]
    fn a_scale_that_was_too_slow_stays_out_of_reach_for_a_while() {
        let now = Instant::now();
        let mut governor = Governor::new(FRAME, 0.25, 1.0);
        let shrunk = feed(&mut governor, FRAME * 2, now).expect("two frame times is too slow");
        let held = feed(&mut governor, FRAME / 20, now).expect("plenty of room");
        assert!(
            held < 1.0 && held > shrunk,
            "grows, but not into 1.0: {held}"
        );
        let later = now + REMEMBER_TOO_SLOW;
        assert_eq!(feed(&mut governor, FRAME / 20, later), Some(1.0));
    }

    #[test]
    fn the_scale_stays_within_its_range() {
        let now = Instant::now();
        let mut governor = Governor::new(FRAME, 0.25, 0.5);
        feed(&mut governor, FRAME * 8, now);
        assert_eq!(governor.scale(), 0.25, "never below the minimum");
        let later = now + REMEMBER_TOO_SLOW;
        assert_eq!(feed(&mut governor, FRAME / 20, later), Some(0.5));
        assert_eq!(feed(&mut governor, FRAME / 20, later), None);
    }
}
