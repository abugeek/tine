//! `gpu-stress <seconds> [--report FILE]`: keeps the GPU busy with an ALU-heavy
//! compute shader (one ~10-20 ms batch in flight at a time, like a game frame)
//! and prints/writes throughput: batches per second (a GPU "FPS").

use std::time::{Duration, Instant};

const SHADER: &str = r"
@group(0) @binding(0) var<storage, read_write> out: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    var x = f32(id.x) * 0.0001;
    var y = 1.0;
    for (var i = 0u; i < 16384u; i = i + 1u) {
        x = fma(x, 1.000001, 0.5) * 0.999;
        y = fma(y, x, -0.25) * 0.5;
    }
    out[id.x % 65536u] = x + y;
}
";

fn main() {
    let mut args = std::env::args().skip(1);
    let seconds: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30.0);
    let mut report = None;
    while let Some(a) = args.next() {
        if a == "--report" {
            report = args.next();
        }
    }

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .expect("no GPU adapter");
    let info = adapter.get_info();
    eprintln!("gpu-stress: {} ({:?})", info.name, info.backend);
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("no device");

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("stress"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("stress"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("out"),
        size: 65536 * 4,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buf.as_entire_binding(),
        }],
    });

    let groups: u32 = 4096; // 1M invocations x 16384 iterations per batch
    let start = Instant::now();
    let deadline = Duration::from_secs_f64(seconds);
    let mut batches: u64 = 0;
    let mut worst = Duration::ZERO;
    while start.elapsed() < deadline {
        let t = Instant::now();
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(groups, 1, 1);
        }
        queue.submit([enc.finish()]);
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        worst = worst.max(t.elapsed());
        batches += 1;
    }
    let el = start.elapsed().as_secs_f64();
    let json = format!(
        "{{\"adapter\":\"{}\",\"seconds\":{el:.3},\"batches\":{batches},\"batches_per_s\":{:.3},\"worst_batch_ms\":{:.3}}}",
        info.name.replace('"', "'"),
        batches as f64 / el,
        worst.as_secs_f64() * 1e3
    );
    println!("{json}");
    if let Some(p) = report {
        let _ = std::fs::write(p, json);
    }
}
