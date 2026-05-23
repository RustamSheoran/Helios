use core::sync::atomic::{AtomicBool, Ordering};
use helios_shared::{log_debug, log_error};

// Intrusive block header, aligned to 16 bytes for pointer compatibility.
#[repr(C, align(16))]
pub struct BlockHeader {
    pub size: usize,              // Size of the payload only (in bytes)
    pub is_free: bool,            // Is this block currently free?
    pub next: *mut BlockHeader,   // Next block in physical memory order
    pub prev: *mut BlockHeader,   // Previous block in physical memory order
    pub next_free: *mut BlockHeader, // Next block in the free list
    pub prev_free: *mut BlockHeader, // Previous block in the free list
}

/// A lightweight, allocation-free Spinlock to protect allocator state.
pub struct Spinlock {
    lock: AtomicBool,
}

impl Spinlock {
    pub const fn new() -> Self {
        Self {
            lock: AtomicBool::new(false),
        }
    }

    pub fn lock(&self) {
        while self.lock.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }

    pub fn unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }
}

pub struct BlockAllocator {
    pub head: *mut BlockHeader,       // Head of physical memory blocks list
    pub free_head: *mut BlockHeader,  // Head of the intrusive free list
    pub total_allocated_bytes: usize, // Track telemetry
    pub total_free_bytes: usize,      // Track telemetry
}

// BlockAllocator must be Send because it will be shared across threads under GlobalAlloc.
unsafe impl Send for BlockAllocator {}

impl BlockAllocator {
    pub const fn new() -> Self {
        Self {
            head: core::ptr::null_mut(),
            free_head: core::ptr::null_mut(),
            total_allocated_bytes: 0,
            total_free_bytes: 0,
        }
    }

    /// Allocates memory from the heap.
    pub unsafe fn allocate(&mut self, size: usize, align: usize) -> *mut u8 {
        // Enforce alignment to 16 bytes as standard to simplify offset calculations
        let aligned_size = (size + 15) & !15;

        // 1. Search free list using Best-Fit strategy
        let mut best_block: *mut BlockHeader = core::ptr::null_mut();
        let mut curr = self.free_head;

        while !curr.is_null() {
            if (*curr).size >= aligned_size {
                if best_block.is_null() || (*curr).size < (*best_block).size {
                    best_block = curr;
                }
            }
            curr = (*curr).next_free;
        }

        // 2. If a suitable free block is found, allocate from it
        if !best_block.is_null() {
            self.remove_from_free_list(best_block);
            (*best_block).is_free = false;

            // Check if we can split this block
            let header_size = core::mem::size_of::<BlockHeader>();
            // We require at least 16 bytes for the new split payload
            if (*best_block).size >= aligned_size + header_size + 16 {
                let split_address = (best_block as usize + header_size + aligned_size) as *mut BlockHeader;
                let remaining_size = (*best_block).size - aligned_size - header_size;

                // Configure the new split block
                core::ptr::write(
                    split_address,
                    BlockHeader {
                        size: remaining_size,
                        is_free: true,
                        next: (*best_block).next,
                        prev: best_block,
                        next_free: core::ptr::null_mut(),
                        prev_free: core::ptr::null_mut(),
                    },
                );

                if !(*best_block).next.is_null() {
                    (*(*best_block).next).prev = split_address;
                }
                (*best_block).next = split_address;
                (*best_block).size = aligned_size;

                // Add the newly created split block back into the free list
                self.add_to_free_list(split_address);
                self.total_free_bytes += remaining_size;
            }

            self.total_free_bytes -= (*best_block).size;
            self.total_allocated_bytes += (*best_block).size;

            let payload_ptr = (best_block as usize + core::mem::size_of::<BlockHeader>()) as *mut u8;
            log_debug!("allocator", "Block allocator: reused free block");
            return payload_ptr;
        }

        // 3. If no block matches, allocate new pages from the kernel using mmap
        let header_size = core::mem::size_of::<BlockHeader>();
        let total_needed = aligned_size + header_size;
        
        // Map at least 4KB (1 page) or a multiple of 4KB
        let page_size = 4096;
        let map_size = ((total_needed + page_size - 1) / page_size) * page_size;

        let mmap_ptr = libc::mmap(
            core::ptr::null_mut(),
            map_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );

        if mmap_ptr == libc::MAP_FAILED {
            log_error!("allocator", "Block allocator: mmap failed!");
            return core::ptr::null_mut();
        }

        let new_block = mmap_ptr as *mut BlockHeader;
        let payload_size = map_size - header_size;

        // Initialize block header
        core::ptr::write(
            new_block,
            BlockHeader {
                size: payload_size,
                is_free: true,
                next: core::ptr::null_mut(),
                prev: core::ptr::null_mut(),
                next_free: core::ptr::null_mut(),
                prev_free: core::ptr::null_mut(),
            },
        );

        // Chain into physical order tracking list
        if self.head.is_null() {
            self.head = new_block;
        } else {
            // Find the tail in memory order and append
            let mut tail = self.head;
            while !(*tail).next.is_null() {
                tail = (*tail).next;
            }
            (*tail).next = new_block;
            (*new_block).prev = tail;
        }

        // Add this massive fresh page to the free list
        self.add_to_free_list(new_block);
        self.total_free_bytes += payload_size;

        // Re-call allocate recursively — it is guaranteed to find and split our new block!
        self.allocate(aligned_size, align)
    }

