use std::path::PathBuf;

use objc2_core_foundation::{
    CFArray, CFDictionary, CFLocale, CFRange, CFRetained, CFString, CFType, CFURL,
};
use objc2_core_text::{
    kCTFontFamilyNameAttribute, kCTFontURLAttribute, CTFont, CTFontDescriptor,
    CTFontManagerCopyAvailableFontURLs, CTFontUIFontType,
};

use crate::discovery::{is_font_file, FaceRef};
use crate::{Generic, Scope};

#[derive(Debug, thiserror::Error)]
pub(crate) enum CoreTextError {
    #[error("CTFontManagerCopyAvailableFontURLs listed no font files")]
    NoFontFiles,
    #[error("CTFontCreateUIFontForLanguage has no {0} font")]
    NoInterfaceFont(&'static str),
}

const LAST_RESORT: &str = "LastResort";

const SCRIPT_SAMPLES: [char; 31] = [
    'あ', 'ア', '中', '한', 'ع', 'ے', 'א', 'क', 'ক', 'ਕ', 'ક', 'କ', 'த', 'క', 'ಕ', 'ക', 'ක', 'ก',
    'ກ', 'ཀ', 'က', 'ក', 'ሀ', 'ᐁ', 'Ա', 'ა', '★', '→', '∑', '⌘', '♪',
];

pub(crate) struct Snapshot {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) sorted: Vec<(Generic, Vec<FaceRef>)>,
}

enum Lead {
    Interface(CTFontUIFontType, &'static str),
    Families(&'static [&'static str]),
}

fn lead(generic: Generic) -> Lead {
    match generic {
        Generic::SansSerif => Lead::Interface(CTFontUIFontType::System, "system"),
        Generic::Monospace => Lead::Interface(CTFontUIFontType::UserFixedPitch, "user fixed-pitch"),
        Generic::Serif => Lead::Families(&["Times", "New York"]),
        Generic::Cursive => Lead::Families(&["Apple Chancery"]),
        Generic::Fantasy => Lead::Families(&["Papyrus"]),
        Generic::Emoji => Lead::Families(&["Apple Color Emoji"]),
    }
}

fn elements(array: &CFArray) -> impl Iterator<Item = CFRetained<CFType>> + '_ {
    let array: &CFArray<CFType> = unsafe { array.cast_unchecked() };
    array.iter()
}

pub(crate) fn interface_font(kind: CTFontUIFontType) -> Option<CFRetained<CTFont>> {
    unsafe { CTFont::new_ui_font_for_language(kind, 0.0, None) }
}

pub(crate) fn family_font(family: &str) -> Option<CFRetained<CTFont>> {
    let name = CFString::from_str(family);
    let key = unsafe { kCTFontFamilyNameAttribute };
    let attributes = CFDictionary::<CFString, CFString>::from_slices(&[key], &[&name]);
    let descriptor = unsafe { CTFontDescriptor::with_attributes(attributes.as_ref()) };
    let font = unsafe { CTFont::with_font_descriptor(&descriptor, 0.0, std::ptr::null()) };
    let found = unsafe { font.family_name() }.to_string();
    found.eq_ignore_ascii_case(family).then_some(font)
}

pub(crate) fn face_of(font: &CTFont) -> Option<FaceRef> {
    let url = unsafe { font.attribute(kCTFontURLAttribute) }?
        .downcast::<CFURL>()
        .ok()?;
    Some(FaceRef::Named {
        path: url.to_file_path()?,
        post_script_name: unsafe { font.post_script_name() }.to_string(),
    })
}

fn chosen_for_samples(lead: &CTFont) -> Vec<FaceRef> {
    SCRIPT_SAMPLES
        .iter()
        .filter_map(|sample| {
            let text = CFString::from_str(sample.encode_utf8(&mut [0; 4]));
            let range = CFRange {
                location: 0,
                length: sample.len_utf16() as isize,
            };
            let chosen = unsafe { lead.for_string(&text, range) };
            face_of(&chosen)
        })
        .collect()
}

fn cascade(lead: &CTFont) -> Vec<FaceRef> {
    let languages = CFLocale::preferred_languages();
    let Some(list) = (unsafe { lead.default_cascade_list_for_languages(languages.as_deref()) })
    else {
        return Vec::new();
    };
    let mut faces = Vec::new();
    let mut composites_expanded = false;
    for descriptor in
        elements(&list).filter_map(|element| element.downcast::<CTFontDescriptor>().ok())
    {
        let font = unsafe { CTFont::with_font_descriptor(&descriptor, 0.0, std::ptr::null()) };
        match (face_of(&font), composites_expanded) {
            (Some(face), _) => faces.push(face),
            (None, false) => {
                faces.extend(chosen_for_samples(lead));
                composites_expanded = true;
            }
            (None, true) => {}
        }
    }
    faces.retain(|face| {
        !matches!(face, FaceRef::Named { post_script_name, .. } if post_script_name == LAST_RESORT)
    });
    faces
}

fn sorted(generic: Generic) -> Result<Vec<FaceRef>, CoreTextError> {
    let leads: Vec<CFRetained<CTFont>> = match lead(generic) {
        Lead::Interface(kind, name) => {
            vec![interface_font(kind).ok_or(CoreTextError::NoInterfaceFont(name))?]
        }
        Lead::Families(families) => families
            .iter()
            .filter_map(|family| family_font(family))
            .collect(),
    };
    let fallbacks = match (generic.fallback_scope(), leads.first()) {
        (Scope::Everything, Some(first)) => cascade(first),
        (Scope::Everything, None) | (Scope::Preferred, _) => Vec::new(),
    };
    let mut faces: Vec<FaceRef> = Vec::new();
    for face in leads
        .iter()
        .filter_map(|font| face_of(font))
        .chain(fallbacks)
    {
        if !faces.contains(&face) {
            faces.push(face);
        }
    }
    Ok(faces)
}

fn available_files() -> Vec<PathBuf> {
    let urls = unsafe { CTFontManagerCopyAvailableFontURLs() };
    elements(&urls)
        .filter_map(|element| element.downcast::<CFURL>().ok())
        .filter_map(|url| url.to_file_path())
        .filter(|path| is_font_file(path))
        .collect()
}

pub(crate) fn snapshot() -> Result<Snapshot, CoreTextError> {
    let mut files = available_files();
    if files.is_empty() {
        return Err(CoreTextError::NoFontFiles);
    }
    let sorted = Generic::ALL
        .into_iter()
        .map(|generic| sorted(generic).map(|faces| (generic, faces)))
        .collect::<Result<Vec<_>, _>>()?;
    files.extend(
        sorted
            .iter()
            .flat_map(|(_, faces)| faces.iter().map(|face| face.path().to_path_buf())),
    );
    files.sort();
    files.dedup();
    Ok(Snapshot { files, sorted })
}
