use std::ffi::{c_char, c_uint, c_void, CStr, OsString};
use std::fmt;
use std::ptr::{null, null_mut};

use libloading::os::unix::Library;

use super::custom::compiler::{open, Attempt, Attempts, LibrarySource};
use super::map::{slot, Slot, BINDINGS, LANGUAGE, SET};

pub const LIBRARY: &str = "libspirv-cross-c-shared.dylib";

const SUCCESS: i32 = 0;
const BACKEND_MSL: c_uint = 3;
const CAPTURE_TAKE_OWNERSHIP: c_uint = 1;
const OPTION_MSL_VERSION: c_uint = 17 | 0x0800_0000;
const OPTION_MSL_PLATFORM: c_uint = 31 | 0x0800_0000;
const PLATFORM_MACOS: c_uint = 1;
const MODEL_COMPUTE: c_uint = 5;
const MODE_LOCAL_SIZE: c_uint = 17;

type Handle = *mut c_void;

#[repr(C)]
#[derive(Default)]
struct ResourceBinding {
    stage: c_uint,
    desc_set: c_uint,
    binding: c_uint,
    count: c_uint,
    msl_buffer: c_uint,
    msl_texture: c_uint,
    msl_sampler: c_uint,
}

#[repr(C)]
struct EntryPoint {
    execution_model: c_uint,
    name: *const c_char,
}

#[derive(Clone, Copy)]
struct Api {
    context_create: unsafe extern "C" fn(*mut Handle) -> i32,
    context_destroy: unsafe extern "C" fn(Handle),
    last_error: unsafe extern "C" fn(Handle) -> *const c_char,
    parse: unsafe extern "C" fn(Handle, *const u32, usize, *mut Handle) -> i32,
    create_compiler: unsafe extern "C" fn(Handle, c_uint, Handle, c_uint, *mut Handle) -> i32,
    execution_model: unsafe extern "C" fn(Handle) -> c_uint,
    binding_init: unsafe extern "C" fn(*mut ResourceBinding),
    add_binding: unsafe extern "C" fn(Handle, *const ResourceBinding) -> i32,
    create_options: unsafe extern "C" fn(Handle, *mut Handle) -> i32,
    set_uint: unsafe extern "C" fn(Handle, c_uint, c_uint) -> i32,
    install_options: unsafe extern "C" fn(Handle, Handle) -> i32,
    compile: unsafe extern "C" fn(Handle, *mut *const c_char) -> i32,
    entry_points: unsafe extern "C" fn(Handle, *mut *const EntryPoint, *mut usize) -> i32,
    cleansed_name: unsafe extern "C" fn(Handle, *const c_char, c_uint) -> *const c_char,
    mode_argument: unsafe extern "C" fn(Handle, c_uint, c_uint) -> c_uint,
}

fn symbol<T: Copy>(library: &Library, name: &'static str) -> Result<T, libloading::Error> {
    let terminated = format!("{name}\0");
    unsafe { library.get::<T>(terminated.as_bytes()) }.map(|symbol| *symbol)
}

