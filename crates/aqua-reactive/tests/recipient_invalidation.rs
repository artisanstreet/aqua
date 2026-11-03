use std::cell::Cell;
use std::rc::Rc;

use aqua_reactive::Runtime;

#[test]
fn mounted_recipients_are_invalidated_on_each_write_until_disposal() {
    let runtime = Runtime::new();
    runtime.set_auto_flush(false);
    let owner = runtime.create_scope();
    let (first, write_first) = owner.signal(0);
    let (second, write_second) = owner.signal(0);
    let (unrelated, write_unrelated) = owner.signal(0);
    let first_epoch = Rc::new(Cell::new(0));
    let second_epoch = Rc::new(Cell::new(0));
    let first_scope = owner.child();
    first_scope.set_invalidation_counter(first_epoch.clone());
    let mounted_child = first_scope.child();
    mounted_child.subscribe(&first, |_| {});
    let second_scope = owner.child();
    second_scope.set_invalidation_counter(second_epoch.clone());
    second_scope.subscribe(&second, |_| {});
    owner.subscribe(&unrelated, |_| {});
    runtime.flush();
    write_first.set(1);
    assert_eq!(first_epoch.get(), 1);
    assert_eq!(second_epoch.get(), 0);
    write_unrelated.set(1);
    assert_eq!(first_epoch.get(), 1);
    write_first.set(2);
    assert_eq!(first_epoch.get(), 2);
    write_second.set(1);
    assert_eq!(second_epoch.get(), 1);
    mounted_child.dispose();
    write_first.set(3);
    assert_eq!(first_epoch.get(), 2);
}
