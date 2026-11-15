//! When a procedural texture takes a simulation step: the engine's texture tick (`UTexture::Update` / `Tick`).

/// Steps per second for a texture that asks for "every frame" (`MaxFrameRate` 0). The original steps such a texture
/// once per drawn frame, so its water ran faster on faster machines; the port pins it to the simulation rate.
pub const EVERY_FRAME_RATE: f32 = 60.0;

/// The engine's per-texture timing state.
#[derive(Debug, Clone)]
pub struct AnimClock {
    pub min_frame_rate: f32,
    pub max_frame_rate: f32,
    /// Steps run once, the first time the texture is drawn (so water starts already rippling).
    pub prime_count: u8,
    prime_current: u8,
    accumulator: f32,
    /// Time of the last update (`InternalTime`; starts at 0 like the engine's).
    last: f64,
}

impl AnimClock {
    pub fn new(min_frame_rate: f32, max_frame_rate: f32, prime_count: u8) -> AnimClock {
        AnimClock { min_frame_rate, max_frame_rate, prime_count, prime_current: 0, accumulator: 0.0, last: 0.0 }
    }

    /// The texture is drawn at `time` (seconds): returns how many simulation steps to run now. Nothing happens when the
    /// time has not changed since the last call.
    pub fn update(&mut self, time: f64) -> u32 {
        if time == self.last {
            return 0;
        }
        // (the engine's viewport clock never runs backwards; if a caller's does, restart from the new time)
        let dt = (time - self.last).max(0.0) as f32;
        self.last = time;
        let mut steps = 0;
        while self.prime_current < self.prime_count {
            self.prime_current += 1;
            steps += 1;
        }
        // rates are clamped to 0.01..100 steps per second, then the accumulated time decides
        let (max_rate, min_rate) = if self.max_frame_rate == 0.0 {
            (EVERY_FRAME_RATE, EVERY_FRAME_RATE)
        } else {
            (self.max_frame_rate, self.min_frame_rate)
        };
        let shortest = 1.0 / max_rate.clamp(0.01, 100.0);
        let longest = 1.0 / min_rate.clamp(0.01, 100.0);
        self.accumulator += dt;
        if shortest <= self.accumulator {
            steps += 1;
            if self.accumulator < longest {
                self.accumulator = 0.0;
            } else {
                self.accumulator -= longest;
                if longest < self.accumulator {
                    self.accumulator = longest;
                }
            }
        }
        steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primes_once_then_steps_at_the_rate() {
        // 15..20 steps per second, drawn at 100 frames per second for 2 s
        let mut c = AnimClock::new(15.0, 20.0, 5);
        let first = c.update(0.01);
        assert_eq!(first, 5);
        let mut n = 0;
        for i in 2..=200 {
            n += c.update(i as f64 * 0.01);
        }
        // a step whenever 1/20 s has gathered (the accumulator restarts at 0): every 5th frame, or every 6th when five
        // single-precision hundredths add up to just under 1/20
        assert!((33..=40).contains(&n), "{n} steps");
    }

    #[test]
    fn same_time_twice_does_nothing() {
        let mut c = AnimClock::new(0.0, 0.0, 0);
        assert_eq!(c.update(1.0), 1);
        assert_eq!(c.update(1.0), 0);
    }

    #[test]
    fn every_frame_textures_step_at_most_once_per_frame() {
        let mut c = AnimClock::new(0.0, 0.0, 0);
        c.update(1.0);
        // 30 fps: below the pinned rate, one step per frame
        let slow: u32 = (1..=30).map(|i| c.update(1.0 + i as f64 / 30.0)).sum();
        assert_eq!(slow, 30);
        // 240 fps for one second: about 60 steps
        let fast: u32 = (1..=240).map(|i| c.update(2.0 + i as f64 / 240.0)).sum();
        assert!((58..=61).contains(&fast), "{fast} steps");
    }
}
