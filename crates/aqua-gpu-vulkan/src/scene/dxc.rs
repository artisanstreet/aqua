use std::ffi::{c_int, c_ulong, c_void, OsString};
use std::fmt;
use std::path::PathBuf;
use std::ptr::{null_mut, NonNull};
use std::sync::Arc;

use libloading::os::unix::{Library, RTLD_LOCAL, RTLD_NOW};

pub const LIBRARY: &str = "libdxcompiler.so";
const RTLD_NODELETE: c_int = 0x1000;
const UTF8: u32 = 65001;

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

const CLSID_DXC_COMPILER: Guid = Guid {
    data1: 0x73e2_2d93,
    data2: 0xe6ce,
    data3: 0x47f3,
    data4: [0xb5, 0xbf, 0xf0, 0x66, 0x4f, 0x39, 0xc1, 0xb0],
};

const IID_DXC_COMPILER3: Guid = Guid {
    data1: 0x228b_4687,
    data2: 0x5a6a,
    data3: 0x4730,
    data4: [0x90, 0x0c, 0x97, 0x02, 0xb2, 0x20, 0x3f, 0x54],
};

const IID_DXC_RESULT: Guid = Guid {
    data1: 0x5834_6cda,
    data2: 0xdde7,
    data3: 0x4497,
    data4: [0x94, 0x61, 0x6f, 0x87, 0xaf, 0x5e, 0x06, 0x59],
};

#[repr(C)]
struct DxcBuffer {
    ptr: *const c_void,
    size: usize,
    encoding: u32,
}

type CreateInstance = unsafe extern "C" fn(*const Guid, *const Guid, *mut *mut c_void) -> i32;

#[repr(C)]
struct UnknownTable {
    query_interface: *const c_void,
    add_ref: *const c_void,
    release: unsafe extern "C" fn(*mut c_void) -> c_ulong,
}

#[repr(C)]
struct CompilerTable {
    unknown: UnknownTable,
    compile: unsafe extern "C" fn(
        *mut c_void,
        *const DxcBuffer,
        *const *const u32,
        u32,
        *mut c_void,
        *const Guid,
        *mut *mut c_void,
    ) -> i32,
}

#[repr(C)]
struct ResultTable {
    unknown: UnknownTable,
    get_status: unsafe extern "C" fn(*mut c_void, *mut i32) -> i32,
    get_result: unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> i32,
    get_error_buffer: unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> i32,
}

#[repr(C)]
struct BlobTable {
    unknown: UnknownTable,
    get_buffer_pointer: unsafe extern "C" fn(*mut c_void) -> *const u8,
    get_buffer_size: unsafe extern "C" fn(*mut c_void) -> usize,
}

#[repr(C)]
struct Object<T> {
    table: *const T,
}

struct Com<T>(NonNull<Object<T>>);

impl<T> Com<T> {
    unsafe fn adopt(raw: *mut c_void) -> Option<Self> {
        NonNull::new(raw.cast()).map(Self)
    }

    fn this(&self) -> *mut c_void {
        self.0.as_ptr().cast()
    }

    fn table(&self) -> &T {
        unsafe { &*self.0.as_ref().table }
    }
}

impl<T> Drop for Com<T> {
    fn drop(&mut self) {
        let unknown = unsafe { &*self.0.as_ref().table.cast::<UnknownTable>() };
        unsafe { (unknown.release)(self.this()) };
    }
}

