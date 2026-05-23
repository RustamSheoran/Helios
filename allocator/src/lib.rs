#![no_std] // Ensure standard library is not strictly assumed inside allocator logic

extern crate libc;
extern crate helios_shared;

pub mod block;
pub mod slab;
pub mod telemetry;

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use block::{BlockAllocator, Spinlock};
use slab::SlabAllocator;
use telemetry::AllocatorTelemetry;

/// The global memory allocator for Project Helios.
pub struct HeliosAllocator {
    pub lock: Spinlock,
    // Wrapped in UnsafeCell to enable legal interior mutability under Rust's rules.
    pub block_alloc: UnsafeCell<BlockAllocator>,
    pub slab_alloc: UnsafeCell<SlabAllocator>,
}

// Explicitly implement Send and Sync, as UnsafeCell disables them by default.
// This is safe because our custom Spinlock guarantees mutually exclusive access.
unsafe impl Send for HeliosAllocator {}
unsafe impl Sync for HeliosAllocator {}

impl HeliosAllocator {
    /// Creates a static, thread-safe instance of the allocator.
    pub const fn new() -> Self {
        Self {
            lock: Spinlock::new(),
            block_alloc: UnsafeCell::new(BlockAllocator::new()),
            slab_alloc: UnsafeCell::new(SlabAllocator::new()),
        }
    }

    /// Allocator-safe method to dump telemetry layout.
    pub fn dump_telemetry(&self) {
        unsafe {
            self.lock.lock();
            AllocatorTelemetry::dump_state(&*self.block_alloc.get(), &*self.slab_alloc.get());
            self.lock.unlock();
        }
    }
}

unsafe impl GlobalAlloc for HeliosAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.lock.lock();

        let block_alloc = &mut *self.block_alloc.get();
        let slab_alloc = &mut *self.slab_alloc.get();

        // 1. Try to allocate via the fast fixed-size Slab cache first
        let mut ptr = slab_alloc.allocate(layout.size(), block_alloc);

        // 2. If size class fits but Slab was exhausted, or size was too large,
        // fall back to the raw page block allocator.
        if ptr.is_null() {
            ptr = block_alloc.allocate(layout.size(), layout.align());
        }

        self.lock.unlock();
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        if ptr.is_null() {
            return;
        }

        self.lock.lock();

        let block_alloc = &mut *self.block_alloc.get();
        let slab_alloc = &mut *self.slab_alloc.get();

        // 1. Try to return to Slab allocator first
        let slab_freed = slab_alloc.deallocate(ptr);

        // 2. If it did not belong to a slab class, return it to the block allocator
        if !slab_freed {
            block_alloc.deallocate(ptr);
        }

        self.lock.unlock();
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
        let new_ptr = self.alloc(new_layout);
        if new_ptr.is_null() {
            return core::ptr::null_mut();
        }
        let copy_size = core::cmp::min(layout.size(), new_size);
        core::ptr::copy_nonoverlapping(ptr, new_ptr, copy_size);
        self.dealloc(ptr, layout);
        new_ptr
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::alloc::Layout;

    #[test]
    fn test_slab_allocation_sizes() {
        let allocator = HeliosAllocator::new();
        unsafe {
            // Allocate a small 16-byte object
            let layout_16 = Layout::from_size_align(16, 8).unwrap();
            let ptr_16 = allocator.alloc(layout_16);
            assert!(!ptr_16.is_null());

            // Validate that the slot is page-aligned to some 4096 slab page
            let slab_ptr = (ptr_16 as usize & !(4095)) as *mut slab::Slab;
            assert_eq!((*slab_ptr).magic, 0x48454c49);
            assert_eq!((*slab_ptr).object_size, 16);

            // Free the object
            allocator.dealloc(ptr_16, layout_16);
        }
    }

    #[test]
    fn test_large_block_allocations() {
        let allocator = HeliosAllocator::new();
        unsafe {
            // Allocate a large block (exceeding slab classes)
            let layout_large = Layout::from_size_align(4000, 16).unwrap();
            let ptr_large = allocator.alloc(layout_large);
            assert!(!ptr_large.is_null());

            // Since it's large, it should not be in the slab cache
            let slab_ptr = (ptr_large as usize & !(4095)) as *mut slab::Slab;
            // The magic signature must not match the slab cache magic
            assert_ne!((*slab_ptr).magic, 0x48454c49);

            // Free the large block
            allocator.dealloc(ptr_large, layout_large);
        }
    }

    #[test]
    fn test_block_coalescing_and_fragmentation() {
        let allocator = HeliosAllocator::new();
        unsafe {
            let layout_a = Layout::from_size_align(2048, 16).unwrap();
            let layout_b = Layout::from_size_align(2048, 16).unwrap();

            let ptr_a = allocator.alloc(layout_a);
            let ptr_b = allocator.alloc(layout_b);

            assert!(!ptr_a.is_null());
            assert!(!ptr_b.is_null());

            // Deallocate them
            allocator.dealloc(ptr_a, layout_a);
            allocator.dealloc(ptr_b, layout_b);

            // Verify they coalesced and returned all allocated space to free pool
            let block_alloc = &*allocator.block_alloc.get();
            assert_eq!(block_alloc.total_allocated_bytes, 0);
        }
    }
}
