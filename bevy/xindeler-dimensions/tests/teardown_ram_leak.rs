//! BL-82 EM-4.6 (T47.8) measured acceptance: a RAM delta over N repeated
//! REAL spinup->teardown cycles shows no net growth (spec §1.9's own
//! acceptance bar — "one of two items in Phase 4 that most directly needs a
//! real measurement, not just a code review").
//!
//! ## Why a counting global allocator, not `/proc/self/status` RSS
//! Process RSS is a poor leak signal here: allocators (including the system
//! allocator on both Linux glibc and macOS) commonly keep freed pages in
//! per-thread/per-arena caches rather than returning them to the OS
//! immediately, so RSS can plateau at a "high-water mark" even when the
//! actual LIVE heap is perfectly flat — a real leak and normal allocator
//! caching look identical from RSS alone, and a genuinely leak-free run can
//! still show RSS "growth" that never reverses. Wrapping the process's
//! global allocator to count live (allocated − freed) bytes measures the
//! ACTUAL fact this test cares about — total bytes the program itself still
//! considers reachable/in-use — immune to that noise, and honest about a
//! real per-cycle leak (which WOULD show as monotonic growth in this
//! metric, cycle over cycle).
//!
//! Needs real assets (tiny procgen worlds, `Index::new` loads the
//! color/feature manifests) — same `#[ignore]` convention this crate's other
//! asset-dependent tests already use (`registry.rs`'s
//! `chunk_generation_is_isolated_per_dimension`, `spinup.rs`'s
//! `spinup_dimension_message_reaches_active_via_the_task_pool`). Run with:
//! `VELOREN_ASSETS="$(pwd)/assets" cargo test -p xindeler-dimensions \
//!   --test teardown_ram_leak -- --ignored`

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::{
        Arc,
        atomic::{AtomicIsize, Ordering},
    },
    time::{Duration, Instant},
};

use bevy::{
    MinimalPlugins,
    app::PluginGroup,
    ecs::entity::Entity,
    prelude::*,
    time::{Fixed, TimeUpdateStrategy},
};
use xindeler_dimensions::{
    DimensionId, DimensionRegistry, DimensionSpinupConfig, DimensionsPlugin, SpinupDimension,
    WorldGenThreadPool,
};

/// Wraps [`System`], counting live (allocated − freed) bytes across the
/// WHOLE test binary — every allocation from every dependency (`server`,
/// `world`, `rayon`, Bevy's own ECS storages, …) is included, giving a true
/// process-wide "how much heap is actually in use right now" reading. Scoped
/// to this one integration-test binary (each `tests/*.rs` file compiles to
/// its own binary) — does not affect the crate's unit tests or any other
/// integration test file.
struct CountingAllocator;

static LIVE_BYTES: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE_BYTES.fetch_sub(layout.size() as isize, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            LIVE_BYTES.fetch_add(
                new_size as isize - layout.size() as isize,
                Ordering::Relaxed,
            );
        }
        new_ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        ptr
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn live_bytes() -> isize { LIVE_BYTES.load(Ordering::Relaxed) }

/// Runs ONE full real spinup -> (synthetic occupant join+leave) -> teardown
/// cycle against `app`, blocking (via a wall-clock-deadline poll loop,
/// mirroring `spinup.rs`'s own ignored test's exact pattern — generation
/// runs on REAL OS worker threads regardless of how fast this loop calls
/// `app.update()`) until the dimension is fully gone from the registry
/// again.
fn run_one_cycle(app: &mut App, id: u64) {
    app.world_mut().write_message(SpinupDimension {
        id: DimensionId(id),
        base_seed: id as u32,
        config: DimensionSpinupConfig::default(),
    });

    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        app.update();
        if app
            .world()
            .resource::<DimensionRegistry>()
            .lifecycle(DimensionId(id))
            == Some(xindeler_dimensions::DimensionLifecycle::Active)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        app.world()
            .resource::<DimensionRegistry>()
            .lifecycle(DimensionId(id)),
        Some(xindeler_dimensions::DimensionLifecycle::Active),
        "dimension {id} should reach Active before this cycle can proceed"
    );

    // A synthetic occupant joins then leaves — the empty-Draining path
    // (`begin_draining` tears down immediately once occupants hit zero)
    // exercises the SAME real transition a genuinely empty player session
    // would, without needing a real connected client for this RAM test.
    let dummy = Entity::from_raw_u32((id as u32) + 1000).expect("small synthetic entity id");
    {
        let mut registry = app.world_mut().resource_mut::<DimensionRegistry>();
        registry.try_add_occupant(DimensionId(id), dummy).unwrap();
        registry.remove_occupant(DimensionId(id), dummy).unwrap();
        registry.begin_draining(DimensionId(id)).unwrap();
    }

    // A handful of updates lets `teardown_completed_dimensions` remove the
    // registry entry and the deferred despawn command flush.
    for _ in 0..5 {
        app.update();
    }
    assert!(
        !app.world()
            .resource::<DimensionRegistry>()
            .contains(DimensionId(id)),
        "dimension {id} should be fully torn down (removed from the registry) after this cycle"
    );
}

#[test]
#[ignore = "boots real (tiny) worlds repeatedly: needs assets; run locally with \
            VELOREN_ASSETS=\"$(pwd)/assets\""]
fn spinup_teardown_cycles_show_no_net_ram_growth() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins.build());
    app.add_plugins(DimensionsPlugin);
    app.insert_resource(WorldGenThreadPool(Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap(),
    )));
    // EM-4.10 Finding B: the dimension-lifecycle chain now lives in
    // `FixedUpdate`, not `Update`. `run_one_cycle`'s tail loop (5 back-to-
    // back `app.update()` calls with no pacing) used to rely on `Update`
    // firing unconditionally every call; `FixedUpdate` instead depends on
    // `Time::<Fixed>`'s real-time accumulator actually crossing a timestep,
    // which a tight no-sleep loop cannot guarantee. Pin the fixed step so
    // every `app.update()` call deterministically runs the chain exactly
    // once, matching this test's original assumption (mirrors the same
    // fix `xindeler-sim-bridge`'s own `FixedUpdate`-dependent tests already
    // use).
    app.insert_resource(Time::<Fixed>::from_hz(64.0));
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 64.0,
    )));

    // Warm-up cycle: lets one-time allocator/thread-pool/lazy-static
    // initialization overhead happen OUTSIDE the measured window (asset
    // manifests, thread pools, etc. legitimately allocate once and stay —
    // that is not a per-cycle leak, and including it in the baseline avoids
    // it masquerading as one).
    run_one_cycle(&mut app, 900);

    const CYCLES: u64 = 5;
    let mut samples = Vec::with_capacity(CYCLES as usize);
    for i in 0..CYCLES {
        run_one_cycle(&mut app, 100 + i);
        samples.push(live_bytes());
    }

    let baseline = samples[0];
    let last = *samples.last().unwrap();
    let delta = last - baseline;
    // Generous tolerance (allocator fragmentation/arena slack is real and
    // expected) — the bar this test enforces is "no UNBOUNDED per-cycle
    // growth", not "byte-for-byte identical", which a REAL leak (growing
    // linearly with cycle count) would blow through by a wide margin at
    // only 5 cycles.
    let tolerance_bytes: isize = 8 * 1024 * 1024; // 8 MiB
    assert!(
        delta.abs() <= tolerance_bytes,
        "live heap bytes grew by {delta} across {CYCLES} spinup->teardown cycles (samples: \
         {samples:?}) — exceeds the {tolerance_bytes}-byte tolerance, suggesting a real per-cycle \
         leak rather than allocator noise"
    );
}
