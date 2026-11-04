#[derive(Clone, Debug)]
struct Shelf {
    y: u32,
    height: u32,
    cursor: u32,
}

#[derive(Clone, Debug)]
pub struct ShelfPacker {
    width: u32,
    height: u32,
    shelves: Vec<Shelf>,
    next_y: u32,
}

fn shelf_height(height: u32) -> u32 {
    match height {
        0..=64 => height.div_ceil(4) * 4,
        _ => height,
    }
}

impl ShelfPacker {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            shelves: Vec::new(),
            next_y: 0,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn reset(&mut self) {
        self.shelves.clear();
        self.next_y = 0;
    }

    pub fn allocate(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        if width == 0 || height == 0 || width > self.width || height > self.height {
            return None;
        }
        let best = self
            .shelves
            .iter_mut()
            .filter(|shelf| {
                shelf.height >= height
                    && shelf.height <= height.saturating_mul(2)
                    && shelf.cursor + width <= self.width
            })
            .min_by_key(|shelf| shelf.height);
        match best {
            Some(shelf) => {
                let position = (shelf.cursor, shelf.y);
                shelf.cursor += width;
                Some(position)
            }
            None => {
                let shelf_h = shelf_height(height).min(self.height - self.next_y.min(self.height));
                match self.next_y + height <= self.height && shelf_h >= height {
                    true => {
                        let y = self.next_y;
                        self.shelves.push(Shelf {
                            y,
                            height: shelf_h,
                            cursor: width,
                        });
                        self.next_y += shelf_h;
                        Some((0, y))
                    }
                    false => None,
                }
            }
        }
    }
}
