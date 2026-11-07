use std::cell::RefCell;
use std::rc::Rc;

use crate::style::LayoutStyle;

thread_local! {
    static RECENT: RefCell<Vec<Rc<LayoutStyle>>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn intern(style: LayoutStyle) -> Rc<LayoutStyle> {
    RECENT.with_borrow_mut(|recent| {
        if let Some(value) = recent.last().filter(|value| value.as_ref() == &style) {
            return Rc::clone(value);
        }
        match recent.iter().rposition(|value| value.as_ref() == &style) {
            Some(index) => {
                let value = recent.remove(index);
                recent.push(Rc::clone(&value));
                value
            }
            None => {
                let value = Rc::new(style);
                if recent.len() == 32 {
                    recent.remove(0);
                }
                recent.push(Rc::clone(&value));
                value
            }
        }
    })
}
