pub use aqua_shell::notifications::activation::{
    activation_url, parse_activation_url, Activation, ActivationUrlError, Launch, ACTIVATION_FLAG,
    SCHEME,
};

pub fn handle_launch() -> Launch {
    Launch::Ordinary
}
