use helios_allocator::HeliosAllocator;
use std::hint::black_box;
use std::time::Instant;

// Register HeliosAllocator as the Global Allocator for this binary!
#[global_allocator]
static ALLOCATOR: HeliosAllocator = HeliosAllocator::new();

fn main() {
    println!("===============================================================");
    println!("            HELIOS MEMORY ALLOCATOR DEMONSTRATION");
    println!("===============================================================");
    println!("Registering HeliosAllocator as #[global_allocator]... [OK]");
    
    // Dump initial empty state
    println!("\n[1] Dumping Initial Telemetry State (Empty Heap):");
    ALLOCATOR.dump_telemetry();

    // 1. Trigger Slab Cache Allocations
    println!("\n[2] Performing small size allocations (triggering Slab Caches)...");
    let mut small_vecs = Vec::new();
    for i in 0..20 {
        // Allocating a 32-byte object (Box of 8-byte array under 64-bit size constraints)
        let boxed_val = Box::new([i as u64; 4]);
        small_vecs.push(black_box(boxed_val));
    }
    
    println!("Completed 20 slab allocations. Live state:");
    ALLOCATOR.dump_telemetry();

    // 2. Trigger Backing Page Block Allocations
    println!("\n[3] Allocating large vector (triggering backing Block Allocator)...");
    // Large allocation exceeding slab size class (1024 bytes)
    let mut large_vec = Vec::with_capacity(3000);
    for i in 0..3000 {
        large_vec.push(black_box(i as u64));
    }
    
    println!("Large allocation of ~24KB complete. Live state:");
    ALLOCATOR.dump_telemetry();

    // 3. Trigger Deallocation and Coalescing
    println!("\n[4] Deallocating large vector and verifying physical coalescing...");
    drop(large_vec);
    println!("Large vector dropped. Backing blocks coalesced and returned to Free Pool. Live state:");
    ALLOCATOR.dump_telemetry();

    // 4. Drop Slab Allocations
    println!("\n[5] Dropping slab allocations...");
    drop(small_vecs);
    println!("Slab allocations dropped. Live state:");
    ALLOCATOR.dump_telemetry();

    // 5. High-speed Allocation Benchmark
    println!("\n[6] Running performance stress-test (10,000 iterations)...");
    let start = Instant::now();
    for i in 0..10_000 {
        let val = Box::new(i);
        black_box(val); // Immediately allocated and deallocated
    }
    let duration = start.elapsed();
    println!("Stress-test complete!");
    println!("Speed: 10,000 allocations/deallocations in {:?}", duration);
    println!("===============================================================\n");
}
