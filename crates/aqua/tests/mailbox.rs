use aqua::mailbox::{Mailbox, SendError};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[test]
fn capacity_preserves_rejected_messages_and_wakes_only_pending_transitions() {
    let mut mailbox = Mailbox::new(NonZeroUsize::new(2).unwrap());
    let sender = mailbox.sender();
    sender.send(1).unwrap();
    let wakes = Arc::new(AtomicUsize::new(0));
    let count = wakes.clone();
    mailbox.set_waker(Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    sender.send(2).unwrap();
    assert_eq!(sender.send(3), Err(SendError::Full(3)));
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    assert_eq!(mailbox.drain(), vec![1, 2]);
    sender.send(3).unwrap();
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
    mailbox.close();
    assert!(mailbox.drain().is_empty());
    assert_eq!(sender.send(4), Err(SendError::Closed(4)));
}

#[test]
fn concurrent_producers_preserve_every_message_and_release_payloads_on_drop() {
    let mut mailbox = Mailbox::new(NonZeroUsize::new(800).unwrap());
    let mut workers = Vec::new();
    for producer in 0..8 {
        let sender = mailbox.sender();
        workers.push(std::thread::spawn(move || {
            for sequence in 0..100 {
                sender.send((producer, sequence)).unwrap();
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    let messages = mailbox.drain();
    assert_eq!(messages.len(), 800);
    for producer in 0..8 {
        assert_eq!(
            messages
                .iter()
                .filter(|(id, _)| *id == producer)
                .map(|(_, sequence)| *sequence)
                .collect::<Vec<_>>(),
            (0..100).collect::<Vec<_>>()
        );
    }
    let mut mailbox = Mailbox::new(NonZeroUsize::new(2).unwrap());
    let sender = mailbox.sender();
    let payload = Arc::new(());
    sender.send(payload.clone()).unwrap();
    assert_eq!(Arc::strong_count(&payload), 2);
    mailbox.close();
    assert_eq!(Arc::strong_count(&payload), 1);
    drop(mailbox);
    assert_eq!(
        sender.send(payload.clone()),
        Err(SendError::Closed(payload))
    );
}

#[test]
fn wake_can_send_again_without_holding_the_sender_lock() {
    let mailbox = Mailbox::new(NonZeroUsize::new(2).unwrap());
    let sender = mailbox.sender();
    let reentrant_sender = sender.clone();
    mailbox.set_waker(Arc::new(move || {
        reentrant_sender.send(2).unwrap();
    }));
    sender.send(1).unwrap();
    let mut mailbox = mailbox;
    assert_eq!(mailbox.drain(), vec![1, 2]);
}

#[test]
fn closing_releases_payloads_after_unlocking_the_queue() {
    struct Payload {
        sender: Option<aqua::mailbox::Sender<Payload>>,
        released: Arc<AtomicUsize>,
    }
    impl Drop for Payload {
        fn drop(&mut self) {
            self.released.fetch_add(1, Ordering::SeqCst);
            if let Some(sender) = self.sender.take() {
                assert!(matches!(
                    sender.send(Payload {
                        sender: None,
                        released: self.released.clone()
                    }),
                    Err(SendError::Closed(_))
                ));
            }
        }
    }
    let mut mailbox = Mailbox::new(NonZeroUsize::new(2).unwrap());
    let sender = mailbox.sender();
    let released = Arc::new(AtomicUsize::new(0));
    assert!(sender
        .send(Payload {
            sender: Some(sender.clone()),
            released: released.clone()
        })
        .is_ok());
    mailbox.close();
    assert_eq!(released.load(Ordering::SeqCst), 2);
}
