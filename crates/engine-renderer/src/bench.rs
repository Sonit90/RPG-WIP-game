//! Offscreen benchmark harness.
//!
//! Renders to a texture rather than a swap chain, on purpose. Milestone 7 measured
//! `present` costing three to four times as much CPU time as recording an entire frame,
//! and behaving in ways that do not track renderer cost, so a benchmark that presented
//! would mostly report the compositor's behaviour. Here there is no swap chain, no
//! present and no window.
//!
//! What is measured per frame:
//!
//! * **CPU record time** — building and submitting the command list. This is the number
//!   draw-call count moves.
//! * **GPU time** — timestamp queries around the recorded work, as in the main renderer.
//! * **Serialized frame time** — record, submit and wait for completion. Because there is
//!   no frame pipelining here, this is roughly CPU + GPU rather than the larger of the
//!   two, and any frame rate derived from it is a lower bound on what a pipelined
//!   renderer achieves. It is reported as such and never called "FPS" without that
//!   qualification.

use std::time::Instant;

use engine_core::time::{FrameStats, FrameStatsWindow, duration_to_ms};
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
    D3D12_DESCRIPTOR_HEAP_TYPE_RTV, D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES,
    D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_READBACK, D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    D3D12_RANGE, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_BUFFER,
    D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
    D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE,
    D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_TEXTURE_COPY_LOCATION, D3D12_TEXTURE_COPY_LOCATION_0,
    D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
    D3D12_TEXTURE_LAYOUT_ROW_MAJOR, ID3D12CommandAllocator, ID3D12CommandList,
    ID3D12DescriptorHeap, ID3D12GraphicsCommandList, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::core::Interface;

use crate::command::{scissor_for, transition_barrier, viewport_for};
use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::gpu_timer::GpuTimer;
use crate::instancing::{InstanceData, InstancedPipeline, UNIT_TRIANGLE, build_instance_grid};
use crate::mesh::GpuMesh;
use crate::offscreen::{BYTES_PER_PIXEL, CapturedImage};
use crate::swapchain::BACK_BUFFER_FORMAT;
use crate::sync::GpuFence;

/// How the instances are submitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawMode {
    /// One draw call per instance, selecting it with `StartInstanceLocation`.
    PerInstanceDrawCalls,
    /// A single draw call with an instance count.
    SingleInstancedDraw,
}

impl DrawMode {
    /// Short label for reports.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::PerInstanceDrawCalls => "A: one draw call per instance",
            Self::SingleInstancedDraw => "B: one instanced draw",
        }
    }

    /// Draw calls this mode issues for `instances` instances.
    #[must_use]
    pub fn draw_calls(self, instances: u32) -> u32 {
        match self {
            Self::PerInstanceDrawCalls => instances,
            Self::SingleInstancedDraw => 1,
        }
    }
}

/// One benchmark configuration's results.
#[derive(Debug, Clone, Copy)]
pub struct BenchResult {
    /// How the instances were submitted.
    pub mode: DrawMode,
    /// Instances drawn per frame.
    pub instances: u32,
    /// Draw calls issued per frame.
    pub draw_calls: u32,
    /// Triangles submitted per frame.
    pub triangles: u64,
    /// Frames measured.
    pub frames: u32,
    /// CPU time spent recording and submitting.
    pub cpu: FrameStats,
    /// GPU execution time from timestamp queries.
    pub gpu: FrameStats,
    /// Record, submit and wait — see the module documentation.
    pub serialized: FrameStats,
}

impl BenchResult {
    /// Lower-bound frame rate implied by the serialized frame time.
    #[must_use]
    pub fn serialized_fps(&self) -> f64 {
        self.serialized.mean_fps()
    }
}

/// Offscreen device, render target and pipeline for the instancing benchmark.
///
/// Field order matters: `context` owns the device and is declared last so everything it
/// created is released first.
pub struct BenchHarness {
    width: u32,
    height: u32,
    /// The colour target. `rtv` is a view of it, so it must outlive every frame.
    target: ID3D12Resource,
    /// Held for ownership: `rtv` is a handle into this heap.
    #[expect(
        dead_code,
        reason = "ownership only — the descriptor handle points into this heap"
    )]
    rtv_heap: ID3D12DescriptorHeap,
    rtv: D3D12_CPU_DESCRIPTOR_HANDLE,
    pipeline: InstancedPipeline,
    mesh: GpuMesh,
    instance_buffer: Option<GpuMesh>,
    instance_scratch: Vec<InstanceData>,
    instances: u32,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    execute_lists: [Option<ID3D12CommandList>; 1],
    fence: GpuFence,
    timer: GpuTimer,
    context: RenderDevice,
}