    /// Deallocates memory at ptr.
    pub unsafe fn deallocate(&mut self, ptr: *mut u8) {
        if ptr.is_null() {
            return;
        }

        let header_size = core::mem::size_of::<BlockHeader>();
        let block = (ptr as usize - header_size) as *mut BlockHeader;

        if (*block).is_free {
            log_error!("allocator", "Block allocator: double-free detected!");
            return;
        }

        log_debug!("allocator", "Block allocator: block deallocated");

        (*block).is_free = true;
        self.total_allocated_bytes -= (*block).size;
        self.total_free_bytes += (*block).size;

        self.add_to_free_list(block);

        // Coalesce with adjacent physical neighbors to prevent fragmentation
        self.coalesce(block);
    }

    /// Merges physically adjacent free blocks.
    unsafe fn coalesce(&mut self, block: *mut BlockHeader) {
        let header_size = core::mem::size_of::<BlockHeader>();

        // 1. Try to merge with the next physical neighbor
        let next_block = (*block).next;
        if !next_block.is_null() && (*next_block).is_free {
            // Check if they are physically contiguous in the VM space
            if (block as usize + header_size + (*block).size) == next_block as usize {
                log_debug!("allocator", "Block allocator: coalesced next neighbor");
                
                // Remove both from the free list to handle updates
                self.remove_from_free_list(block);
                self.remove_from_free_list(next_block);

                // Absorb next block
                (*block).size += header_size + (*next_block).size;
                (*block).next = (*next_block).next;
                if !(*next_block).next.is_null() {
                    (*(*next_block).next).prev = block;
                }

                // Add merged block back to free list
                self.add_to_free_list(block);
            }
        }

        // 2. Try to merge with the previous physical neighbor
        let prev_block = (*block).prev;
        if !prev_block.is_null() && (*prev_block).is_free {
            // Check if they are physically contiguous
            if (prev_block as usize + header_size + (*prev_block).size) == block as usize {
                log_debug!("allocator", "Block allocator: coalesced prev neighbor");
                
                // Remove both from the free list
                self.remove_from_free_list(block);
                self.remove_from_free_list(prev_block);

                // Absorb current block into previous
                (*prev_block).size += header_size + (*block).size;
                (*prev_block).next = (*block).next;
                if !(*block).next.is_null() {
                    (*(*block).next).prev = prev_block;
                }

                // Add merged prev block back to free list
                self.add_to_free_list(prev_block);
            }
        }
    }

    // Helper functions to manage the intrusive free list
    unsafe fn add_to_free_list(&mut self, block: *mut BlockHeader) {
        if self.free_head.is_null() {
            self.free_head = block;
            (*block).next_free = core::ptr::null_mut();
            (*block).prev_free = core::ptr::null_mut();
        } else {
            // Insert at the head of the free list (O(1))
            (*block).next_free = self.free_head;
            (*block).prev_free = core::ptr::null_mut();
            (*self.free_head).prev_free = block;
            self.free_head = block;
        }
    }

    unsafe fn remove_from_free_list(&mut self, block: *mut BlockHeader) {
        if self.free_head == block {
            self.free_head = (*block).next_free;
        }

        if !(*block).next_free.is_null() {
            (*(*block).next_free).prev_free = (*block).prev_free;
        }

        if !(*block).prev_free.is_null() {
            (*(*block).prev_free).next_free = (*block).next_free;
        }

        (*block).next_free = core::ptr::null_mut();
        (*block).prev_free = core::ptr::null_mut();
    }
}