impl Com<BlobTable> {
    fn bytes(&self) -> Vec<u8> {
        let table = self.table();
        let size = unsafe { (table.get_buffer_size)(self.this()) };
        let start = unsafe { (table.get_buffer_pointer)(self.this()) };
        match (start.is_null(), size) {
            (true, _) | (_, 0) => Vec::new(),
            (false, size) => unsafe { std::slice::from_raw_parts(start, size) }.to_vec(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Pixel,
    Compute,
}

impl Stage {
    fn profile(self) -> &'static str {
        match self {
            Stage::Vertex => "vs_6_0",
            Stage::Pixel => "ps_6_0",
            Stage::Compute => "cs_6_0",
        }
    }
}

const SPIRV_ARGUMENTS: &[&str] = &[
    "-HV",
    "2018",
    "-O3",
    "-spirv",
    "-fspv-target-env=vulkan1.1",
    "-fvk-use-dx-layout",
    "-fspv-use-unknown-image-format",
    "-fvk-b-shift",
    "0",
    "0",
    "-fvk-t-shift",
    "16",
    "0",
    "-fvk-s-shift",
    "32",
    "0",
    "-fvk-u-shift",
    "48",
    "0",
];

fn wide(argument: &str) -> Vec<u32> {
    argument.chars().map(u32::from).chain([0]).collect()
}

fn arguments(entry: &str, stage: Stage) -> Vec<Vec<u32>> {
    ["aqua-custom", "-E", entry, "-T", stage.profile()]
        .into_iter()
        .chain(SPIRV_ARGUMENTS.iter().copied())
        .map(wide)
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ShaderCompiler {
    #[default]
    Search,
    Library(PathBuf),
    Disabled,
}

impl ShaderCompiler {
    pub fn from_env() -> Self {
        std::env::var_os("AQUA_DXCOMPILER")
            .filter(|path| !path.is_empty())
            .map_or(ShaderCompiler::Search, |path| {
                ShaderCompiler::Library(path.into())
            })
    }

    fn candidates(&self) -> Vec<OsString> {
        match self {
            ShaderCompiler::Disabled => Vec::new(),
            ShaderCompiler::Library(path) => vec![path.clone().into_os_string()],
            ShaderCompiler::Search => {
                let beside = std::env::current_exe()
                    .ok()
                    .and_then(|exe| exe.parent().map(PathBuf::from))
                    .map(|dir| {
                        vec![
                            dir.join(LIBRARY).into_os_string(),
                            dir.join("../lib").join(LIBRARY).into_os_string(),
                        ]
                    })
                    .unwrap_or_default();
                beside.into_iter().chain([LIBRARY.into()]).collect()
            }
        }
    }
}

#[derive(Debug)]
pub struct Attempt {
    pub location: OsString,
    pub reason: String,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("runtime shader compilation is disabled")]
    Disabled,
    #[error("{LIBRARY} could not be loaded: {}", Attempts(.attempts))]
    NotFound { attempts: Vec<Attempt> },
    #[error("{} exports no DxcCreateInstance: {source}", .location.to_string_lossy())]
    NoEntryPoint {
        location: OsString,
        source: libloading::Error,
    },
    #[error("{} could not create a compiler: HRESULT {hresult:#010x}", .location.to_string_lossy())]
    Instance { location: OsString, hresult: i32 },
}

struct Attempts<'a>(&'a [Attempt]);

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

#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    #[error("{0}")]
    Rejected(String),
    #[error("the entry point name contains NUL")]
    EntryContainsNul,
    #[error("DxcCreateInstance failed with HRESULT {0:#010x}")]
    Instance(i32),
    #[error("IDxcCompiler3::Compile failed with HRESULT {0:#010x}")]
    Call(i32),
    #[error("the compiler returned no SPIR-V")]
    Empty,
    #[error("the compiler returned {0} bytes, which is not whole SPIR-V words")]
    Misaligned(usize),
}

pub struct Compiler {
    create: CreateInstance,
    location: OsString,
    _library: Library,
}

impl fmt::Debug for Compiler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Compiler")
            .field("location", &self.location)
            .finish_non_exhaustive()
    }
}

impl Compiler {
    pub fn load(choice: &ShaderCompiler) -> Result<Self, LoadError> {
        if *choice == ShaderCompiler::Disabled {
            return Err(LoadError::Disabled);
        }
        let mut attempts = Vec::new();
        for location in choice.candidates() {
            let flags = RTLD_NOW | RTLD_LOCAL | RTLD_NODELETE;
            match unsafe { Library::open(Some(&location), flags) } {
                Ok(library) => return Self::bind(library, location),
                Err(error) => attempts.push(Attempt {
                    location,
                    reason: error.to_string(),
                }),
            }
        }
        Err(LoadError::NotFound { attempts })
    }

    fn bind(library: Library, location: OsString) -> Result<Self, LoadError> {
        let create = match unsafe { library.get::<CreateInstance>(b"DxcCreateInstance\0") } {
            Ok(symbol) => *symbol,
            Err(source) => return Err(LoadError::NoEntryPoint { location, source }),
        };
        let compiler = Self {
            create,
            location,
            _library: library,
        };
        match compiler.instance() {
            Ok(_) => Ok(compiler),
            Err(CompileError::Instance(hresult)) => Err(LoadError::Instance {
                location: compiler.location,
                hresult,
            }),
            Err(other) => panic!("creating a DXC compiler failed unexpectedly: {other}"),
        }
    }