impl BenchHarness {
    /// Creates the offscreen device and everything the benchmark draws with.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError`] if the device, the render target or the pipeline cannot
    /// be created.
    pub fn new(width: u32, height: u32) -> Result<Self, RendererError> {
        if width == 0 || height == 0 {
            return Err(RendererError::InvalidConfig {
                reason: format!("benchmark target {width}x{height} has a zero dimension"),
            });
        }

        // The validation layer costs several times the frame time, so a benchmark must
        // not run with it on.
        let context = RenderDevice::new(false, false)?;
        let device = context.device();

        let target = create_render_target(&context, width, height)?;

        let heap_desc = D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
            NumDescriptors: 1,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
            NodeMask: 0,
        };
        // SAFETY: FFI call with an initialized descriptor that outlives the call.
        let rtv_heap: ID3D12DescriptorHeap = unsafe { device.CreateDescriptorHeap(&heap_desc) }
            .map_err(|error| RendererError::api("ID3D12Device::CreateDescriptorHeap", error))?;
        // SAFETY: FFI calls creating a view of a live resource inside our own heap.
        let rtv = unsafe {
            let handle = rtv_heap.GetCPUDescriptorHandleForHeapStart();
            device.CreateRenderTargetView(&target, None, handle);
            handle
        };

        let pipeline = InstancedPipeline::new(device)?;
        let mesh = GpuMesh::upload(&context, &UNIT_TRIANGLE)?;

        let allocator = context.create_command_allocator()?;
        let list = context.create_command_list(&allocator)?;

        let execute_lists = [Some(list.cast().map_err(|error| {
            RendererError::api("ID3D12GraphicsCommandList::cast", error)
        })?)];

        let fence = GpuFence::new(&context)?;
        let timer = GpuTimer::new(device, context.queue(), 1)?;

