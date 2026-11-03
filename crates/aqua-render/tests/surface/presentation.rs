use aqua_render::surface::choose_present_mode;
use wgpu::PresentMode::*;

#[test]
fn presentation_preferences_resolve_automatic_modes_and_reject_incompatible_surfaces() {
    let modes = [Immediate, Mailbox, Fifo, FifoRelaxed];
    assert_eq!(choose_present_mode(&modes, AutoNoVsync).unwrap(), Immediate);
    assert_eq!(
        choose_present_mode(&modes[1..], AutoNoVsync).unwrap(),
        Mailbox
    );
    assert_eq!(choose_present_mode(&[Fifo], AutoNoVsync).unwrap(), Fifo);
    assert_eq!(choose_present_mode(&modes, AutoVsync).unwrap(), FifoRelaxed);
    assert_eq!(choose_present_mode(&[Fifo], AutoVsync).unwrap(), Fifo);
    assert_eq!(choose_present_mode(&modes, Fifo).unwrap(), Fifo);
    assert_eq!(choose_present_mode(&[Fifo], Mailbox).unwrap(), Fifo);
    assert!(choose_present_mode(&[], AutoVsync).is_err());
    assert!(choose_present_mode(&[Immediate], AutoVsync).is_err());
}
