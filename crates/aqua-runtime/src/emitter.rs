use aqua_protocol::SceneDiff;
use aqua_scene::Scene;

pub fn emit_scene_diff(scene: &mut Scene) -> SceneDiff {
    let (upserts, fills, srgba_fills) = scene.drain_updates();
    scene.drain_removed();
    let order = scene
        .take_structure_changed()
        .then(|| scene.ordered().map(|(id, _)| id).collect());
    SceneDiff {
        upserts,
        fills,
        srgba_fills,
        order,
    }
}