    pub fn location(&self) -> &OsString {
        &self.location
    }

    fn instance(&self) -> Result<Com<CompilerTable>, CompileError> {
        let mut raw = null_mut();
        let hresult = unsafe { (self.create)(&CLSID_DXC_COMPILER, &IID_DXC_COMPILER3, &mut raw) };
        match (hresult, unsafe { Com::adopt(raw) }) {
            (0.., Some(compiler)) => Ok(compiler),
            (hresult, _) => Err(CompileError::Instance(hresult)),
        }
    }

    pub fn compile(
        &self,
        source: &str,
        entry: &str,
        stage: Stage,
    ) -> Result<Vec<u32>, CompileError> {
        if entry.contains('\0') {
            return Err(CompileError::EntryContainsNul);
        }
        let compiler = self.instance()?;
        let arguments = arguments(entry, stage);
        let pointers: Vec<*const u32> =
            arguments.iter().map(|argument| argument.as_ptr()).collect();
        let buffer = DxcBuffer {
            ptr: source.as_ptr().cast(),
            size: source.len(),
            encoding: UTF8,
        };
        let mut raw = null_mut();
        let hresult = unsafe {
            (compiler.table().compile)(
                compiler.this(),
                &buffer,
                pointers.as_ptr(),
                pointers.len() as u32,
                null_mut(),
                &IID_DXC_RESULT,
                &mut raw,
            )
        };
        let result = match (hresult, unsafe { Com::<ResultTable>::adopt(raw) }) {
            (0.., Some(result)) => result,
            (0.., None) => return Err(CompileError::Empty),
            (hresult, _) => return Err(CompileError::Call(hresult)),
        };
        let table = result.table();
        let mut status = 0;
        let queried = unsafe { (table.get_status)(result.this(), &mut status) };
        if queried < 0 {
            return Err(CompileError::Call(queried));
        }
        if status < 0 {
            let mut errors = null_mut();
            unsafe { (table.get_error_buffer)(result.this(), &mut errors) };
            let text = unsafe { Com::<BlobTable>::adopt(errors) }
                .map(|blob| {
                    String::from_utf8_lossy(&blob.bytes())
                        .trim_end_matches(['\0', '\n', ' '])
                        .to_owned()
                })
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| format!("compilation failed with HRESULT {status:#010x}"));
            return Err(CompileError::Rejected(text));
        }
        let mut object = null_mut();
        let fetched = unsafe { (table.get_result)(result.this(), &mut object) };
        let bytes = match (fetched, unsafe { Com::<BlobTable>::adopt(object) }) {
            (0.., Some(blob)) => blob.bytes(),
            (0.., None) => return Err(CompileError::Empty),
            (hresult, _) => return Err(CompileError::Call(hresult)),
        };
        match (bytes.len(), bytes.len() % 4) {
            (0, _) => Err(CompileError::Empty),
            (_, 0) => Ok(bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|word| u32::from_le_bytes(*word))
                .collect()),
            (length, _) => Err(CompileError::Misaligned(length)),
        }
    }
}

#[derive(Clone, Debug)]
pub enum RuntimeCompiler {
    Ready(Arc<Compiler>),
    Unavailable(Arc<LoadError>),
}

impl RuntimeCompiler {
    pub fn load(choice: &ShaderCompiler) -> Self {
        match Compiler::load(choice) {
            Ok(compiler) => RuntimeCompiler::Ready(Arc::new(compiler)),
            Err(LoadError::Disabled) => RuntimeCompiler::Unavailable(Arc::new(LoadError::Disabled)),
            Err(error) => {
                eprintln!("aqua: custom shaders are unavailable on Vulkan: {error}; set AQUA_DXCOMPILER to a {LIBRARY} from DXC 1.7.2308 or newer");
                RuntimeCompiler::Unavailable(Arc::new(error))
            }
        }
    }

    pub fn ready(&self) -> Option<&Compiler> {
        match self {
            RuntimeCompiler::Ready(compiler) => Some(compiler),
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