impl Api {
    fn bind(library: &Library) -> Result<Self, (&'static str, libloading::Error)> {
        macro_rules! resolve {
            ($name:literal) => {
                symbol(library, $name).map_err(|error| ($name, error))?
            };
        }
        Ok(Self {
            context_create: resolve!("spvc_context_create"),
            context_destroy: resolve!("spvc_context_destroy"),
            last_error: resolve!("spvc_context_get_last_error_string"),
            parse: resolve!("spvc_context_parse_spirv"),
            create_compiler: resolve!("spvc_context_create_compiler"),
            execution_model: resolve!("spvc_compiler_get_execution_model"),
            binding_init: resolve!("spvc_msl_resource_binding_init_2"),
            add_binding: resolve!("spvc_compiler_msl_add_resource_binding_2"),
            create_options: resolve!("spvc_compiler_create_compiler_options"),
            set_uint: resolve!("spvc_compiler_options_set_uint"),
            install_options: resolve!("spvc_compiler_install_compiler_options"),
            compile: resolve!("spvc_compiler_compile"),
            entry_points: resolve!("spvc_compiler_get_entry_points"),
            cleansed_name: resolve!("spvc_compiler_get_cleansed_entry_point_name"),
            mode_argument: resolve!("spvc_compiler_get_execution_mode_argument_by_index"),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("{LIBRARY} could not be loaded: {}", Attempts(.attempts))]
    NotFound { attempts: Vec<Attempt> },
    #[error("{} exports no {symbol}: {source}", .location.to_string_lossy())]
    NoEntryPoint {
        location: OsString,
        symbol: &'static str,
        source: libloading::Error,
    },
    #[error("{} could not create a context: spvc_result {result}", .location.to_string_lossy())]
    Context { location: OsString, result: i32 },
}

#[derive(Debug, thiserror::Error)]
pub enum TranslateError {
    #[error("spvc_context_create failed with spvc_result {0}")]
    Context(i32),
    #[error("SPIRV-Cross {step} failed: {reason}")]
    Rejected { step: &'static str, reason: String },
    #[error("SPIRV-Cross found {0} entry points where the compiler wrote one")]
    EntryPoints(usize),
}

#[derive(Debug)]
pub struct Msl {
    pub source: String,
    pub entry: String,
}

#[derive(Debug)]
pub struct Kernel {
    pub msl: Msl,
    pub threads: [u32; 3],
}

pub struct Translator {
    api: Api,
    location: OsString,
    _library: Library,
}

impl fmt::Debug for Translator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Translator")
            .field("location", &self.location)
            .finish_non_exhaustive()
    }
}

struct Context<'a> {
    api: &'a Api,
    raw: Handle,
}

impl Drop for Context<'_> {
    fn drop(&mut self) {
        unsafe { (self.api.context_destroy)(self.raw) };
    }
}

impl Context<'_> {
    fn check(&self, step: &'static str, result: i32) -> Result<(), TranslateError> {
        match result {
            SUCCESS => Ok(()),
            _ => Err(TranslateError::Rejected {
                step,
                reason: text(unsafe { (self.api.last_error)(self.raw) })
                    .unwrap_or_else(|| format!("spvc_result {result}")),
            }),
        }
    }
}

fn text(raw: *const c_char) -> Option<String> {
    match raw.is_null() {
        true => None,
        false => Some(
            unsafe { CStr::from_ptr(raw) }
                .to_string_lossy()
                .into_owned(),
        )
        .filter(|text| !text.is_empty()),
    }
}

fn signed_pow(msl: &str) -> String {
    let mut out = String::with_capacity(msl.len());
    let mut rest = msl;
    while let Some(at) = rest.find("powr(") {
        let (before, after) = rest.split_at(at);
        let standalone = !before
            .chars()
            .next_back()
            .is_some_and(|previous| previous.is_ascii_alphanumeric() || previous == '_');
        out.push_str(before);
        out.push_str(if standalone { "pow(" } else { "powr(" });
        rest = &after["powr(".len()..];
    }
    out.push_str(rest);
    out
}

fn target(slot: Slot) -> c_uint {
    match slot {
        Slot::Buffer(index) | Slot::Texture(index) | Slot::Sampler(index) => index,
    }
}

impl Translator {
    pub fn load(source: &LibrarySource) -> Result<Self, LoadError> {
        let (library, location) =
            open(source, LIBRARY).map_err(|attempts| LoadError::NotFound { attempts })?;
        let api = match Api::bind(&library) {
            Ok(api) => api,
            Err((symbol, source)) => {
                return Err(LoadError::NoEntryPoint {
                    location,
                    symbol,
                    source,
                })
            }
        };
        let translator = Self {
            api,
            location,
            _library: library,
        };
        let created = translator.context().map(drop);
        match created {
            Ok(()) => Ok(translator),
            Err(TranslateError::Context(result)) => Err(LoadError::Context {
                location: translator.location,
                result,
            }),
            Err(other) => panic!("creating a SPIRV-Cross context failed unexpectedly: {other}"),
        }
    }

    pub fn location(&self) -> &OsString {
        &self.location
    }

