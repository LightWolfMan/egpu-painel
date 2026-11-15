//! Teste de banda sob demanda: cópias D3D12 na fila COPY (motor DMA) entre
//! memória do host (heaps UPLOAD/READBACK) e VRAM (heap DEFAULT) da GPU do dock.
//! Mede o que de fato atravessa o túnel PCIe do Thunderbolt. Nada fica residente:
//! o dispositivo D3D12 e os buffers são destruídos ao terminar.

use std::time::Instant;
use windows::core::Interface;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};

#[derive(Clone, Debug)]
pub struct BenchResult {
    pub adapter: String,
    pub h2d_gbs: f64,
    pub d2h_gbs: f64,
    pub h2d_runs: Vec<f64>,
    pub d2h_runs: Vec<f64>,
    pub chunk_mb: u64,
    pub per_run_mb: u64,
}

fn amd_adapter() -> Option<(IDXGIAdapter1, DXGI_ADAPTER_DESC1)> {
    unsafe {
        let f: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut i = 0;
        while let Ok(a) = f.EnumAdapters1(i) {
            i += 1;
            if let Ok(d) = a.GetDesc1() {
                if d.VendorId == 0x1002 {
                    return Some((a, d));
                }
            }
        }
        None
    }
}

/// "luid_0xHHHHHHHH_0xLLLLLLLL" como aparece nas instâncias PDH.
pub fn amd_adapter_luid() -> Option<String> {
    let (_, d) = amd_adapter()?;
    Some(format!("luid_0x{:08X}_0x{:08X}", d.AdapterLuid.HighPart as u32, d.AdapterLuid.LowPart))
}

fn buffer(dev: &ID3D12Device, heap: D3D12_HEAP_TYPE, size: u64, state: D3D12_RESOURCE_STATES) -> windows::core::Result<ID3D12Resource> {
    let hp = D3D12_HEAP_PROPERTIES { Type: heap, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: size,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut r: Option<ID3D12Resource> = None;
    unsafe { dev.CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &desc, state, None, &mut r)? };
    Ok(r.unwrap())
}

pub fn run(progress: impl Fn(&str)) -> Result<BenchResult, String> {
    const CHUNK: u64 = 128 << 20; // 128 MiB
    const REPS_PER_RUN: u32 = 8; // 1 GiB por medição
    const RUNS: usize = 4;
    let (adapter, desc) = amd_adapter().ok_or("nenhum adaptador AMD encontrado via DXGI (a GPU do dock está conectada?)")?;
    let name = crate::util::from_wide(&desc.Description);
    progress("criando dispositivo D3D12…");
    unsafe {
        let mut dev: Option<ID3D12Device> = None;
        D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut dev).map_err(|e| format!("D3D12CreateDevice: {e}"))?;
        let dev = dev.unwrap();
        let qd = D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_COPY, ..Default::default() };
        let queue: ID3D12CommandQueue = dev.CreateCommandQueue(&qd).map_err(|e| e.to_string())?;
        let alloc: ID3D12CommandAllocator = dev.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COPY).map_err(|e| e.to_string())?;
        let list: ID3D12GraphicsCommandList = dev
            .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COPY, &alloc, None)
            .map_err(|e| e.to_string())?;
        list.Close().map_err(|e| e.to_string())?;
        let fence: ID3D12Fence = dev.CreateFence(0, D3D12_FENCE_FLAG_NONE).map_err(|e| e.to_string())?;
        let ev = CreateEventW(None, false, false, None).map_err(|e| e.to_string())?;
        let mut fv = 0u64;

        progress("alocando 128 MiB no host e na VRAM…");
        let up = buffer(&dev, D3D12_HEAP_TYPE_UPLOAD, CHUNK, D3D12_RESOURCE_STATE_GENERIC_READ).map_err(|e| e.to_string())?;
        let vram = buffer(&dev, D3D12_HEAP_TYPE_DEFAULT, CHUNK, D3D12_RESOURCE_STATE_COMMON).map_err(|e| e.to_string())?;
        let rb = buffer(&dev, D3D12_HEAP_TYPE_READBACK, CHUNK, D3D12_RESOURCE_STATE_COPY_DEST).map_err(|e| e.to_string())?;
        // preenche o buffer de upload (evita páginas zeradas otimizadas)
        let mut p = std::ptr::null_mut();
        up.Map(0, None, Some(&mut p)).map_err(|e| e.to_string())?;
        let words = std::slice::from_raw_parts_mut(p as *mut u64, (CHUNK / 8) as usize);
        for (i, w) in words.iter_mut().enumerate() {
            *w = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
        up.Unmap(0, None);

        let mut measure = |dst: &ID3D12Resource, src: &ID3D12Resource| -> Result<f64, String> {
            alloc.Reset().map_err(|e| e.to_string())?;
            list.Reset(&alloc, None).map_err(|e| e.to_string())?;
            for _ in 0..REPS_PER_RUN {
                list.CopyBufferRegion(dst, 0, src, 0, CHUNK);
            }
            list.Close().map_err(|e| e.to_string())?;
            let cl: ID3D12CommandList = list.cast().map_err(|e| e.to_string())?;
            let t = Instant::now();
            queue.ExecuteCommandLists(&[Some(cl)]);
            fv += 1;
            queue.Signal(&fence, fv).map_err(|e| e.to_string())?;
            fence.SetEventOnCompletion(fv, ev).map_err(|e| e.to_string())?;
            WaitForSingleObject(ev, INFINITE);
            let secs = t.elapsed().as_secs_f64();
            Ok((CHUNK * REPS_PER_RUN as u64) as f64 / secs / 1e9)
        };

        progress("aquecendo…");
        measure(&vram, &up)?;
        measure(&rb, &vram)?;
        let mut h2d = vec![];
        let mut d2h = vec![];
        for i in 0..RUNS {
            progress(&format!("host → GPU, medição {}/{RUNS}…", i + 1));
            h2d.push(measure(&vram, &up)?);
        }
        for i in 0..RUNS {
            progress(&format!("GPU → host, medição {}/{RUNS}…", i + 1));
            d2h.push(measure(&rb, &vram)?);
        }
        let _ = CloseHandle(ev);
        let best = |v: &Vec<f64>| v.iter().cloned().fold(0.0, f64::max);
        Ok(BenchResult {
            adapter: name,
            h2d_gbs: best(&h2d),
            d2h_gbs: best(&d2h),
            h2d_runs: h2d,
            d2h_runs: d2h,
            chunk_mb: CHUNK >> 20,
            per_run_mb: (CHUNK * REPS_PER_RUN as u64) >> 20,
        })
    }
}
