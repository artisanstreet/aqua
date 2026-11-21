#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Occlusion {
    Exposed,
    Covered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Hidden,
    Visible(Occlusion),
    Minimized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Shown(Occlusion),
    Hidden,
    Miniaturized,
    Deminiaturized(Occlusion),
    Occluded(Occlusion),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reports {
    pub covered: Option<bool>,
    pub minimized: Option<bool>,
}

impl Presence {
    pub fn covered(self) -> bool {
        self != Presence::Visible(Occlusion::Exposed)
    }

    pub fn minimized(self) -> bool {
        self == Presence::Minimized
    }

    pub fn apply(self, change: Change) -> (Presence, Reports) {
        let next = match (self, change) {
            (_, Change::Shown(occlusion)) => Presence::Visible(occlusion),
            (_, Change::Hidden) => Presence::Hidden,
            (_, Change::Miniaturized) => Presence::Minimized,
            (_, Change::Deminiaturized(occlusion)) => Presence::Visible(occlusion),
            (Presence::Visible(_), Change::Occluded(occlusion)) => Presence::Visible(occlusion),
            (other, Change::Occluded(_)) => other,
        };
        let changed = |before: bool, after: bool| (before != after).then_some(after);
        let reports = Reports {
            covered: changed(self.covered(), next.covered()),
            minimized: changed(self.minimized(), next.minimized()),
        };
        (next, reports)
    }
}
