use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn activation_claim_is_single_use_and_rejects_replaced_or_withdrawn_generations() {
    let active = Arc::new(Mutex::new(HashMap::from([(
        "thread".into(),
        (2, "current"),
    )])));
    assert!(!remove_current(&active, "thread", 1));
    assert!(remove_current(&active, "thread", 2));
    assert!(!remove_current(&active, "thread", 2));
    assert!(!remove_current(&active, "missing", 3));
}

#[test]
fn completed_notification_is_released_after_the_registry_lock() {
    struct Payload {
        registry: std::sync::Weak<Mutex<HashMap<String, (u64, Payload)>>>,
        released: Arc<AtomicUsize>,
    }
    impl Drop for Payload {
        fn drop(&mut self) {
            assert!(self.registry.upgrade().unwrap().try_lock().is_ok());
            self.released.fetch_add(1, Ordering::SeqCst);
        }
    }
    let active = Arc::new(Mutex::new(HashMap::new()));
    let released = Arc::new(AtomicUsize::new(0));
    active.lock().unwrap().insert(
        "thread".into(),
        (
            4,
            Payload {
                registry: Arc::downgrade(&active),
                released: released.clone(),
            },
        ),
    );
    assert!(remove_current(&active, "thread", 4));
    assert_eq!(released.load(Ordering::SeqCst), 1);
}
