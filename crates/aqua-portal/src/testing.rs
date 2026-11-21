use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_lite::{future, StreamExt};

use crate::{Bus, PortalError};

static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

const CONFIG: &str = r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=@DIR@</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#;

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("cannot prepare {path}: {source}")]
    Prepare {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot start {binary}: {source}")]
    Spawn {
        binary: String,
        source: std::io::Error,
    },
    #[error("{binary} exited before printing its address")]
    NoAddress { binary: String },
    #[error("{name} did not appear on the private bus within {after:?}")]
    NameTimeout { name: String, after: Duration },
    #[error(transparent)]
    Bus(#[from] PortalError),
    #[error("watching {name} on the private bus failed: {reason}")]
    Watch { name: String, reason: String },
}

pub fn required() -> bool {
    std::env::var("AQUA_REQUIRE_DBUS").is_ok_and(|value| value == "1")
}

pub fn unavailable<T>(message: String) -> Option<T> {
    match required() {
        true => panic!("{message}; AQUA_REQUIRE_DBUS=1 makes this a failure"),
        false => {
            eprintln!("{message}");
            None
        }
    }
}

pub fn find_binary(name: &str, extra: &[&str]) -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|dir| dir.join(name));
    on_path
        .chain(extra.iter().map(PathBuf::from))
        .find(|candidate| candidate.is_file())
}

pub fn scratch_dir(label: &str) -> Result<PathBuf, HarnessError> {
    let dir = std::env::temp_dir().join(format!(
        "aqua-{label}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let prepare = |source| HarnessError::Prepare {
        path: dir.clone(),
        source,
    };
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(prepare)?;
    std::fs::set_permissions(
        &dir,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
    )
    .map_err(prepare)?;
    Ok(dir)
}

pub struct PrivateBus {
    address: String,
    dir: PathBuf,
    child: Child,
}

impl PrivateBus {
    pub fn start() -> Option<PrivateBus> {
        let Some(binary) = find_binary("dbus-daemon", &[]) else {
            return unavailable(
                "SKIP-NO-DBUS: dbus-daemon is not on PATH (nix-shell nix/linux.nix provides it)"
                    .into(),
            );
        };
        match Self::spawn(&binary) {
            Ok(bus) => Some(bus),
            Err(error) => panic!("the private D-Bus daemon did not start: {error}"),
        }
    }

    fn spawn(binary: &Path) -> Result<PrivateBus, HarnessError> {
        let dir = scratch_dir("dbus")?;
        let config = dir.join("bus.conf");
        std::fs::write(&config, CONFIG.replace("@DIR@", &dir.display().to_string())).map_err(
            |source| HarnessError::Prepare {
                path: config.clone(),
                source,
            },
        )?;
        let label = binary.display().to_string();
        let mut child = Command::new(binary)
            .arg(format!("--config-file={}", config.display()))
            .arg("--nofork")
            .arg("--nopidfile")
            .arg("--print-address=1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|source| HarnessError::Spawn {
                binary: label.clone(),
                source,
            })?;
        let mut line = String::new();
        let read = child
            .stdout
            .take()
            .map(|stdout| BufReader::new(stdout).read_line(&mut line));
        let address = line.trim().to_owned();
        if !matches!(read, Some(Ok(length)) if length > 0) || address.is_empty() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(HarnessError::NoAddress { binary: label });
        }
        Ok(PrivateBus {
            address,
            dir,
            child,
        })
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn bus(&self) -> Bus {
        Bus::connect(&self.address).unwrap_or_else(|error| panic!("{error}"))
    }

    pub fn wait_for_name(&self, name: &str, after: Duration) -> Result<(), HarnessError> {
        let watch = |error: zbus::Error| HarnessError::Watch {
            name: name.to_owned(),
            reason: error.to_string(),
        };
        zbus::block_on(async {
            let connection = zbus::connection::Builder::address(self.address.as_str())
                .map_err(watch)?
                .build()
                .await
                .map_err(watch)?;
            let proxy = zbus::fdo::DBusProxy::new(&connection)
                .await
                .map_err(watch)?;
            let mut owners = proxy
                .receive_name_owner_changed_with_args(&[(0, name)])
                .await
                .map_err(watch)?;
            let bus_name =
                zbus::names::BusName::try_from(name).map_err(|error| watch(error.into()))?;
            if proxy
                .name_has_owner(bus_name)
                .await
                .map_err(|error: zbus::fdo::Error| watch(error.into()))?
            {
                return Ok(());
            }
            future::or(
                async {
                    while let Some(change) = owners.next().await {
                        if change.args().is_ok_and(|args| args.new_owner().is_some()) {
                            return Ok(());
                        }
                    }
                    Err(HarnessError::NameTimeout {
                        name: name.to_owned(),
                        after,
                    })
                },
                async {
                    async_io::Timer::after(after).await;
                    Err(HarnessError::NameTimeout {
                        name: name.to_owned(),
                        after,
                    })
                },
            )
            .await
        })
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
