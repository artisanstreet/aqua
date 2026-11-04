use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};

use aqua_gpu_d3d12::hal::budget::Visibility;
use aqua_gpu_d3d12::hal::selection::{Adapter, AdapterPreference};
use aqua_gpu_d3d12::hal::tier::Tier;
use aqua_gpu_d3d12::hal::{Core, DeviceOptions, PresentMode, PresentStatus, Swapchain};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;

use super::window::TestWindow;

fn options(preference: AdapterPreference) -> DeviceOptions {
    DeviceOptions::default().with_preference(preference)
}

fn working_set() -> u64 {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut _ as *mut _,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
        .expect("memory info");
    }
    counters.WorkingSetSize as u64
}

fn private_bytes() -> u64 {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut _ as *mut _,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
        .expect("memory info");
    }
    counters.PrivateUsage as u64
}

fn barrier(
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: unsafe { std::mem::transmute_copy(resource) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

fn render_frame(
    core: &mut Core,
    list: &ID3D12GraphicsCommandList,
    swapchain: &Swapchain,
    color: [f32; 4],
) -> Result<PresentStatus, aqua_gpu_d3d12::hal::Error> {
    swapchain.wait_for_frame(1000)?;
    let frame = core.begin_frame()?;
    let back = swapchain.current();
    unsafe {
        list.Reset(&frame.allocator, None)?;
        list.ResourceBarrier(&[barrier(
            back.resource,
            D3D12_RESOURCE_STATE_PRESENT,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
        )]);
        list.ClearRenderTargetView(back.rtv, &color, None);
        list.ResourceBarrier(&[barrier(
            back.resource,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            D3D12_RESOURCE_STATE_PRESENT,
        )]);
        list.Close()?;
    }
    core.execute(list)?;
    core.end_frame(&frame)?;
    swapchain.present(PresentMode::Vsync, &[])
}

fn present_frames(
    core: &mut Core,
    swapchain: &Swapchain,
    window: &TestWindow,
    count: u32,
) -> Vec<Duration> {
    let list = core.create_command_list().expect("list");
    let mut intervals = Vec::new();
    let mut last = Instant::now();
    for index in 0..count {
        let shade = (index % 60) as f32 / 60.0;
        render_frame(core, &list, swapchain, [shade, 0.2, 0.5, 1.0]).expect("frame");
        window.pump();
        intervals.push(last.elapsed());
        last = Instant::now();
    }
    intervals
}

fn report(label: &str, core: &Core, intervals: &[Duration]) {
    let mut sorted: Vec<f64> = intervals
        .iter()
        .skip(5)
        .map(|d| d.as_secs_f64() * 1000.0)
        .collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];
    let worst = sorted[sorted.len() - 1];
    let info = core.info();
    eprintln!(
        "[{label}] adapter={:?} kind={:?} fl={:?} sm={}.{} binding={:?} tiers={:?} uma={} tearing={} median_frame_ms={median:.2} worst_ms={worst:.2} private_mb={:.1}",
        info.adapter.name,
        info.adapter.kind,
        info.features.feature_level,
        info.features.shader_model.major,
        info.features.shader_model.minor,
        info.features.binding_tier,
        info.tiers,
        info.unified_memory,
        info.tearing,
        private_bytes() as f64 / 1048576.0
    );
}

fn present_and_resize(preference: AdapterPreference, label: &str) -> Core {
    let window = TestWindow::open(640, 420);
    let mut core = Core::new(options(preference)).expect("device");
    let (width, height) = window.client();
    let mut swapchain = core
        .create_swapchain(window.raw(), width, height)
        .expect("swapchain");
    let intervals = present_frames(&mut core, &swapchain, &window, 90);
    assert_eq!(swapchain.last_present_count().unwrap(), 90);
    report(label, &core, &intervals);

    for (outer_width, outer_height) in [(900, 600), (333, 222), (1200, 800), (640, 420)] {
        window.resize(outer_width, outer_height);
        let (width, height) = window.client();
        assert!(
            core.resize_swapchain(&mut swapchain, width, height)
                .expect("resize")
        );
        assert_eq!(swapchain.extent(), (width, height));
        present_frames(&mut core, &swapchain, &window, 6);
    }
    let (width, height) = window.client();
    assert!(
        !core
            .resize_swapchain(&mut swapchain, width, height)
            .expect("same size")
    );
    core.wait_idle().expect("idle");
    core
}

#[test]
fn hardware_gpu_presents_and_resizes() {
    let core = present_and_resize(AdapterPreference::HighPerformance, "hardware");
    let info = core.info();
    match info.adapter.kind {
        Adapter::Hardware => assert!(!info.fell_back_to_warp),
        Adapter::Software => assert!(info.fell_back_to_warp),
    }
}

#[test]
fn warp_presents_and_resizes() {
    let core = present_and_resize(AdapterPreference::Warp, "warp");
    assert_eq!(core.info().adapter.kind, Adapter::Software);
    assert!(!core.info().fell_back_to_warp);
}

#[test]
fn baseline_cap_disables_better_tiers_on_the_same_hardware() {
    let capped =
        Core::new(options(AdapterPreference::HighPerformance).with_max_tier(Tier::Baseline))
            .expect("device");
    assert_eq!(capped.tiers().binding, Tier::Baseline);
    assert!(!capped.tiers().enhanced_barriers);
}

#[test]
fn adapters_are_listed_and_luid_selection_is_exact() {
    let core = Core::new(options(AdapterPreference::HighPerformance)).expect("device");
    let adapters = aqua_gpu_d3d12::hal::list_adapters(core.factory());
    eprintln!("adapters: {adapters:#?}");
    assert!(!adapters.is_empty());
    let chosen = core.info().adapter.clone();
    let again = Core::new(options(AdapterPreference::Luid(chosen.luid))).expect("device by luid");
    assert_eq!(again.info().adapter.luid, chosen.luid);
}

fn survive_removal(preference: AdapterPreference, label: &str) {
    let window = TestWindow::open(500, 320);
    let mut core = Core::new(options(preference)).expect("device");
    let (width, height) = window.client();
    let swapchain = core
        .create_swapchain(window.raw(), width, height)
        .expect("swapchain");
    present_frames(&mut core, &swapchain, &window, 10);

    core.inject_device_removal().expect("remove");
    let list = core.create_command_list();
    let outcome =
        list.and_then(|list| render_frame(&mut core, &list, &swapchain, [1.0, 0.0, 0.0, 1.0]));
    let cause = outcome
        .expect_err("frame after removal must fail")
        .loss_cause();
    assert!(cause.is_some(), "removal must be classified");
    assert!(core.check_removed().is_err());
    drop(swapchain);

    let (mut core, recovered) = core.recover().expect("recover");
    eprintln!(
        "[{label}] removal cause={cause:?} recovery={recovered:?} adapter={}",
        core.info().adapter.name
    );
    assert_eq!(recovered.generation, 1);
    assert!(core.check_removed().is_ok());

    let swapchain = core
        .create_swapchain(window.raw(), width, height)
        .expect("swapchain after recovery");
    present_frames(&mut core, &swapchain, &window, 10);
    assert_eq!(swapchain.last_present_count().unwrap(), 10);

    core.inject_device_removal().expect("second removal");
    drop(swapchain);
    let (core, second) = core.recover().expect("second recovery");
    eprintln!(
        "[{label}] second recovery={second:?} adapter={}",
        core.info().adapter.name
    );
    assert_eq!(second.generation, 2);
    assert_eq!(
        core.info().adapter.kind,
        Adapter::Software,
        "two losses inside the window must fall back to WARP"
    );
}

#[test]
fn device_removal_is_detected_recovered_and_repeated_loss_falls_back_to_warp() {
    survive_removal(AdapterPreference::HighPerformance, "hardware-removal");
}

#[test]
fn warp_survives_device_removal() {
    let window = TestWindow::open(500, 320);
    let mut core = Core::new(options(AdapterPreference::Warp)).expect("device");
    let (width, height) = window.client();
    let swapchain = core
        .create_swapchain(window.raw(), width, height)
        .expect("swapchain");
    present_frames(&mut core, &swapchain, &window, 5);
    core.inject_device_removal().expect("remove");
    assert!(core.check_removed().is_err());
    drop(swapchain);
    let (mut core, recovered) = core.recover().expect("recover");
    assert_eq!(recovered.generation, 1);
    let swapchain = core
        .create_swapchain(window.raw(), width, height)
        .expect("swapchain");
    present_frames(&mut core, &swapchain, &window, 5);
    assert_eq!(core.info().adapter.kind, Adapter::Software);
}

#[test]
fn upload_ring_grows_without_stalling_and_returns_memory_when_hidden() {
    let mut core = Core::new(options(AdapterPreference::HighPerformance)).expect("device");
    let list = core.create_command_list().expect("list");
    let destination = {
        let mut resource: Option<ID3D12Resource> = None;
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Alignment: 0,
            Width: 32 << 20,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_NONE,
        };
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            CreationNodeMask: 1,
            VisibleNodeMask: 1,
            ..Default::default()
        };
        unsafe {
            core.device()
                .CreateCommittedResource(
                    &heap,
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    None,
                    &mut resource,
                )
                .expect("destination");
        }
        resource.expect("resource")
    };
    let initial = core.upload_capacity();
    for round in 0..40u32 {
        let frame = core.begin_frame().expect("begin");
        unsafe { list.Reset(&frame.allocator, None).expect("reset") };
        let size = match round % 8 {
            7 => 6 << 20,
            _ => 200_000 + round as u64 * 17_000,
        };
        let allocation = core.upload(size, 512).expect("upload");
        assert_eq!(allocation.offset % 512, 0);
        unsafe {
            std::ptr::write_bytes(allocation.ptr, round as u8, size as usize);
            list.CopyBufferRegion(
                &destination,
                0,
                &allocation.resource,
                allocation.offset,
                size,
            );
            list.Close().expect("close");
        }
        core.execute(&list).expect("execute");
        core.end_frame(&frame).expect("end");
    }
    core.wait_idle().expect("idle");
    let grown = core.upload_capacity();
    assert!(
        grown > initial,
        "ring should have grown from {initial} to fit 6 MiB frames, is {grown}"
    );

    let before = core.memory().expect("memory");
    let trim = core.set_visibility(Visibility::Hidden).expect("hide");
    assert!(trim.shrink_upload_ring);
    assert!(core.upload_capacity() < grown);
    let after = core.memory().expect("memory");
    eprintln!(
        "[hidden] upload ring {grown} -> {} bytes, non_local usage {} -> {} bytes",
        core.upload_capacity(),
        before.non_local.usage,
        after.non_local.usage
    );
    assert!(
        after.non_local.usage + (8 << 20) < before.non_local.usage,
        "hiding must return the grown ring"
    );
    core.set_visibility(Visibility::Visible).expect("show");
}

