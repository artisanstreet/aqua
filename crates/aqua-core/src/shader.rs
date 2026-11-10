use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub struct ReferenceShader(pub fn(&[f32], [f32; 2]) -> [f32; 4]);

impl PartialEq for ReferenceShader {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::fn_addr_eq(self.0, other.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FragmentShader {
    pub name: Arc<str>,
    pub hlsl: Arc<str>,
    pub entry: Arc<str>,
    pub reference: Option<ReferenceShader>,
}