        Ok(Self {
            width,
            height,
            target,
            rtv_heap,
            rtv,
            pipeline,
            mesh,
            instance_buffer: None,
            instance_scratch: Vec::new(),
            instances: 0,
            allocator,
            list,
            execute_lists,
            fence,
            timer,
            context,
        })
    }

    /// Adapter the benchmark runs on.
    #[must_use]
    pub fn adapter_name(&self) -> &str {
        &self.context.adapter_info().name
    }

    /// Render-target size.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Builds and uploads `count` instances.
    ///
    /// Both draw modes then use this same buffer, so the comparison holds nothing else
    /// constant by accident.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError`] if `count` is zero or the upload fails.
    pub fn set_instances(&mut self, count: u32) -> Result<(), RendererError> {
        if count == 0 {
            return Err(RendererError::InvalidConfig {
                reason: "the benchmark needs at least one instance".to_owned(),
            });
        }
        build_instance_grid(count, &mut self.instance_scratch);
        self.instance_buffer = Some(GpuMesh::upload_raw(&self.context, &self.instance_scratch)?);
        self.instances = count;
        Ok(())
    }

    /// Runs `frames` measured frames after `warmup` unmeasured ones.
    ///
    /// The warm-up matters: the first frames pay for shader and pipeline setup inside the
    /// driver, and including them would attribute one-off costs to the mode under test.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError`] if no instances have been set, or if any D3D12 call
    /// fails.
    pub fn run(
        &mut self,
        mode: DrawMode,
        frames: u32,
        warmup: u32,
    ) -> Result<BenchResult, RendererError> {
        if self.instance_buffer.is_none() {
            return Err(RendererError::InvalidConfig {
                reason: "set_instances must be called before running the benchmark".to_owned(),
            });
        }

        for _ in 0..warmup {
            self.frame(mode)?;
        }

        let capacity = frames.max(1) as usize;
        let mut cpu = FrameStatsWindow::new(capacity);
        let mut gpu = FrameStatsWindow::new(capacity);
        let mut serialized = FrameStatsWindow::new(capacity);

        for _ in 0..frames {
            let started = Instant::now();
            let record_ms = self.frame(mode)?;
            serialized.record_ms(duration_to_ms(started.elapsed()));
            cpu.record_ms(record_ms);
            if let Some(gpu_ms) = self.timer.read_frame_ms(0)? {
                gpu.record_ms(gpu_ms);
            }
        }

        let empty = FrameStats {
            count: 0,
            mean_ms: 0.0,
            min_ms: 0.0,
            max_ms: 0.0,
            p50_ms: 0.0,
            p95_ms: 0.0,
            p99_ms: 0.0,
        };

        Ok(BenchResult {
            mode,
            instances: self.instances,
            draw_calls: mode.draw_calls(self.instances),
            triangles: u64::from(self.instances),
            frames,
            cpu: cpu.stats().unwrap_or(empty),
            gpu: gpu.stats().unwrap_or(empty),
            serialized: serialized.stats().unwrap_or(empty),
        })
    }

    /// Renders one frame in `mode` and reads the result back to the CPU.
    ///
    /// Used to prove the two modes draw the same picture, which is the premise the whole
    /// comparison rests on. Not part of the measured path.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError`] if rendering or the readback fails.
    pub fn capture(&mut self, mode: DrawMode) -> Result<CapturedImage, RendererError> {
        self.frame(mode)?;

        let device = self.context.device();
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: u64::from(self.width),
            Height: self.height,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: BACK_BUFFER_FORMAT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
            ..Default::default()
        };

        let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut total_bytes = 0u64;
        // SAFETY: FFI call describing the texture being copied; every out-parameter is a
        // valid owned value.
        unsafe {
            device.GetCopyableFootprints(
                &desc,
                0,
                1,
                0,
                Some(&mut footprint),
                None,
                None,
                Some(&mut total_bytes),
            );
        }

        let readback = create_readback_buffer(device, total_bytes)?;

        // SAFETY: the GPU is idle after `frame`, so resetting is legal.
        unsafe { self.allocator.Reset() }
            .map_err(|error| RendererError::api("ID3D12CommandAllocator::Reset", error))?;
        // SAFETY: the list is closed at this point.
        unsafe { self.list.Reset(&self.allocator, None) }
            .map_err(|error| RendererError::api("ID3D12GraphicsCommandList::Reset", error))?;

        let source = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: non-owning copy of the interface pointer, wrapped in ManuallyDrop
            // so it is never released; `self.target` outlives this structure.
            pResource: unsafe { std::mem::transmute_copy(&self.target) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: 0,
            },
        };
        let destination = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: as above; `readback` outlives this structure.
            pResource: unsafe { std::mem::transmute_copy(&readback) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: footprint,
            },
        };

        // SAFETY: FFI calls on the open list. The target is left back in RENDER_TARGET
        // state so the next frame needs no transition.
        unsafe {
            self.list.ResourceBarrier(&[transition_barrier(
                &self.target,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )]);
            self.list
                .CopyTextureRegion(&destination, 0, 0, 0, &source, None);
            self.list.ResourceBarrier(&[transition_barrier(
                &self.target,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
            )]);
        }

        // SAFETY: FFI call closing the list recorded above.
        unsafe { self.list.Close() }
            .map_err(|error| RendererError::api("ID3D12GraphicsCommandList::Close", error))?;
        // SAFETY: FFI call submitting the closed list, awaited immediately below.
        unsafe {
            self.context
                .queue()
                .ExecuteCommandLists(&self.execute_lists)
        };
        self.fence.wait_for_idle(self.context.queue())?;

        let pixels = read_back_pixels(&readback, &footprint, self.width, self.height, total_bytes)?;
        Ok(CapturedImage {
            width: self.width,
            height: self.height,
            pixels,
        })
    }

    /// Records, submits and waits for one frame. Returns the CPU record+submit time in ms.
    fn frame(&mut self, mode: DrawMode) -> Result<f64, RendererError> {
        let Some(instance_buffer) = self.instance_buffer.as_ref() else {
            return Err(RendererError::InvalidConfig {
                reason: "no instance buffer".to_owned(),
            });
        };

        let record_started = Instant::now();

        // SAFETY: the GPU is idle — every frame waits for completion before returning —
        // which is exactly what Reset requires of the allocator and the list.
        unsafe { self.allocator.Reset() }
            .map_err(|error| RendererError::api("ID3D12CommandAllocator::Reset", error))?;
        // SAFETY: as above; the list is closed at this point.
        unsafe { self.list.Reset(&self.allocator, None) }
            .map_err(|error| RendererError::api("ID3D12GraphicsCommandList::Reset", error))?;

        self.timer.begin_frame(&self.list, 0);

        // SAFETY: FFI calls recording into the open list. Every referenced object is
        // owned by this harness and outlives the submission, which is awaited below.
        unsafe {
            self.list
                .RSSetViewports(&[viewport_for(self.width, self.height)]);
            self.list
                .RSSetScissorRects(&[scissor_for(self.width, self.height)]);
            self.list
                .OMSetRenderTargets(1, Some(&self.rtv), false, None);
            self.list
                .ClearRenderTargetView(self.rtv, &[0.0, 0.0, 0.0, 1.0], None);
            self.list
                .SetGraphicsRootSignature(self.pipeline.root_signature());
            self.list.SetPipelineState(self.pipeline.pipeline_state());
            self.list
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.list
                .IASetVertexBuffers(0, Some(&[*self.mesh.view(), *instance_buffer.view()]));

            match mode {
                DrawMode::PerInstanceDrawCalls => {
                    // One draw per instance. StartInstanceLocation selects which entry of
                    // the instance stream this draw reads, so the picture is identical to
                    // mode B and only the submission differs.
                    for index in 0..self.instances {
                        self.list.DrawInstanced(3, 1, 0, index);
                    }
                }
                DrawMode::SingleInstancedDraw => {
                    self.list.DrawInstanced(3, self.instances, 0, 0);
                }
            }
        }

        self.timer.end_frame(&self.list, 0);

        // SAFETY: FFI call closing the list recorded above.
        unsafe { self.list.Close() }
            .map_err(|error| RendererError::api("ID3D12GraphicsCommandList::Close", error))?;
        // SAFETY: FFI call submitting the closed list, which outlives the call.
        unsafe {
            self.context
                .queue()
                .ExecuteCommandLists(&self.execute_lists)
        };

        let record_ms = duration_to_ms(record_started.elapsed());

        // No swap chain and one frame slot, so the CPU waits here rather than pipelining.
        self.fence.wait_for_idle(self.context.queue())?;

        Ok(record_ms)
    }
}

