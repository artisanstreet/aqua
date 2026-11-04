use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;

use super::{ImageAssetBudget, ImageAssets, DECODE_STARTED};

struct Guard {
    id: u8,
    dropped: mpsc::Sender<u8>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.dropped.send(self.id).unwrap();
    }
}

#[test]
fn queued_cancel_releases_guard_while_active_cancel_waits_for_decode() {
    let (started, active) = mpsc::channel();
    let barrier = Arc::new((Mutex::new(false), Condvar::new()));
    let worker_barrier = barrier.clone();
    *DECODE_STARTED.lock().unwrap() = Some(Arc::new(move |_| {
        started.send(()).unwrap();
        let (lock, wake) = &*worker_barrier;
        let released = lock.lock().unwrap();
        drop(wake.wait_while(released, |released| !*released).unwrap());
    }));
    let assets = ImageAssets::new(ImageAssetBudget {
        workers: 1,
        ..Default::default()
    });
    let (dropped, releases) = mpsc::channel();
    let first = assets
        .decode_preview_with_guard(
            Arc::<[u8]>::from([0]),
            Guard {
                id: 1,
                dropped: dropped.clone(),
            },
        )
        .unwrap();
    active.recv_timeout(Duration::from_secs(5)).unwrap();
    let second = assets
        .decode_preview_with_guard(Arc::<[u8]>::from([0]), Guard { id: 2, dropped })
        .unwrap();
    first.cancel();
    assert!(releases.try_recv().is_err());
    second.cancel();
    assert_eq!(releases.recv_timeout(Duration::from_secs(5)).unwrap(), 2);
    assert!(releases.try_recv().is_err());
    let (lock, wake) = &*barrier;
    *lock.lock().unwrap() = true;
    wake.notify_one();
    assert_eq!(releases.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
    *DECODE_STARTED.lock().unwrap() = None;
}
