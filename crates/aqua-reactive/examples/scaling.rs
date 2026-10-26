//! Reproducible data-locality benchmark for `aqua-reactive`.
//!
//! Run: `cargo run --release --example scaling`
//!
//! Measures steady-state update cost (time + allocated bytes) for:
//! - `wide/N`: one source feeding N derived children (N = 10/1000/10000),
//!   plus a deep chain (1000 links) for rank-order propagation.
//!
//! Allocation counting uses a global counting allocator, so byte figures are
//! measured, not estimated. Timing uses `Instant` over many iterations with
//! a warmup; the table prints per-update ns and allocated bytes/update.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use aqua_reactive::Runtime;

struct CountingAlloc;
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static A: CountingAlloc = CountingAlloc;

fn reset_counters() {
    ALLOC_BYTES.store(0, Ordering::SeqCst);
    ALLOC_COUNT.store(0, Ordering::SeqCst);
}

fn counters() -> (usize, usize) {
    (
        ALLOC_BYTES.load(Ordering::SeqCst),
        ALLOC_COUNT.load(Ordering::SeqCst),
    )
}

fn bench_wide(n: usize, iters: u64) {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (src, set_src) = scope.signal(0i64);
    let mut sum = 0i64;
    for _ in 0..n {
        let d = scope.derive1(&src, move |v| std::hint::black_box(v) + 1);
        sum += d.get();
    }
    std::hint::black_box(sum);
    // Warmup (warms rank cache, order buffer, thunk buffers).
    for i in 0..100 {
        set_src.set(i);
    }
    reset_counters();
    let t0 = Instant::now();
    for i in 0..iters {
        set_src.set(100 + i as i64);
    }
    let dt = t0.elapsed();
    let (bytes, allocs) = counters();
    println!(
        "wide/{:<6} updates={:<6} {:>10.1} ns/update {:>12.1} bytes/update {:>8.2} allocs/update",
        n,
        iters,
        dt.as_nanos() as f64 / iters as f64,
        bytes as f64 / iters as f64,
        allocs as f64 / iters as f64,
    );
}

fn bench_chain(depth: usize, iters: u64) {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (src, set_src) = scope.signal(0i64);
    let mut cur = scope.derive1(&src, |v| std::hint::black_box(v) + 1);
    for _ in 1..depth {
        let next = scope.derive1(&cur, |v| std::hint::black_box(v) + 1);
        cur = next;
    }
    assert_eq!(cur.get(), depth as i64);
    for i in 0..100 {
        set_src.set(i);
    }
    reset_counters();
    let t0 = Instant::now();
    for i in 0..iters {
        set_src.set(100 + i as i64);
    }
    let dt = t0.elapsed();
    let (bytes, allocs) = counters();
    println!(
        "chain/{:<5} updates={:<6} {:>10.1} ns/update {:>12.1} bytes/update {:>8.2} allocs/update",
        depth,
        iters,
        dt.as_nanos() as f64 / iters as f64,
        bytes as f64 / iters as f64,
        allocs as f64 / iters as f64,
    );
    assert_eq!(cur.get(), 100 + iters as i64 + depth as i64 - 1);
}

fn main() {
    println!("aqua-reactive scaling (release, counting allocator, steady state)");
    bench_wide(10, 20_000);
    bench_wide(1_000, 2_000);
    bench_wide(10_000, 200);
    bench_chain(1_000, 2_000);
}
