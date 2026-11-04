pub fn visible_glyph_indices(
    glyphs: impl IntoIterator<Item = (u16, bool)>,
    shaped_ids: impl IntoIterator<Item = u16>,
) -> Result<Vec<usize>, &'static str> {
    let visible: Vec<_> = glyphs
        .into_iter()
        .enumerate()
        .filter(|(_, (id, emoji))| *id != 3 || !emoji)
        .map(|(index, (id, _))| (index, id))
        .collect();
    if !visible.iter().map(|(_, id)| *id).eq(shaped_ids) {
        return Err("GPUI glyph mapping mismatch");
    }
    Ok(visible.into_iter().map(|(index, _)| index).collect())
}