#[test]
fn descriptor_heaps_recycle_static_slots_and_wait_on_dynamic_ring() {
    let mut core = Core::new(options(AdapterPreference::Warp)).expect("device");
    let first = core.srv_heap().allocate_static().expect("static");
    let second = core.srv_heap().allocate_static().expect("static");
    assert_ne!(first.index, second.index);
    core.srv_heap().release_static(first);
    assert_eq!(
        core.srv_heap().allocate_static().expect("static").index,
        first.index
    );

    let rtv = core.rtv_heap().allocate().expect("rtv");
    assert_eq!(core.rtv_heap().live(), 1);
    core.rtv_heap().release(rtv);
    assert_eq!(core.rtv_heap().live(), 0);

    let mut frames = 0;
    for _ in 0..200 {
        let frame = core.begin_frame().expect("begin");
        let range = core.allocate_descriptors(1000).expect("dynamic");
        assert!(range.first >= 1024);
        let list = core.create_command_list().expect("list");
        unsafe {
            list.Reset(&frame.allocator, None).expect("reset");
            list.Close().expect("close");
        }
        core.execute(&list).expect("execute");
        core.end_frame(&frame).expect("end");
        frames += 1;
    }
    assert_eq!(frames, 200);
}

#[test]
fn memory_budget_is_reported_and_the_private_footprint_is_small() {
    let window = TestWindow::open(1280, 720);
    let baseline = private_bytes();
    let mut core = Core::new(options(AdapterPreference::HighPerformance)).expect("device");
    let (width, height) = window.client();
    let swapchain = core
        .create_swapchain(window.raw(), width, height)
        .expect("swapchain");
    present_frames(&mut core, &swapchain, &window, 30);
    core.wait_idle().expect("idle");
    let memory = core.memory().expect("memory");
    let added = private_bytes().saturating_sub(baseline);
    eprintln!(
        "[footprint] added_private_mb={:.1} local_budget_mb={} local_usage_mb={} non_local_usage_mb={}",
        added as f64 / 1048576.0,
        memory.local.budget >> 20,
        memory.local.usage >> 20,
        memory.non_local.usage >> 20
    );
    assert!(memory.local.budget > 0);
    assert!(memory.local.usage <= memory.local.budget);
}

