use std::collections::VecDeque;

use super::types::{AtlasUpdates, PageFormat, PageId, TexelUpload};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamBudget {
    pub bytes_per_frame: u64,
}

impl Default for StreamBudget {
    fn default() -> Self {
        Self {
            bytes_per_frame: 4 * 1024 * 1024,
        }
    }
}

struct Band {
    upload: TexelUpload,
}

#[derive(Default)]
pub struct Streamer {
    queue: VecDeque<Band>,
    queued_bytes: u64,
}

fn split(upload: TexelUpload, format: PageFormat, band_bytes: u64) -> Vec<TexelUpload> {
    let row_bytes = upload.width as u64 * format.bytes_per_texel() as u64;
    let rows_per_band = (band_bytes / row_bytes.max(1)).max(1) as u32;
    match upload.height <= rows_per_band {
        true => vec![upload],
        false => (0..upload.height)
            .step_by(rows_per_band as usize)
            .map(|row| {
                let rows = rows_per_band.min(upload.height - row);
                let start = row as usize * row_bytes as usize;
                let end = start + rows as usize * row_bytes as usize;
                TexelUpload {
                    page: upload.page,
                    x: upload.x,
                    y: upload.y + row,
                    width: upload.width,
                    height: rows,
                    data: upload.data[start..end].to_vec(),
                }
            })
            .collect(),
    }
}

impl Streamer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn queued_bytes(&self) -> u64 {
        self.queued_bytes
    }

    pub fn is_pending(&self, page: PageId) -> bool {
        self.queue.iter().any(|band| band.upload.page == page)
    }

    pub fn is_idle(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn submit(&mut self, updates: AtlasUpdates, budget: StreamBudget) -> AtlasUpdates {
        updates.released.iter().for_each(|page| {
            self.queue.retain(|band| band.upload.page != *page);
        });
        self.queued_bytes = self
            .queue
            .iter()
            .map(|band| band.upload.data.len() as u64)
            .sum();
        let band_bytes = budget.bytes_per_frame.max(1);
        for upload in updates.uploads {
            let format = upload.page.format;
            for piece in split(upload, format, band_bytes) {
                self.queued_bytes += piece.data.len() as u64;
                self.queue.push_back(Band { upload: piece });
            }
        }
        let mut out = AtlasUpdates {
            released: updates.released,
            created: updates.created,
            uploads: Vec::new(),
        };
        let mut spent = 0u64;
        while let Some(band) = self.queue.front() {
            let bytes = band.upload.data.len() as u64;
            if spent > 0 && spent + bytes > budget.bytes_per_frame {
                break;
            }
            spent += bytes;
            self.queued_bytes -= bytes;
            if let Some(band) = self.queue.pop_front() {
                out.uploads.push(band.upload);
            }
        }
        out
    }
}
