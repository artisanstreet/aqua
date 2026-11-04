use aqua_gpu_d3d12::hal::ring::{RingError, RingLayout, SlotAllocator};

struct Live {
    start: u64,
    end: u64,
    fence: u64,
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

fn overlaps(a: &Live, b: &Live) -> bool {
    a.start < b.end && b.start < a.end
}

#[test]
fn allocations_in_flight_never_overlap_and_stay_aligned() {
    let mut random = Lcg(7);
    let mut ring = RingLayout::new(4096);
    let mut live: Vec<Live> = Vec::new();
    let mut completed = 0u64;
    let mut fence = 0u64;
    let mut pending: Vec<(u64, u64, u64)> = Vec::new();
    for _ in 0..20_000 {
        match random.next() % 10 {
            0..=6 => {
                let size = 1 + random.next() % 700;
                let alignment = [1, 4, 16, 256][(random.next() % 4) as usize];
                match ring.allocate(size, alignment) {
                    Ok(offset) => {
                        assert_eq!(offset % alignment, 0);
                        assert!(offset + size <= 4096);
                        pending.push((offset, offset + size, 0));
                    }
                    Err(RingError::Full) | Err(RingError::TooLarge) => {}
                }
            }
            7 | 8 => {
                fence += 1;
                ring.mark(fence);
                for (start, end, _) in pending.drain(..) {
                    live.push(Live { start, end, fence });
                }
            }
            _ => {
                completed = completed.max(fence.saturating_sub(random.next() % 3));
                ring.retire(completed);
                live.retain(|entry| entry.fence > completed);
            }
        }
        let in_flight: Vec<Live> = live
            .iter()
            .map(|entry| Live {
                start: entry.start,
                end: entry.end,
                fence: entry.fence,
            })
            .chain(pending.iter().map(|(start, end, _)| Live {
                start: *start,
                end: *end,
                fence: u64::MAX,
            }))
            .collect();
        for (index, first) in in_flight.iter().enumerate() {
            for second in &in_flight[index + 1..] {
                assert!(
                    !overlaps(first, second),
                    "overlap {}..{} vs {}..{}",
                    first.start,
                    first.end,
                    second.start,
                    second.end
                );
            }
        }
        assert!(ring.used() <= ring.capacity());
    }
}

#[test]
fn retiring_everything_makes_the_ring_reusable_from_zero() {
    let mut ring = RingLayout::new(1024);
    assert_eq!(ring.allocate(1000, 1), Ok(0));
    assert_eq!(ring.allocate(100, 1), Err(RingError::Full));
    ring.mark(1);
    ring.retire(1);
    assert!(ring.is_idle());
    assert_eq!(ring.allocate(1024, 1), Ok(0));
}

#[test]
fn wrapping_wastes_the_tail_and_frees_it_with_the_wrapping_frame() {
    let mut ring = RingLayout::new(1000);
    assert_eq!(ring.allocate(600, 1), Ok(0));
    ring.mark(1);
    assert_eq!(ring.allocate(300, 1), Ok(600));
    ring.mark(2);
    ring.retire(1);
    assert_eq!(ring.allocate(500, 1), Ok(0));
    assert_eq!(ring.used(), 300 + 100 + 500);
    ring.mark(3);
    ring.retire(2);
    ring.retire(3);
    assert!(ring.is_idle());
}

#[test]
fn oversized_requests_are_rejected_for_growth() {
    let mut ring = RingLayout::new(256);
    assert_eq!(ring.allocate(257, 1), Err(RingError::TooLarge));
}

#[test]
fn oldest_fence_names_the_frame_to_wait_for() {
    let mut ring = RingLayout::new(100);
    ring.allocate(60, 1).unwrap();
    ring.mark(5);
    ring.allocate(30, 1).unwrap();
    ring.mark(6);
    assert_eq!(ring.allocate(50, 1), Err(RingError::Full));
    assert_eq!(ring.oldest_fence(), Some(5));
}

#[test]
fn slots_are_recycled_before_new_ones_are_handed_out() {
    let mut slots = SlotAllocator::new(3);
    let first = slots.allocate().unwrap();
    let second = slots.allocate().unwrap();
    slots.release(first);
    assert_eq!(slots.allocate(), Some(first));
    assert_eq!(slots.allocate(), Some(2));
    assert_eq!(slots.allocate(), None);
    assert_eq!(slots.live(), 3);
    slots.release(second);
    assert_eq!(slots.live(), 2);
}