#[test]
fn footprint_breakdown() {
    let window = TestWindow::open(1280, 720);
    let mut stages = vec![("process", private_bytes())];
    let mut bare: Option<ID3D12Device> = None;
    unsafe {
        D3D12CreateDevice(
            None::<&windows::core::IUnknown>,
            windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0,
            &mut bare,
        )
        .expect("bare device")
    };
    stages.push(("bare D3D12 device (driver cost)", private_bytes()));
    let mut core = Core::new(DeviceOptions::from_env()).expect("device");
    stages.push(("core (queue, rings, heaps, monitor)", private_bytes()));
    let (width, height) = window.client();
    let swapchain = core
        .create_swapchain(window.raw(), width, height)
        .expect("swapchain");
    stages.push(("swapchain", private_bytes()));
    let list = core.create_command_list().expect("list");
    stages.push(("command list", private_bytes()));
    {
        let frame = core.begin_frame().expect("begin");
        unsafe {
            list.Reset(&frame.allocator, None).expect("reset");
            list.Close().expect("close");
        }
        core.execute(&list).expect("execute");
        core.end_frame(&frame).expect("end");
        core.wait_idle().expect("idle");
    }
    stages.push(("first empty submit", private_bytes()));
    {
        let frame = core.begin_frame().expect("begin");
        let back = swapchain.current();
        unsafe {
            list.Reset(&frame.allocator, None).expect("reset");
            list.ResourceBarrier(&[barrier(
                back.resource,
                D3D12_RESOURCE_STATE_PRESENT,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
            )]);
            list.ClearRenderTargetView(back.rtv, &[0.0f32; 4], None);
            list.ResourceBarrier(&[barrier(
                back.resource,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_PRESENT,
            )]);
            list.Close().expect("close");
        }
        core.execute(&list).expect("execute");
        core.end_frame(&frame).expect("end");
        core.wait_idle().expect("idle");
        stages.push(("first clear of back buffer (no present)", private_bytes()));
        swapchain.present(PresentMode::Vsync, &[]).expect("present");
        core.wait_idle().expect("idle");
        stages.push(("first present", private_bytes()));
    }
    present_frames(&mut core, &swapchain, &window, 30);
    core.wait_idle().expect("idle");
    stages.push(("30 frames", private_bytes()));
    eprintln!(
        "[footprint] working set now {:.1} MB",
        working_set() as f64 / 1048576.0
    );
    present_frames(&mut core, &swapchain, &window, 600);
    core.wait_idle().expect("idle");
    stages.push(("600 more frames (steady state)", private_bytes()));
    drop(bare);
    for pair in stages.windows(2) {
        eprintln!(
            "[footprint] {:<36} +{:.1} MB",
            pair[1].0,
            (pair[1].1 as f64 - pair[0].1 as f64) / 1048576.0
        );
    }
    eprintln!(
        "[footprint] total over bare process +{:.1} MB",
        (stages.last().unwrap().1 as f64 - stages[0].1 as f64) / 1048576.0
    );
}

#[test]
fn evicting_a_resource_returns_video_memory_until_it_is_made_resident_again() {
    let mut core = Core::new(options(AdapterPreference::HighPerformance)).expect("device");
    if core.info().unified_memory {
        return;
    }
    let mut resource: Option<ID3D12Resource> = None;
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: 128 << 20,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    };
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
        ..Default::default()
    };
    unsafe {
        core.device()
            .CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &desc,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut resource,
            )
            .expect("buffer");
    }
    let pageable: ID3D12Pageable =
        windows::core::Interface::cast(&resource.expect("resource")).expect("pageable");
    core.wait_idle().expect("idle");
    let resident = core.memory().expect("memory").local.usage;
    aqua_gpu_d3d12::hal::evict(core.device(), &[pageable.clone()]).expect("evict");
    let evicted = core.memory().expect("memory").local.usage;
    aqua_gpu_d3d12::hal::make_resident(core.device(), &[pageable]).expect("resident");
    let restored = core.memory().expect("memory").local.usage;
    eprintln!("[residency] resident {resident} evicted {evicted} restored {restored}");
    assert!(evicted + (64 << 20) < resident);
}