/// Creates a readback buffer of `bytes` bytes.
fn create_readback_buffer(
    device: &windows::Win32::Graphics::Direct3D12::ID3D12Device,
    bytes: u64,
) -> Result<ID3D12Resource, RendererError> {
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_READBACK,
        ..Default::default()
    };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };

    let mut buffer: Option<ID3D12Resource> = None;
    // SAFETY: FFI call with initialized descriptors that outlive it. COPY_DEST is the
    // required initial state for a readback buffer written by a copy.
    unsafe {
        device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &mut buffer,
        )
    }
    .map_err(|error| RendererError::api("ID3D12Device::CreateCommittedResource", error))?;

    buffer.ok_or_else(|| RendererError::InvalidConfig {
        reason: "benchmark readback buffer creation produced no resource".to_owned(),
    })
}

/// Maps the readback buffer and strips its row padding.
fn read_back_pixels(
    readback: &ID3D12Resource,
    footprint: &D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    width: u32,
    height: u32,
    total_bytes: u64,
) -> Result<Vec<u8>, RendererError> {
    let row_pitch = footprint.Footprint.RowPitch as usize;
    let row_bytes = width as usize * BYTES_PER_PIXEL;

    let mut mapped: *mut std::ffi::c_void = std::ptr::null_mut();
    let range = D3D12_RANGE {
        Begin: 0,
        End: total_bytes as usize,
    };
    // SAFETY: FFI call mapping subresource 0 of a readback buffer, declaring the whole
    // range as readable.
    unsafe { readback.Map(0, Some(&range), Some(&mut mapped)) }
        .map_err(|error| RendererError::api("ID3D12Resource::Map", error))?;

    let mut pixels = vec![0u8; row_bytes * height as usize];
    // SAFETY: the mapping covers `total_bytes`, sized by GetCopyableFootprints to hold
    // `height` rows of `row_pitch` bytes, so every copied row stays inside it.
    unsafe {
        let base = mapped.cast::<u8>();
        for row in 0..height as usize {
            std::ptr::copy_nonoverlapping(
                base.add(row * row_pitch),
                pixels.as_mut_ptr().add(row * row_bytes),
                row_bytes,
            );
        }
        readback.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
    }

    Ok(pixels)
}

/// Creates the offscreen colour target.
fn create_render_target(
    context: &RenderDevice,
    width: u32,
    height: u32,
) -> Result<ID3D12Resource, RendererError> {
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        ..Default::default()
    };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: u64::from(width),
        Height: height,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: BACK_BUFFER_FORMAT,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
        ..Default::default()
    };

    let mut target: Option<ID3D12Resource> = None;
    // SAFETY: FFI call with initialized descriptors that outlive it and a valid
    // out-parameter. The resource stays in RENDER_TARGET state for the whole benchmark,
    // so no transitions are recorded per frame.
    unsafe {
        context.device().CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            None,
            &mut target,
        )
    }
    .map_err(|error| RendererError::api("ID3D12Device::CreateCommittedResource", error))?;

    target.ok_or_else(|| RendererError::InvalidConfig {
        reason: "benchmark render target creation produced no resource".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_call_counts_match_the_mode() {
        assert_eq!(DrawMode::PerInstanceDrawCalls.draw_calls(1_000), 1_000);
        assert_eq!(DrawMode::SingleInstancedDraw.draw_calls(1_000), 1);
    }

    #[test]
    fn labels_identify_the_modes() {
        assert!(DrawMode::PerInstanceDrawCalls.label().starts_with("A:"));
        assert!(DrawMode::SingleInstancedDraw.label().starts_with("B:"));
    }
}
