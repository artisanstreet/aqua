use super::Element;

pub(super) fn extend<I, E>(children: &mut Vec<Element>, values: I)
where
    I: IntoIterator<Item = E>,
    E: Into<Element>,
{
    let mut values = values.into_iter().map(Into::into);
    let (lower, upper) = values.size_hint();
    if upper == Some(lower) {
        children.extend(values);
        return;
    }
    while let Some(value) = values.next() {
        if children.len() == children.capacity() {
            let ceiling = children.capacity().saturating_mul(2);
            let reservation = values
                .size_hint()
                .1
                .and_then(|remaining| remaining.checked_add(1))
                .filter(|remaining| {
                    children
                        .len()
                        .checked_add(*remaining)
                        .is_some_and(|total| total <= ceiling)
                });
            if let Some(remaining) = reservation {
                children.reserve_exact(remaining);
            }
        }
        children.push(value);
    }
}
