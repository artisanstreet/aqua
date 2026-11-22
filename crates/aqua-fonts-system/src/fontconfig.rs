use std::ffi::{c_char, c_int, c_void, CStr, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use libloading::os::unix::{Library, RTLD_LOCAL, RTLD_NOW};

use crate::discovery::FaceRef;
use crate::{Generic, Scope};

const LIBRARY: &str = "libfontconfig.so.1";
const RTLD_NODELETE: c_int = 0x1000;
const MATCH: c_int = 0;
const MATCH_PATTERN: c_int = 0;
const FILE: &CStr = c"file";
const INDEX: &CStr = c"index";
const FAMILY: &CStr = c"family";

#[derive(Debug, thiserror::Error)]
pub(crate) enum FontconfigError {
    #[error("{LIBRARY} could not be loaded: {0}")]
    Library(#[source] libloading::Error),
    #[error("{LIBRARY} has no {symbol}: {source}")]
    Symbol {
        symbol: &'static str,
        #[source]
        source: libloading::Error,
    },
    #[error("FcInitLoadConfigAndFonts returned no configuration")]
    Configuration,
    #[error("{call} returned nothing")]
    Empty { call: &'static str },
}

pub(crate) struct Snapshot {
    pub(crate) directories: Vec<PathBuf>,
    pub(crate) files: Vec<PathBuf>,
    pub(crate) sorted: Vec<(Generic, Vec<FaceRef>)>,
}

#[repr(C)]
struct FontSet {
    count: c_int,
    capacity: c_int,
    fonts: *mut *mut c_void,
}

type Handle = *mut c_void;

struct Api {
    init: unsafe extern "C" fn() -> Handle,
    destroy_config: unsafe extern "C" fn(Handle),
    font_dirs: unsafe extern "C" fn(Handle) -> Handle,
    next_string: unsafe extern "C" fn(Handle) -> *const c_char,
    strings_done: unsafe extern "C" fn(Handle),
    pattern_create: unsafe extern "C" fn() -> Handle,
    pattern_destroy: unsafe extern "C" fn(Handle),
    add_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    get_string: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut *const c_char) -> c_int,
    get_integer: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_int) -> c_int,
    objects_create: unsafe extern "C" fn() -> Handle,
    objects_add: unsafe extern "C" fn(Handle, *const c_char) -> c_int,
    objects_destroy: unsafe extern "C" fn(Handle),
    list: unsafe extern "C" fn(Handle, Handle, Handle) -> *mut FontSet,
    sort: unsafe extern "C" fn(Handle, Handle, c_int, *mut Handle, *mut c_int) -> *mut FontSet,
    set_destroy: unsafe extern "C" fn(*mut FontSet),
    substitute: unsafe extern "C" fn(Handle, Handle, c_int) -> c_int,
    default_substitute: unsafe extern "C" fn(Handle),
    _library: Library,
}

fn symbol<T: Copy>(library: &Library, name: &'static str) -> Result<T, FontconfigError> {
    let mut terminated = name.as_bytes().to_vec();
    terminated.push(0);
    unsafe { library.get::<T>(&terminated) }
        .map(|found| *found)
        .map_err(|source| FontconfigError::Symbol {
            symbol: name,
            source,
        })
}

impl Api {
    fn load() -> Result<Api, FontconfigError> {
        let library =
            unsafe { Library::open(Some(LIBRARY), RTLD_NOW | RTLD_LOCAL | RTLD_NODELETE) }
                .map_err(FontconfigError::Library)?;
        Ok(Api {
            init: symbol(&library, "FcInitLoadConfigAndFonts")?,
            destroy_config: symbol(&library, "FcConfigDestroy")?,
            font_dirs: symbol(&library, "FcConfigGetFontDirs")?,
            next_string: symbol(&library, "FcStrListNext")?,
            strings_done: symbol(&library, "FcStrListDone")?,
            pattern_create: symbol(&library, "FcPatternCreate")?,
            pattern_destroy: symbol(&library, "FcPatternDestroy")?,
            add_string: symbol(&library, "FcPatternAddString")?,
            get_string: symbol(&library, "FcPatternGetString")?,
            get_integer: symbol(&library, "FcPatternGetInteger")?,
            objects_create: symbol(&library, "FcObjectSetCreate")?,
            objects_add: symbol(&library, "FcObjectSetAdd")?,
            objects_destroy: symbol(&library, "FcObjectSetDestroy")?,
            list: symbol(&library, "FcFontList")?,
            sort: symbol(&library, "FcFontSort")?,
            set_destroy: symbol(&library, "FcFontSetDestroy")?,
            substitute: symbol(&library, "FcConfigSubstitute")?,
            default_substitute: symbol(&library, "FcDefaultSubstitute")?,
            _library: library,
        })
    }
}

struct Config<'a> {
    api: &'a Api,
    handle: Handle,
}

impl Drop for Config<'_> {
    fn drop(&mut self) {
        unsafe { (self.api.destroy_config)(self.handle) }
    }
}

struct Pattern<'a> {
    api: &'a Api,
    handle: Handle,
}

impl Drop for Pattern<'_> {
    fn drop(&mut self) {
        unsafe { (self.api.pattern_destroy)(self.handle) }
    }
}

struct Objects<'a> {
    api: &'a Api,
    handle: Handle,
}

impl Drop for Objects<'_> {
    fn drop(&mut self) {
        unsafe { (self.api.objects_destroy)(self.handle) }
    }
}

struct Set<'a> {
    api: &'a Api,
    set: *mut FontSet,
}

