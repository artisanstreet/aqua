use aqua_core::{GpuiMesh, GpuiMeshVertex};

#[test]
fn triangle_soup_packs_normals_and_color_and_discards_incomplete_triangles() {
    let colored = GpuiMeshVertex::new(
        [1.0, 2.0, 3.0],
        [2.0, -0.5, f32::NAN],
        Some([1.0, 0.0, 0.25]),
    );
    assert_eq!(colored.normal, [32767, -16384, 0, 0]);
    assert_eq!(colored.color, [255, 0, 137, 255]);
    let plain = GpuiMeshVertex::new([0.0; 3], [0.0, 0.0, 1.0], None);
    let mesh = GpuiMesh::new(vec![colored, plain, plain, colored]);
    assert_eq!(mesh.vertices().len(), 3);
    assert_eq!(mesh.vertices()[0], colored);
    assert_eq!(mesh.vertices()[1].color, [0; 4]);
    assert_ne!(mesh.id(), GpuiMesh::new(vec![]).id());
}
