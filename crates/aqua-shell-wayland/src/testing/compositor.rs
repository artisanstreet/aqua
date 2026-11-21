use super::ipc::{self, IpcError};
use super::HarnessError;
use crate::state::{self, Endpoint};
use aqua_shell::ShellError;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

const BASE_CONFIG: &str = include_str!("../../../../tests/sway.conf");
const STARTUP: Duration = Duration::from_secs(10);
static SEQUENCE: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub config: Vec<String>,
    pub hidden_globals: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

pub struct Compositor {
    child: Child,
    runtime: PathBuf,
    display: PathBuf,
    ipc: PathBuf,
    hidden_globals: Vec<String>,
}

fn binary() -> String {
    std::env::var("AQUA_TEST_COMPOSITOR").unwrap_or_else(|_| "sway".into())
}

fn on_path(binary: &str) -> bool {
    match binary.contains('/') {
        true => Path::new(binary).is_file(),
        false => std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(binary).is_file())),
    }
}

fn required() -> bool {
    std::env::var("AQUA_REQUIRE_WAYLAND").is_ok_and(|value| value == "1")
}

fn find_socket(runtime: &Path, accept: impl Fn(&str) -> bool) -> Option<PathBuf> {
    std::fs::read_dir(runtime)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(&accept)
                && std::fs::metadata(path)
                    .is_ok_and(|meta| std::os::unix::fs::FileTypeExt::is_socket(&meta.file_type()))
        })
}

impl Compositor {
    pub fn start() -> Option<Compositor> {
        Self::start_with(Options::default())
    }

    pub fn start_with(options: Options) -> Option<Compositor> {
        let binary = binary();
        if !on_path(&binary) {
            let message = format!(
                "SKIP-NO-COMPOSITOR: {binary} is not on PATH (nix-shell nix/linux.nix provides it)"
            );
            match required() {
                true => panic!("{message}; AQUA_REQUIRE_WAYLAND=1 makes this a failure"),
                false => {
                    eprintln!("{message}");
                    return None;
                }
            }
        }
        match Self::spawn(&binary, options) {
            Ok(compositor) => Some(compositor),
            Err(error) => panic!("the headless compositor did not start: {error}"),
        }
    }

    fn spawn(binary: &str, options: Options) -> Result<Compositor, HarnessError> {
        let runtime = std::env::temp_dir().join(format!(
            "aqua-wayland-test-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let prepare = |result: std::io::Result<()>| {
            result.map_err(|source| HarnessError::RuntimeDir {
                path: runtime.clone(),
                source,
            })
        };
        let _ = std::fs::remove_dir_all(&runtime);
        prepare(std::fs::create_dir_all(&runtime))?;
        prepare(std::fs::set_permissions(
            &runtime,
            std::fs::Permissions::from_mode(0o700),
        ))?;
        let config = runtime.join("sway.conf");
        let text = std::iter::once(BASE_CONFIG.to_owned())
            .chain(options.config.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n");
        prepare(std::fs::write(&config, text))?;
        let log = runtime.join("compositor.log");
        let output = std::fs::File::create(&log).map_err(|source| HarnessError::RuntimeDir {
            path: log.clone(),
            source,
        })?;
        let errors = output
            .try_clone()
            .map_err(|source| HarnessError::RuntimeDir {
                path: log.clone(),
                source,
            })?;
        let mut child = Command::new(binary)
            .arg("--config")
            .arg(&config)
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .env_remove("SWAYSOCK")
            .env_remove("WAYLAND_DEBUG")
            .env_remove("WAYLAND_SOCKET")
            .env("XDG_RUNTIME_DIR", &runtime)
            .env(
                "AT_SPI_BUS_ADDRESS",
                format!(
                    "unix:path={}",
                    runtime.join("no-accessibility-bus").display()
                ),
            )
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}", runtime.join("no-session-bus").display()),
            )
            .env("WLR_BACKENDS", "headless")
            .env("WLR_RENDERER", "pixman")
            .env("WLR_LIBINPUT_NO_DEVICES", "1")
            .stdin(Stdio::null())
            .stdout(output)
            .stderr(errors)
            .spawn()
            .map_err(|source| HarnessError::Spawn {
                binary: binary.to_owned(),
                source,
            })?;
        let started = std::time::Instant::now();
        let read_log = || std::fs::read_to_string(&log).unwrap_or_default();
        loop {
            let display = find_socket(&runtime, |name| {
                name.starts_with("wayland-") && !name.ends_with(".lock")
            });
            let ipc = find_socket(&runtime, |name| {
                name.starts_with("sway-ipc.") && name.ends_with(".sock")
            });
            if let (Some(display), Some(ipc)) = (display, ipc) {
                let compositor = Compositor {
                    child,
                    runtime,
                    display,
                    ipc,
                    hidden_globals: options.hidden_globals,
                };
                compositor.settle();
                return Ok(compositor);
            }
            if let Ok(Some(_)) = child.try_wait() {
                return Err(HarnessError::Exited {
                    binary: binary.to_owned(),
                    log: read_log(),
                });
            }
            if started.elapsed() > STARTUP {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HarnessError::NoSockets {
                    binary: binary.to_owned(),
                    waited: STARTUP,
                    log: read_log(),
                });
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn settle(&self) {
        let started = std::time::Instant::now();
        while self.outputs().is_err() && started.elapsed() < STARTUP {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn display(&self) -> &Path {
        &self.display
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.runtime
    }

    pub fn attach(&self) -> Result<(), ShellError> {
        state::connect(Endpoint::Socket {
            path: self.display.clone(),
            hidden: self.hidden_globals.clone(),
        })
    }

    pub fn command(&self, command: &str) -> Result<serde_json::Value, IpcError> {
        let reply = ipc::request(&self.ipc, ipc::RUN_COMMAND, command)?;
        let failed = reply
            .as_array()
            .into_iter()
            .flatten()
            .find(|result| result["success"] != serde_json::Value::Bool(true));
        match failed {
            Some(result) => Err(IpcError::Refused {
                command: command.to_owned(),
                reply: result.to_string(),
            }),
            None => Ok(reply),
        }
    }

    pub fn tree(&self) -> Result<serde_json::Value, IpcError> {
        ipc::request(&self.ipc, ipc::GET_TREE, "")
    }

    pub fn outputs(&self) -> Result<serde_json::Value, IpcError> {
        ipc::request(&self.ipc, ipc::GET_OUTPUTS, "")
    }

    pub fn view(&self, app_id: &str) -> Option<serde_json::Value> {
        self.tree().ok().and_then(|tree| find_view(&tree, app_id))
    }

    pub fn view_rect(&self, app_id: &str) -> Option<Rect> {
        self.view(app_id).and_then(|view| rect_of(&view["rect"]))
    }
}

pub(crate) fn rect_of(value: &serde_json::Value) -> Option<Rect> {
    let field = |name: &str| value[name].as_i64().and_then(|v| i32::try_from(v).ok());
    Some(Rect {
        x: field("x")?,
        y: field("y")?,
        width: field("width")?,
        height: field("height")?,
    })
}

fn find_view(node: &serde_json::Value, app_id: &str) -> Option<serde_json::Value> {
    if node["app_id"].as_str() == Some(app_id) {
        return Some(node.clone());
    }
    ["nodes", "floating_nodes"]
        .iter()
        .filter_map(|key| node[*key].as_array())
        .flatten()
        .find_map(|child| find_view(child, app_id))
}

impl Drop for Compositor {
    fn drop(&mut self) {
        if state::connected() {
            state::disconnect();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.runtime);
    }
}