impl Set<'_> {
    fn patterns(&self) -> &[Handle] {
        let set = unsafe { &*self.set };
        match (set.fonts.is_null(), usize::try_from(set.count)) {
            (false, Ok(count)) => unsafe { std::slice::from_raw_parts(set.fonts, count) },
            _ => &[],
        }
    }
}

impl Drop for Set<'_> {
    fn drop(&mut self) {
        unsafe { (self.api.set_destroy)(self.set) }
    }
}

fn path_of(text: *const c_char) -> PathBuf {
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    PathBuf::from(OsStr::from_bytes(bytes))
}

impl Config<'_> {
    fn directories(&self) -> Vec<PathBuf> {
        let list = unsafe { (self.api.font_dirs)(self.handle) };
        if list.is_null() {
            return Vec::new();
        }
        let mut found = Vec::new();
        loop {
            let next = unsafe { (self.api.next_string)(list) };
            if next.is_null() {
                break;
            }
            found.push(path_of(next));
        }
        unsafe { (self.api.strings_done)(list) };
        found
    }

    fn pattern(&self) -> Result<Pattern<'_>, FontconfigError> {
        let handle = unsafe { (self.api.pattern_create)() };
        match handle.is_null() {
            true => Err(FontconfigError::Empty {
                call: "FcPatternCreate",
            }),
            false => Ok(Pattern {
                api: self.api,
                handle,
            }),
        }
    }

    fn set(&self, set: *mut FontSet, call: &'static str) -> Result<Set<'_>, FontconfigError> {
        match set.is_null() {
            true => Err(FontconfigError::Empty { call }),
            false => Ok(Set { api: self.api, set }),
        }
    }

    fn face(&self, pattern: Handle) -> Option<FaceRef> {
        let mut file: *const c_char = std::ptr::null();
        let found = unsafe { (self.api.get_string)(pattern, FILE.as_ptr(), 0, &mut file) };
        if found != MATCH || file.is_null() {
            return None;
        }
        let mut index: c_int = 0;
        let indexed = unsafe { (self.api.get_integer)(pattern, INDEX.as_ptr(), 0, &mut index) };
        let index = match indexed {
            MATCH => u32::try_from(index).ok()? & 0xffff,
            _ => 0,
        };
        Some(FaceRef {
            path: path_of(file),
            index,
        })
    }

    fn files(&self) -> Result<Vec<PathBuf>, FontconfigError> {
        let pattern = self.pattern()?;
        let objects = Objects {
            api: self.api,
            handle: unsafe { (self.api.objects_create)() },
        };
        if objects.handle.is_null() {
            return Err(FontconfigError::Empty {
                call: "FcObjectSetCreate",
            });
        }
        for object in [FILE, INDEX] {
            unsafe { (self.api.objects_add)(objects.handle, object.as_ptr()) };
        }
        let listed = unsafe { (self.api.list)(self.handle, pattern.handle, objects.handle) };
        let set = self.set(listed, "FcFontList")?;
        let mut files: Vec<PathBuf> = set
            .patterns()
            .iter()
            .filter_map(|pattern| self.face(*pattern))
            .map(|face| face.path)
            .collect();
        files.sort();
        files.dedup();
        Ok(files)
    }

    fn strings(&self, pattern: Handle, object: &CStr) -> Vec<String> {
        let mut found = Vec::new();
        loop {
            let mut value: *const c_char = std::ptr::null();
            let index = c_int::try_from(found.len()).unwrap_or(c_int::MAX);
            let result =
                unsafe { (self.api.get_string)(pattern, object.as_ptr(), index, &mut value) };
            if result != MATCH || value.is_null() {
                return found;
            }
            found.push(
                unsafe { CStr::from_ptr(value) }
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }

    fn sorted(&self, generic: Generic) -> Result<Vec<FaceRef>, FontconfigError> {
        let pattern = self.pattern()?;
        let name = generic.fontconfig_name();
        unsafe {
            (self.api.add_string)(pattern.handle, FAMILY.as_ptr(), name.as_ptr());
            (self.api.substitute)(self.handle, pattern.handle, MATCH_PATTERN);
            (self.api.default_substitute)(pattern.handle);
        }
        let generic_name = name.to_string_lossy();
        let preferred: Vec<String> = self
            .strings(pattern.handle, FAMILY)
            .into_iter()
            .take_while(|family| !family.eq_ignore_ascii_case(&generic_name))
            .collect();
        let mut result: c_int = 0;
        let sorted = unsafe {
            (self.api.sort)(
                self.handle,
                pattern.handle,
                1,
                std::ptr::null_mut(),
                &mut result,
            )
        };
        let set = self.set(sorted, "FcFontSort")?;
        let scoped = |face: &Handle| match generic.fallback_scope() {
            Scope::Everything => true,
            Scope::Preferred => self.strings(*face, FAMILY).iter().any(|family| {
                preferred
                    .iter()
                    .any(|wanted| wanted.eq_ignore_ascii_case(family))
            }),
        };
        Ok(set
            .patterns()
            .iter()
            .filter(|face| scoped(face))
            .filter_map(|face| self.face(*face))
            .collect())
    }
}

pub(crate) fn snapshot() -> Result<Snapshot, FontconfigError> {
    let api = Api::load()?;
    let handle = unsafe { (api.init)() };
    if handle.is_null() {
        return Err(FontconfigError::Configuration);
    }
    let config = Config { api: &api, handle };
    let sorted = Generic::ALL
        .into_iter()
        .map(|generic| config.sorted(generic).map(|faces| (generic, faces)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Snapshot {
        directories: config.directories(),
        files: config.files()?,
        sorted,
    })
}
