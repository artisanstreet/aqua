use std::panic::{catch_unwind, AssertUnwindSafe};

use aqua_reactive::{read_check, Runtime};

#[test]
fn declared_reads_pass() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, _) = scope.signal(1);
    let (b, _) = scope.signal(2);
    let deps = (a.clone(), b.clone());
    let sum = read_check::render_scope(&deps, || a.get() + b.get());
    assert_eq!(sum, 3);
}

#[test]
fn undeclared_read_names_creation_site() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (declared, _) = scope.signal(1);
    let (forgotten, _) = scope.signal(2);
    let created_line = line!() - 1;
    let deps = (declared.clone(),);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        read_check::render_scope(&deps, || forgotten.get())
    }));
    let payload = outcome.expect_err("undeclared read must panic");
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(message.contains("read_check.rs"), "{message}");
    assert!(message.contains(&format!("{created_line}:")), "{message}");
    assert!(!read_check::in_render());
}

#[test]
fn derived_reads_are_checked_like_sources() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, _) = scope.signal(1);
    let doubled = scope.derive1(&a, |v| v * 2);
    let deps = (a.clone(),);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        read_check::render_scope(&deps, || doubled.get())
    }));
    assert!(outcome.is_err());
}

#[test]
fn reads_outside_render_and_in_unchecked_scopes_are_free() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, _) = scope.signal(1);
    let (b, _) = scope.signal(2);
    assert_eq!(b.get(), 2);
    let deps = (a.clone(),);
    let value = read_check::render_scope(&deps, || {
        read_check::unchecked(|| {
            assert!(!read_check::in_render());
            b.get()
        })
    });
    assert_eq!(value, 2);
}

#[test]
fn scopes_nest_and_restore() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, _) = scope.signal(1);
    let (b, _) = scope.signal(2);
    let outer = (a.clone(),);
    let inner = (b.clone(),);
    read_check::render_scope(&outer, || {
        read_check::render_scope(&inner, || {
            b.get();
        });
        assert!(catch_unwind(AssertUnwindSafe(|| b.get())).is_err());
        a.get();
    });
}
