use core::ptr;
use helios_shared::{log_debug, log_error};
use crate::block::BlockAllocator;

// Intrusive slot in a slab, stored in the slot's payload itself when free
pub struct SlabSlot {
    pub next: *mut SlabSlot,
}

// Header for a 4KB aligned Slab page
#[repr(C)]
pub struct Slab {
    pub magic: u32,                   // Magic Signature: 0x48454c49 ("HELI")
    pub object_size: usize,           // Size of objects managed by this slab
    pub total_slots: usize,           // Total slots configured in this page
    pub free_slots: usize,            // Count of free slots remaining
    pub next: *mut Slab,              // Next slab page in the size-class chain
    pub free_list_head: *mut SlabSlot, // Head of the intrusive free slot list
}

pub struct SlabSizeClass {
    pub object_size: usize,
    pub head: *mut Slab,
}

pub struct SlabAllocator {
    pub classes: [SlabSizeClass; 7], // 16, 32, 64, 128, 256, 512, 1024 bytes
}

impl SlabAllocator {
    pub const fn new() -> Self {
        Self {
            classes: [
                SlabSizeClass { object_size: 16, head: ptr::null_mut() },
                SlabSizeClass { object_size: 32, head: ptr::null_mut() },
                SlabSizeClass { object_size: 64, head: ptr::null_mut() },
                SlabSizeClass { object_size: 128, head: ptr::null_mut() },
                SlabSizeClass { object_size: 256, head: ptr::null_mut() },
                SlabSizeClass { object_size: 512, head: ptr::null_mut() },
                SlabSizeClass { object_size: 1024, head: ptr::null_mut() },
            ],
        }
    }

    /// Try to allocate using Slab if size fits standard class
    pub unsafe fn allocate(&mut self, size: usize, _block_alloc: &mut BlockAllocator) -> *mut u8 {
        let size_class_idx = self.get_size_class_index(size);
        if size_class_idx.is_none() {
            return ptr::null_mut(); // Exceeds slab sizes, fallback to block allocator
        }

        let idx = size_class_idx.unwrap();
        let class = &mut self.classes[idx];
        let obj_size = class.object_size;

        // Search for a slab with free slots
        let mut curr_slab = class.head;
        while !curr_slab.is_null() {
            if (*curr_slab).free_slots > 0 {
                break;
            }
            curr_slab = (*curr_slab).next;
        }

        // If no slab has free slots, allocate a new page directly from the OS using raw mmap
        if curr_slab.is_null() {
            log_debug!("allocator", "Slab allocator: allocating new slab page");
            
            // Map exactly 1 page (4096 bytes) from the OS, page-aligned
            let mmap_ptr = libc::mmap(
                core::ptr::null_mut(),
                4096,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            );

            if mmap_ptr == libc::MAP_FAILED {
                log_error!("allocator", "Slab allocator: failed to allocate page via mmap");
                return ptr::null_mut();
            }

            curr_slab = mmap_ptr as *mut Slab;
            let header_size = core::mem::size_of::<Slab>();
            
            // Calculate total slots we can fit in the remaining space of the 4KB page
            let available_space = 4096 - header_size;
            let total_slots = available_space / obj_size;

            ptr::write(
                curr_slab,
                Slab {
                    magic: 0x48454c49, // "HELI"
                    object_size: obj_size,
                    total_slots,
                    free_slots: total_slots,
                    next: class.head,
                    free_list_head: ptr::null_mut(),
                },
            );

            // Populate the free slots list inside the slab
            let slots_start = (mmap_ptr as usize + header_size) as *mut u8;
            for i in 0..total_slots {
                let slot_ptr = slots_start.add(i * obj_size) as *mut SlabSlot;
                (*slot_ptr).next = (*curr_slab).free_list_head;
                (*curr_slab).free_list_head = slot_ptr;
            }

            class.head = curr_slab;
        }

        // Allocate slot from the selected slab
        let slot = (*curr_slab).free_list_head;
        (*curr_slab).free_list_head = (*slot).next;
        (*curr_slab).free_slots -= 1;

        log_debug!("allocator", "Slab allocator: allocated slot");
        slot as *mut u8
    }

    /// Try to deallocate from Slab. Returns true if pointer belonged to a Slab.
    pub unsafe fn deallocate(&mut self, ptr: *mut u8) -> bool {
        if ptr.is_null() {
            return false;
        }

        // The Page-Alignment Round-Down Trick:
        // Round down pointer to nearest 4096 byte boundary to get Slab header
        let slab_ptr = (ptr as usize & !(4095)) as *mut Slab;

        // Verify this is a valid slab header by checking the Magic Signature
        if (*slab_ptr).magic != 0x48454c49 {
            return false; // Not a slab page (likely a raw block allocation)
        }

        // Additional safeguard check on object size
        let obj_size = (*slab_ptr).object_size;
        if !self.is_valid_size_class(obj_size) {
            return false;
        }

        log_debug!("allocator", "Slab allocator: slot deallocated");

        let slot = ptr as *mut SlabSlot;
        (*slot).next = (*slab_ptr).free_list_head;
        (*slab_ptr).free_list_head = slot;
        (*slab_ptr).free_slots += 1;

        true
    }

    fn get_size_class_index(&self, size: usize) -> Option<usize> {
        if size <= 16 { Some(0) }
        else if size <= 32 { Some(1) }
        else if size <= 64 { Some(2) }
        else if size <= 128 { Some(3) }
        else if size <= 256 { Some(4) }
        else if size <= 512 { Some(5) }
        else if size <= 1024 { Some(6) }
        else { None }
    }

    fn is_valid_size_class(&self, size: usize) -> bool {
        matches!(size, 16 | 32 | 64 | 128 | 256 | 512 | 1024)
    }
}
