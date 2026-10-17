//! How long the sound has been silent, so the output device is let go of while nothing plays:
//! a doze that stops the game lets it go, and a suspend to RAM never finds it open. Ported
//! from slot.advance (suprsomething).

use std::time::Duration;

pub struct Silence {
    heard: Duration,
    after: Duration,
}

impl Silence {
    pub fn new(after: Duration) -> Self {
        Silence {
            heard: Duration::ZERO,
            after,
        }
    }

    /// Hears `samples`, `span` of sound; whether the device should still be held.
    pub fn hear(&mut self, samples: &[i16], span: Duration) -> bool {
        match samples.iter().all(|&s| s == 0) {
            true => self.heard = self.heard.saturating_add(span),
            false => self.heard = Duration::ZERO,
        }
        self.heard < self.after
    }
}
