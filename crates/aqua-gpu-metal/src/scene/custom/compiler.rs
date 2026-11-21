use std::ffi::{c_int, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use libloading::os::unix::{Library, RTLD_LOCAL, RTLD_NOW};

use super::super::dxc::{self, Compiler, LIBRARY as DXCOMPILER};
use super::super::spirv_cross::{self, Translator, LIBRARY as SPIRV_CROSS};

const RTLD_NODELETE: c_int = 0x80;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LibrarySource {
    #[default]
    Search,
    Library(PathBuf),
}

impl LibrarySource {
    pub fn from_env(variable: &str) -> Self {
        std::env::var_os(variable)
            .filter(|path| !path.is_empty())
            .map_or(LibrarySource::Search, |path| {
                LibrarySource::Library(path.into())
            })
    }

    fn candidates(&self, name: &str) -> Vec<OsString> {
        match self {
            LibrarySource::Library(path) => vec![path.clone().into_os_string()],
            LibrarySource::Search => std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf))
                .map(|dir| {
                    [dir.join("../Frameworks"), dir.clone(), dir.join("../lib")]
                        .map(|folder| folder.join(name).into_os_string())
                        .to_vec()
                })
                .unwrap_or_default()
                .into_iter()
                .chain([name.into()])
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShaderCompiler {
    Enabled {
        dxcompiler: LibrarySource,
        spirv_cross: LibrarySource,
    },
    Disabled,
}

impl Default for ShaderCompiler {
    fn default() -> Self {
        ShaderCompiler::Enabled {
            dxcompiler: LibrarySource::Search,
            spirv_cross: LibrarySource::Search,
        }
    }
}

impl ShaderCompiler {
    pub fn from_env() -> Self {
        ShaderCompiler::Enabled {
            dxcompiler: LibrarySource::from_env("AQUA_DXCOMPILER"),
            spirv_cross: LibrarySource::from_env("AQUA_SPIRV_CROSS_C"),
        }
    }
}

#[derive(Debug)]
pub struct Attempt {
    pub location: OsString,
    pub reason: String,
}

pub(in crate::scene) struct Attempts<'a>(pub &'a [Attempt]);

impl fmt::Display for Attempts<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for attempt in self.0 {
            let separator = if first { "" } else { "; " };
            first = false;
            write!(
                f,
                "{separator}{}: {}",
                attempt.location.to_string_lossy(),
                attempt.reason
            )?;
        }
        Ok(())
    }
}

pub(in crate::scene) fn open(
    source: &LibrarySource,
    name: &str,
) -> Result<(Library, OsString), Vec<Attempt>> {
    let mut attempts = Vec::new();
    for location in source.candidates(name) {
        let flags = RTLD_NOW | RTLD_LOCAL | RTLD_NODELETE;
        match unsafe { Library::open(Some(&location), flags) } {
            Ok(library) => return Ok((library, location)),
            Err(error) => attempts.push(Attempt {
                location,
                reason: error.to_string(),
            }),
        }
    }
    Err(attempts)
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("runtime shader compilation is disabled")]
    Disabled,
    #[error(transparent)]
    Dxc(dxc::LoadError),
    #[error(transparent)]
    SpirvCross(spirv_cross::LoadError),
    #[error("{dxc}; {spirv_cross}")]
    Both {
        dxc: Box<dxc::LoadError>,
        spirv_cross: Box<spirv_cross::LoadError>,
    },
}

#[derive(Debug)]
pub struct Toolchain {
    pub compiler: Compiler,
    pub translator: Translator,
}

#[derive(Clone, Debug)]
pub enum RuntimeCompiler {
    Ready(Arc<Toolchain>),
    Unavailable(Arc<LoadError>),
}

fn load(choice: &ShaderCompiler) -> Result<Toolchain, LoadError> {
    let ShaderCompiler::Enabled {
        dxcompiler,
        spirv_cross,
    } = choice
    else {
        return Err(LoadError::Disabled);
    };
    match (Compiler::load(dxcompiler), Translator::load(spirv_cross)) {
        (Ok(compiler), Ok(translator)) => Ok(Toolchain {
            compiler,
            translator,
        }),
        (Err(dxc), Ok(_)) => Err(LoadError::Dxc(dxc)),
        (Ok(_), Err(spirv_cross)) => Err(LoadError::SpirvCross(spirv_cross)),
        (Err(dxc), Err(spirv_cross)) => Err(LoadError::Both {
            dxc: Box::new(dxc),
            spirv_cross: Box::new(spirv_cross),
        }),
    }
}

impl RuntimeCompiler {
    pub fn load(choice: &ShaderCompiler) -> Self {
        match load(choice) {
            Ok(toolchain) => RuntimeCompiler::Ready(Arc::new(toolchain)),
            Err(LoadError::Disabled) => RuntimeCompiler::Unavailable(Arc::new(LoadError::Disabled)),
            Err(error) => {
                eprintln!("aqua: custom shaders are unavailable on Metal: {error}; set AQUA_DXCOMPILER to a {DXCOMPILER} from DXC 1.7.2308 or newer and AQUA_SPIRV_CROSS_C to a {SPIRV_CROSS}, or bundle both in the app's Contents/Frameworks");
                RuntimeCompiler::Unavailable(Arc::new(error))
            }
        }
    }

    pub fn ready(&self) -> Option<&Toolchain> {
        match self {
            RuntimeCompiler::Ready(toolchain) => Some(toolchain),
            RuntimeCompiler::Unavailable(_) => None,
        }
    }

    pub fn error(&self) -> Option<&LoadError> {
        match self {
            RuntimeCompiler::Ready(_) => None,
            RuntimeCompiler::Unavailable(error) => Some(error),
        }
    }
}