    fn context(&self) -> Result<Context<'_>, TranslateError> {
        let mut raw = null_mut();
        let result = unsafe { (self.api.context_create)(&mut raw) };
        match (result, raw.is_null()) {
            (SUCCESS, false) => Ok(Context {
                api: &self.api,
                raw,
            }),
            (result, _) => Err(TranslateError::Context(result)),
        }
    }

    fn compiler(&self, context: &Context<'_>, words: &[u32]) -> Result<Handle, TranslateError> {
        let api = &self.api;
        let mut ir = null_mut();
        context.check("parsing", unsafe {
            (api.parse)(context.raw, words.as_ptr(), words.len(), &mut ir)
        })?;
        let mut compiler = null_mut();
        context.check("creating the MSL compiler", unsafe {
            (api.create_compiler)(
                context.raw,
                BACKEND_MSL,
                ir,
                CAPTURE_TAKE_OWNERSHIP,
                &mut compiler,
            )
        })?;
        let model = unsafe { (api.execution_model)(compiler) };
        let slots = (0..BINDINGS).filter_map(|binding| slot(binding).map(|slot| (binding, slot)));
        for (binding, slot) in slots {
            let mut resource = ResourceBinding::default();
            unsafe { (api.binding_init)(&mut resource) };
            let index = target(slot);
            let resource = ResourceBinding {
                stage: model,
                desc_set: SET,
                binding,
                msl_buffer: index,
                msl_texture: index,
                msl_sampler: index,
                ..resource
            };
            context.check("binding a resource", unsafe {
                (api.add_binding)(compiler, &resource)
            })?;
        }
        let mut options = null_mut();
        context.check("creating options", unsafe {
            (api.create_options)(compiler, &mut options)
        })?;
        let (major, minor) = LANGUAGE;
        let settings = [
            (OPTION_MSL_VERSION, major * 10_000 + minor * 100),
            (OPTION_MSL_PLATFORM, PLATFORM_MACOS),
        ];
        for (option, value) in settings {
            context.check("setting an option", unsafe {
                (api.set_uint)(options, option, value)
            })?;
        }
        context.check("installing options", unsafe {
            (api.install_options)(compiler, options)
        })?;
        Ok(compiler)
    }

    fn emit(&self, context: &Context<'_>, compiler: Handle) -> Result<Msl, TranslateError> {
        let api = &self.api;
        let mut source = null();
        context.check("compiling to MSL", unsafe {
            (api.compile)(compiler, &mut source)
        })?;
        let source =
            text(source)
                .map(|source| signed_pow(&source))
                .ok_or(TranslateError::Rejected {
                    step: "compiling to MSL",
                    reason: "no source was returned".to_owned(),
                })?;
        let mut entries = null();
        let mut count = 0;
        context.check("listing entry points", unsafe {
            (api.entry_points)(compiler, &mut entries, &mut count)
        })?;
        if count != 1 || entries.is_null() {
            return Err(TranslateError::EntryPoints(count));
        }
        let entry = unsafe { &*entries };
        let name = unsafe { (api.cleansed_name)(compiler, entry.name, entry.execution_model) };
        let entry = text(name).ok_or(TranslateError::Rejected {
            step: "naming the entry point",
            reason: "no name was returned".to_owned(),
        })?;
        Ok(Msl { source, entry })
    }

    pub fn translate(&self, words: &[u32]) -> Result<Msl, TranslateError> {
        let context = self.context()?;
        let compiler = self.compiler(&context, words)?;
        self.emit(&context, compiler)
    }

    pub fn translate_kernel(&self, words: &[u32]) -> Result<Kernel, TranslateError> {
        let context = self.context()?;
        let compiler = self.compiler(&context, words)?;
        let model = unsafe { (self.api.execution_model)(compiler) };
        if model != MODEL_COMPUTE {
            return Err(TranslateError::Rejected {
                step: "reading the workgroup size",
                reason: format!("the entry point has execution model {model}, not compute"),
            });
        }
        let threads = [0, 1, 2]
            .map(|index| unsafe { (self.api.mode_argument)(compiler, MODE_LOCAL_SIZE, index) });
        if threads.contains(&0) {
            return Err(TranslateError::Rejected {
                step: "reading the workgroup size",
                reason: format!("the kernel declares numthreads {threads:?}"),
            });
        }
        let msl = self.emit(&context, compiler)?;
        Ok(Kernel { msl, threads })
    }
}
