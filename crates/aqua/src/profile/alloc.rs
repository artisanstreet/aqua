#[cfg(feature = "profile-alloc")]
mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static COUNTS: Cell<[u64; 4]> = const { Cell::new([0; 4]) };
    }

    fn bump(slot: usize, amount: u64) {
        let _ = COUNTS.try_with(|counts| {
            let mut values = counts.get();
            values[slot] = values[slot].wrapping_add(amount);
            counts.set(values);
        });
    }

    pub struct Counting;

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            bump(0, 1);
            bump(1, layout.size() as u64);
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            bump(0, 1);
            bump(1, layout.size() as u64);
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            bump(2, 1);
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            bump(3, 1);
            if new_size > layout.size() {
                bump(1, (new_size - layout.size()) as u64);
            }
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }

    pub fn snapshot() -> [u64; 4] {
        COUNTS.try_with(Cell::get).unwrap_or([0; 4])
    }
}

#[cfg(feature = "profile-alloc")]
#[global_allocator]
static COUNTING: counting::Counting = counting::Counting;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeapCounts {
    pub allocs: u64,
    pub bytes: u64,
    pub frees: u64,
    pub reallocs: u64,
}

impl HeapCounts {
    #[must_use]
    pub fn since(self, earlier: HeapCounts) -> HeapCounts {
        HeapCounts {
            allocs: self.allocs.wrapping_sub(earlier.allocs),
            bytes: self.bytes.wrapping_sub(earlier.bytes),
            frees: self.frees.wrapping_sub(earlier.frees),
            reallocs: self.reallocs.wrapping_sub(earlier.reallocs),
        }
    }
}

pub const fn counting() -> bool {
    cfg!(feature = "profile-alloc")
}

#[cfg(feature = "profile-alloc")]
pub fn snapshot() -> HeapCounts {
    let [allocs, bytes, frees, reallocs] = counting::snapshot();
    HeapCounts {
        allocs,
        bytes,
        frees,
        reallocs,
    }
}

#[cfg(not(feature = "profile-alloc"))]
pub fn snapshot() -> HeapCounts {
    HeapCounts::default()
}
