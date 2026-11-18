//! Debug-only "page heap": every allocation ends exactly at a no-access guard page,
//! so any out-of-bounds read (e.g. by a Win32 API) faults immediately.
//! Enabled with `cargo build --features guard-alloc`.

use std::alloc::{GlobalAlloc, Layout};
use windows_sys::Win32::System::Memory::{VirtualAlloc, VirtualFree, VirtualProtect, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS, PAGE_READWRITE};

const PAGE: usize = 4096;

pub struct GuardAlloc;

fn sizes(l: &Layout) -> (usize, usize) {
    let rsize = (l.size().max(1) + l.align() - 1) / l.align() * l.align();
    let pages = rsize.div_ceil(PAGE);
    (rsize, pages)
}

unsafe impl GlobalAlloc for GuardAlloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let (rsize, pages) = sizes(&l);
        let base = VirtualAlloc(std::ptr::null(), (pages + 1) * PAGE, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) as *mut u8;
        if base.is_null() {
            return base;
        }
        let guard = base.add(pages * PAGE);
        let mut old = 0;
        VirtualProtect(guard as _, PAGE, PAGE_NOACCESS, &mut old);
        guard.sub(rsize)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        let (rsize, pages) = sizes(&l);
        let base = p.add(rsize).sub(pages * PAGE);
        VirtualFree(base as _, 0, MEM_RELEASE);
    }
}
