use std::thread;
use std::time::Duration;

use aqua_transport::{channel, RecvError, SendError};

#[test]
fn preserves_order_across_threads_under_backpressure() {
    let (mut tx, mut rx) = channel::<u64>(8);
    let producer = thread::spawn(move || {
        for value in 0..200_000u64 {
            tx.send(value).unwrap();
        }
    });
    let mut expected = 0;
    loop {
        match rx.recv_timeout(Some(Duration::from_secs(5))) {
            Ok(value) => {
                assert_eq!(value, expected);
                expected += 1;
            }
            Err(RecvError::Disconnected) => break,
            Err(other) => panic!("{other:?}"),
        }
    }
    producer.join().unwrap();
    assert_eq!(expected, 200_000);
}

#[test]
fn full_ring_rejects_without_losing_the_value() {
    let (mut tx, mut rx) = channel::<u32>(2);
    tx.try_send(1).unwrap();
    tx.try_send(2).unwrap();
    assert_eq!(tx.try_send(3), Err(SendError::Full(3)));
    assert_eq!(rx.try_recv(), Ok(1));
    tx.try_send(3).unwrap();
    assert_eq!(rx.drain(10), vec![2, 3]);
    assert_eq!(rx.try_recv(), Err(RecvError::Empty));
}

#[test]
fn dropping_the_sender_still_delivers_everything_queued() {
    let (mut tx, mut rx) = channel::<u32>(4);
    tx.try_send(1).unwrap();
    tx.try_send(2).unwrap();
    drop(tx);
    assert_eq!(rx.try_recv(), Ok(1));
    assert_eq!(rx.try_recv(), Ok(2));
    assert_eq!(rx.try_recv(), Err(RecvError::Disconnected));
}

#[test]
fn dropping_the_receiver_disconnects_the_sender() {
    let (mut tx, rx) = channel::<u32>(4);
    drop(rx);
    assert_eq!(tx.try_send(1), Err(SendError::Disconnected(1)));
}

#[test]
fn recv_timeout_wakes_on_send_and_times_out_when_idle() {
    let (mut tx, mut rx) = channel::<u32>(4);
    assert_eq!(
        rx.recv_timeout(Some(Duration::from_millis(20))),
        Err(RecvError::Timeout)
    );
    let sender = thread::spawn(move || {
        thread::sleep(Duration::from_millis(30));
        tx.send(9).unwrap();
        tx
    });
    assert_eq!(rx.recv_timeout(Some(Duration::from_secs(5))), Ok(9));
    drop(sender.join().unwrap());
}
