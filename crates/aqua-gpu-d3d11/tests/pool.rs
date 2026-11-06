use std::time::{Duration, Instant};

use aqua_gpu_d3d11::pool::{Pool, POOL_LIMIT};

#[test]
fn take_returns_the_most_recently_released_match_only_once() {
    let now = Instant::now();
    let mut pool: Pool<(u32, u32), &str> = Pool::default();
    pool.put((1, 1), "old", now);
    pool.put((2, 2), "other", now);
    pool.put((1, 1), "new", now);
    assert_eq!(pool.take(&(1, 1)), Some("new"));
    assert_eq!(pool.take(&(1, 1)), Some("old"));
    assert_eq!(pool.take(&(1, 1)), None);
    assert_eq!(pool.len(), 1);
}

#[test]
fn expiry_drops_only_entries_older_than_the_deadline() {
    let start = Instant::now();
    let mut pool: Pool<u32, u32> = Pool::default();
    pool.put(1, 10, start);
    pool.put(2, 20, start + Duration::from_secs(6));
    let dropped = pool.expire(start + Duration::from_secs(11), Duration::from_secs(10));
    assert_eq!(dropped, 1);
    assert_eq!(pool.take(&1), None);
    assert_eq!(pool.take(&2), Some(20));
}

#[test]
fn the_pool_is_bounded_and_evicts_the_oldest_release() {
    let now = Instant::now();
    let mut pool: Pool<usize, usize> = Pool::default();
    (0..POOL_LIMIT + 2).for_each(|index| pool.put(index, index, now));
    assert_eq!(pool.len(), POOL_LIMIT);
    assert_eq!(pool.take(&0), None);
    assert_eq!(pool.take(&(POOL_LIMIT + 1)), Some(POOL_LIMIT + 1));
}
