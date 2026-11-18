use windows::core::{Interface, HRESULT};
use windows::Win32::Foundation::{S_FALSE, S_OK};
use windows::Win32::Graphics::Direct3D11::*;

use super::Error;
use crate::ledger::{record, Kind, Token};

pub const MAX_FRAMES_IN_FLIGHT: usize = 1;

pub struct FrameFence {
    queries: Vec<ID3D11Query>,
    in_flight: Vec<bool>,
    next: usize,
    _token: Token,
}

fn event_query(device: &ID3D11Device) -> Result<ID3D11Query, Error> {
    let mut created = None;
    unsafe {
        device.CreateQuery(
            &D3D11_QUERY_DESC {
                Query: D3D11_QUERY_EVENT,
                MiscFlags: 0,
            },
            Some(&mut created),
        )?
    };
    created.ok_or(Error::NoAdapter)
}

fn poll(context: &ID3D11DeviceContext1, query: &ID3D11Query) -> HRESULT {
    let mut done = windows::core::BOOL::default();
    unsafe {
        (Interface::vtable(context).base__.GetData)(
            Interface::as_raw(context),
            Interface::as_raw(query),
            &mut done as *mut windows::core::BOOL as *mut _,
            std::mem::size_of::<windows::core::BOOL>() as u32,
            0,
        )
    }
}

impl FrameFence {
    pub fn new(device: &ID3D11Device) -> Result<Self, Error> {
        Ok(Self {
            queries: (0..MAX_FRAMES_IN_FLIGHT)
                .map(|_| event_query(device))
                .collect::<Result<_, Error>>()?,
            in_flight: vec![false; MAX_FRAMES_IN_FLIGHT],
            next: 0,
            _token: record(
                "frame_fence",
                Kind::Query,
                (MAX_FRAMES_IN_FLIGHT * 8) as u64,
            ),
        })
    }

    pub fn wait_for_slot(&mut self, context: &ID3D11DeviceContext1) -> Result<(), Error> {
        if !self.in_flight[self.next] {
            return Ok(());
        }
        let query = &self.queries[self.next];
        loop {
            match poll(context, query) {
                S_OK => break,
                S_FALSE => std::thread::yield_now(),
                failure => return Err(windows::core::Error::from(failure).into()),
            }
        }
        self.in_flight[self.next] = false;
        Ok(())
    }

    pub fn signal(&mut self, context: &ID3D11DeviceContext1) {
        unsafe { context.End(&self.queries[self.next]) };
        self.in_flight[self.next] = true;
        self.next = (self.next + 1) % self.queries.len();
    }
}
