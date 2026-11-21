use std::ffi::c_void;

#[allow(clippy::missing_safety_doc)]
pub unsafe trait AccessHooks {
    fn children(&mut self) -> *mut c_void;
    fn focused(&mut self) -> *mut c_void;
    fn hit_test(&mut self, screen_point: (f64, f64)) -> *mut c_void;
}
