use aqua_protocol::{Instant, RenderWake};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WakeTally {
    pub idle: u64,
    pub next_frame: u64,
    pub timed: u64,
}

impl WakeTally {
    pub fn record(&mut self, wake: RenderWake) {
        match wake {
            RenderWake::Idle => self.idle += 1,
            RenderWake::NextFrame => self.next_frame += 1,
            RenderWake::At(_) => self.timed += 1,
        }
    }

    pub fn record_deadline(&mut self, deadline: Option<Instant>) {
        match deadline {
            None => self.idle += 1,
            Some(_) => self.timed += 1,
        }
    }

    pub fn requested(self) -> u64 {
        self.next_frame + self.timed
    }

    pub fn total(self) -> u64 {
        self.idle + self.requested()
    }
}
