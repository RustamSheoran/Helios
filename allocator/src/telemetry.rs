use crate::block::BlockAllocator;
use crate::slab::SlabAllocator;
use helios_shared::RawStderrLogger;

pub struct AllocatorTelemetry;

impl AllocatorTelemetry {
    /// Renders details about the allocator state and prints to stderr
    /// in an allocator-safe way (sequential raw writes).
    pub unsafe fn dump_state(block_alloc: &BlockAllocator, slab_alloc: &SlabAllocator) {
        RawStderrLogger::write_raw("\n=================== HELIOS MEMORY TELEMETRY ===================\n");

        // 1. Core Block Allocator Statistics
        let total_allocated = block_alloc.total_allocated_bytes;
        let total_free = block_alloc.total_free_bytes;
        let total_heap = total_allocated + total_free;

        let mut buf = [0u8; 128];
        let mut write_stat = |label: &str, value: usize| {
            RawStderrLogger::write_raw(label);
            let s = format_usize(value, &mut buf);
            RawStderrLogger::write_raw(s);
            RawStderrLogger::write_raw(" bytes\n");
        };

        write_stat("Total Virtual Memory Allocated: ", total_heap);
        write_stat("  ↳ Active Payload Allocations: ", total_allocated);
        write_stat("  ↳ Unused Free Pool Capacity : ", total_free);

        // 2. Fragmentation Analysis
        // Find largest free block
        let mut largest_free = 0;
        let mut curr = block_alloc.free_head;
        while !curr.is_null() {
            if (*curr).size > largest_free {
                largest_free = (*curr).size;
            }
            curr = (*curr).next_free;
        }

        let frag_percent = if total_free > 0 {
            let ratio = largest_free as f64 / total_free as f64;
            ((1.0 - ratio) * 100.0) as usize
        } else {
            0
        };

        RawStderrLogger::write_raw("External Heap Fragmentation    : ");
        let s = format_usize(frag_percent, &mut buf);
        RawStderrLogger::write_raw(s);
        RawStderrLogger::write_raw("%\n");

        // 3. Visual Block Layout Map
        RawStderrLogger::write_raw("Memory Block Layout Map        : ");
        let mut block = block_alloc.head;
        if block.is_null() {
            RawStderrLogger::write_raw("[Empty Heap]\n");
        } else {
            while !block.is_null() {
                if (*block).is_free {
                    RawStderrLogger::write_raw("[FREE: ");
                } else {
                    RawStderrLogger::write_raw("[ALLOC: ");
                }
                let size_str = format_usize((*block).size, &mut buf);
                RawStderrLogger::write_raw(size_str);
                RawStderrLogger::write_raw("]");
                
                block = (*block).next;
                if !block.is_null() {
                    RawStderrLogger::write_raw(" -> ");
                }
            }
            RawStderrLogger::write_raw("\n");
        }

        // 4. Slab Cache Saturation
        RawStderrLogger::write_raw("Slab Cache Saturation          :\n");
        for class in &slab_alloc.classes {
            let mut total_slabs = 0;
            let mut total_slots = 0;
            let mut free_slots = 0;

            let mut slab = class.head;
            while !slab.is_null() {
                total_slabs += 1;
                total_slots += (*slab).total_slots;
                free_slots += (*slab).free_slots;
                slab = (*slab).next;
            }

            if total_slabs > 0 {
                RawStderrLogger::write_raw("  ↳ Class ");
                RawStderrLogger::write_raw(format_usize(class.object_size, &mut buf));
                RawStderrLogger::write_raw("B : ");
                
                let used_slots = total_slots - free_slots;
                RawStderrLogger::write_raw(format_usize(used_slots, &mut buf));
                RawStderrLogger::write_raw("/");
                RawStderrLogger::write_raw(format_usize(total_slots, &mut buf));
                RawStderrLogger::write_raw(" slots used across ");
                RawStderrLogger::write_raw(format_usize(total_slabs, &mut buf));
                RawStderrLogger::write_raw(" slab pages\n");
            }
        }
        RawStderrLogger::write_raw("===============================================================\n\n");
    }
}

// Simple allocator-safe usize formatting helper that doesn't trigger allocations.
fn format_usize(val: usize, buf: &mut [u8]) -> &str {
    if val == 0 {
        return "0";
    }
    let mut v = val;
    let mut idx = buf.len();
    while v > 0 {
        idx -= 1;
        buf[idx] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    core::str::from_utf8(&buf[idx..]).unwrap_or("?")
}
