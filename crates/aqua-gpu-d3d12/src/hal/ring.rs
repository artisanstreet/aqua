use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingError {
    Full,
    TooLarge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mark {
    fence: u64,
    end: u64,
    bytes: u64,
}

#[derive(Clone, Debug)]
pub struct RingLayout {
    capacity: u64,
    head: u64,
    tail: u64,
    used: u64,
    since_mark: u64,
    marks: VecDeque<Mark>,
}

fn align_up(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment.max(1)) * alignment.max(1)
}

impl RingLayout {
    pub fn new(capacity: u64) -> Self {
        Self {
            capacity,
            head: 0,
            tail: 0,
            used: 0,
            since_mark: 0,
            marks: VecDeque::new(),
        }
    }

    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    pub fn used(&self) -> u64 {
        self.used
    }

    pub fn is_idle(&self) -> bool {
        self.used == 0
    }

    pub fn oldest_fence(&self) -> Option<u64> {
        self.marks.front().map(|mark| mark.fence)
    }

    pub fn allocate(&mut self, size: u64, alignment: u64) -> Result<u64, RingError> {
        let size = size.max(1);
        if size > self.capacity {
            return Err(RingError::TooLarge);
        }
        if self.used == 0 && self.marks.is_empty() {
            self.head = 0;
            self.tail = 0;
        }
        if self.used >= self.capacity {
            return Err(RingError::Full);
        }
        let (offset, consumed, head) =
            match self.head >= self.tail && self.used > 0 || self.used == 0 {
                true => {
                    let aligned = align_up(self.head, alignment);
                    match aligned + size <= self.capacity {
                        true => (aligned, aligned - self.head + size, aligned + size),
                        false => match size <= self.tail {
                            true => (0, self.capacity - self.head + size, size),
                            false => return Err(RingError::Full),
                        },
                    }
                }
                false => {
                    let aligned = align_up(self.head, alignment);
                    match aligned + size <= self.tail {
                        true => (aligned, aligned - self.head + size, aligned + size),
                        false => return Err(RingError::Full),
                    }
                }
            };
        self.head = match head >= self.capacity {
            true => 0,
            false => head,
        };
        self.used += consumed;
        self.since_mark += consumed;
        Ok(offset)
    }

    pub fn mark(&mut self, fence: u64) {
        if self.since_mark == 0 {
            return;
        }
        self.marks.push_back(Mark {
            fence,
            end: self.head,
            bytes: self.since_mark,
        });
        self.since_mark = 0;
    }

    pub fn retire(&mut self, completed: u64) {
        while let Some(mark) = self.marks.front().copied() {
            if mark.fence > completed {
                break;
            }
            self.marks.pop_front();
            self.used -= mark.bytes;
            self.tail = mark.end;
        }
        if self.used == 0 && self.marks.is_empty() && self.since_mark == 0 {
            self.head = 0;
            self.tail = 0;
        }
    }

    pub fn grow_to(&mut self, capacity: u64) {
        assert!(
            self.used == 0 && self.marks.is_empty(),
            "grow requires an idle ring"
        );
        *self = Self::new(capacity);
    }
}

#[derive(Clone, Debug)]
pub struct SlotAllocator {
    free: Vec<u32>,
    next: u32,
    capacity: u32,
}

impl SlotAllocator {
    pub fn new(capacity: u32) -> Self {
        Self {
            free: Vec::new(),
            next: 0,
            capacity,
        }
    }

    pub fn allocate(&mut self) -> Option<u32> {
        match self.free.pop() {
            Some(index) => Some(index),
            None if self.next < self.capacity => {
                self.next += 1;
                Some(self.next - 1)
            }
            None => None,
        }
    }

    pub fn release(&mut self, index: u32) {
        debug_assert!(index < self.next && !self.free.contains(&index));
        self.free.push(index);
    }

    pub fn live(&self) -> u32 {
        self.next - self.free.len() as u32
    }
}
