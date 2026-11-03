#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CursorIcon {
    #[default]
    Default,
    Text,
    Pointer,
    Crosshair,
    Move,
    NotAllowed,
    EwResize,
    NsResize,
    ColResize,
    RowResize,
    Grab,
    Grabbing,
    NeswResize,
    NwseResize,
    Wait,
    Progress,
    Help,
    Hidden,
}
