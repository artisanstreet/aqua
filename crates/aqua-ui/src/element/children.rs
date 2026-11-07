use super::Element;

pub(super) fn extend<I, E>(children: &mut Vec<Element>, values: I)
where
    I: IntoIterator<Item = E>,
    E: Into<Element>,
{
    children.extend(values.into_iter().map(Into::into));
}
