//! Coleta ao vivo — só existe enquanto a janela está aberta.
//! Uma thread, 1 amostra/s: ADLX (GPU), PDH (motores Copy/3D, memória), WMI (bateria/PD),
//! e releitura barata das propriedades PnP do dock (estado D0/D3, presença).

use crate::pnp::{self, Topology};
use eframe::egui;
use serde::Deserialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const HISTORY: usize = 300; // 5 min a 1 Hz

#[derive(Clone, Copy, Default, Debug)]
pub struct GpuM {
    pub temp: Option<f64>,
    pub hot: Option<f64>,
    pub tbp: Option<f64>,
    pub clk: Option<i32>,
    pub vclk: Option<i32>,
    pub fan: Option<i32>,
    pub vram: Option<i32>,
    pub usage: Option<f64>,
    pub volt: Option<i32>,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct PowerM {
    pub online: bool,
    pub charging: bool,
    pub discharging: bool,
    pub charge_mw: i64,
    pub discharge_mw: i64,
    pub mv: i64,
    pub rem_mwh: i64,
    pub full_mwh: i64,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Sample {
    pub t: f64,
    pub gpu: Option<GpuM>,
    pub copy: f32,
    pub d3: f32,
    pub ded_mb: f64,
    pub shared_mb: f64,
    pub power: Option<PowerM>,
    pub cpu_pct: f32,
    pub ws_mb: f64,
}

#[derive(Clone, Default, Debug)]
pub struct GpuStatic {
    pub name: String,
    pub vram_total_mb: u32,
    pub bus: String,
    pub lanes: u32,
    pub luid: Option<String>,
}

#[derive(Default)]
pub struct Live {
    pub samples: VecDeque<Sample>,
    pub topo: Option<Topology>,
    pub gpu_static: Option<GpuStatic>,
    pub notes: Vec<String>,
    pub last: Option<Sample>,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);
static RUNNING: AtomicBool = AtomicBool::new(false);
static DIRTY: AtomicBool = AtomicBool::new(true);
static CTX: OnceLock<Mutex<Option<egui::Context>>> = OnceLock::new();

fn ctx_slot() -> &'static Mutex<Option<egui::Context>> {
    CTX.get_or_init(|| Mutex::new(None))
}

pub fn with_live<R>(f: impl FnOnce(&Live) -> R) -> Option<R> {
    LIVE.lock().unwrap().as_ref().map(f)
}

pub fn repaint() {
    if let Some(c) = ctx_slot().lock().unwrap().as_ref() {
        c.request_repaint();
    }
}

/// Chamado pelo callback PnP: a topologia mudou.
pub fn topology_dirty() {
    DIRTY.store(true, Ordering::SeqCst);
    repaint();
}

pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

/// Texto curto com o estado no instante (anexado a uma queda ao vivo).
pub fn snapshot_text() -> Option<String> {
    let mut parts = vec![];
    if let Some(s) = with_live(|l| l.last).flatten() {
        if let Some(g) = s.gpu {
            parts.push(format!(
                "GPU {:.0}°C (hotspot {:.0}°C), {:.0} W, {} MHz",
                g.temp.unwrap_or(0.0),
                g.hot.unwrap_or(0.0),
                g.tbp.unwrap_or(0.0),
                g.clk.unwrap_or(0)
            ));
        }
        parts.push(format!("Copy {:.0}%, 3D {:.0}%, VRAM {:.0} MB", s.copy, s.d3, s.ded_mb));
    }
    // sempre disponível, sem COM: GetSystemPowerStatus
    unsafe {
        let mut ps = windows::Win32::System::Power::SYSTEM_POWER_STATUS::default();
        if windows::Win32::System::Power::GetSystemPowerStatus(&mut ps).is_ok() {
            parts.push(format!(
                "energia externa {} , bateria {}%",
                match ps.ACLineStatus { 1 => "SIM", 0 => "NÃO", _ => "?" },
                ps.BatteryLifePercent
            ));
        }
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

pub fn start(ctx: egui::Context) -> std::thread::JoinHandle<()> {
    *ctx_slot().lock().unwrap() = Some(ctx);
    *LIVE.lock().unwrap() = Some(Live::default());
    RUNNING.store(true, Ordering::SeqCst);
    DIRTY.store(true, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("coletor".into())
        .spawn(run)
        .expect("thread coletor")
}

pub fn stop(h: std::thread::JoinHandle<()>) {
    RUNNING.store(false, Ordering::SeqCst);
    let _ = h.join();
    *LIVE.lock().unwrap() = None;
    *ctx_slot().lock().unwrap() = None;
}

fn note(msg: String) {
    if let Some(l) = LIVE.lock().unwrap().as_mut() {
        if !l.notes.contains(&msg) {
            l.notes.push(msg);
        }
    }
}

// ---------------------------------------------------------------------------

fn run() {
    let t0 = Instant::now();
    let luid = crate::bench::amd_adapter_luid();
    let mut adlx = Adlx::new().map_err(|e| note(format!("ADLX indisponível: {e}"))).ok();
    if let Some(a) = &adlx {
        let mut st = a.stat.clone();
        st.luid = luid.clone();
        if let Some(l) = LIVE.lock().unwrap().as_mut() {
            l.gpu_static = Some(st);
        }
    }
    let mut pdh = Pdh::new(luid.clone());
    if luid.is_none() {
        note("Adaptador AMD não encontrado via DXGI: contadores do motor Copy indisponíveis".into());
    }
    let wmi = wmi::WMIConnection::with_namespace_path("root\\wmi")
        .map_err(|e| note(format!("WMI root\\wmi indisponível: {e}")))
        .ok();
    let mut last_cpu = crate::util::process_usage().2;
    let mut last_t = Instant::now();
    let mut tick = 0u64;
    while RUNNING.load(Ordering::SeqCst) {
        let start = Instant::now();
        if DIRTY.swap(false, Ordering::SeqCst) || tick % 5 == 0 {
            // Releitura do PnP: barata (cfgmgr32 em memória), a cada 5 s ou quando houve evento.
            let topo = pnp::read_topology();
            if tick == 0 || DIRTY.load(Ordering::SeqCst) {
                crate::history::record_arrivals(&topo);
            }
            if let Some(l) = LIVE.lock().unwrap().as_mut() {
                l.topo = Some(topo);
            }
        }
        let mut s = Sample { t: t0.elapsed().as_secs_f64(), ..Default::default() };
        if let Some(a) = adlx.as_mut() {
            match a.read() {
                Ok(g) => s.gpu = Some(g),
                Err(_) => {
                    // GPU sumiu (dock caiu?) — reabre na próxima volta
                    adlx = None;
                }
            }
        } else if tick % 5 == 0 {
            adlx = Adlx::new().ok();
        }
        if let Some(p) = pdh.as_mut() {
            p.sample(&mut s);
        }
        if let Some(w) = &wmi {
            s.power = read_power(w);
        }
        let (ws, _, cpu) = crate::util::process_usage();
        let dt = last_t.elapsed().as_secs_f64() * 1000.0;
        s.cpu_pct = (((cpu - last_cpu) / dt) * 100.0 / num_cpus()) as f32;
        s.ws_mb = ws;
        last_cpu = cpu;
        last_t = Instant::now();
        if let Some(l) = LIVE.lock().unwrap().as_mut() {
            l.samples.push_back(s);
            while l.samples.len() > HISTORY {
                l.samples.pop_front();
            }
            l.last = Some(s);
        }
        repaint();
        tick += 1;
        // dorme em fatias curtas para fechar rápido
        let target = Duration::from_millis(1000);
        while start.elapsed() < target && RUNNING.load(Ordering::SeqCst) && !DIRTY.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn num_cpus() -> f64 {
    std::thread::available_parallelism().map(|n| n.get() as f64).unwrap_or(1.0)
}

// ---------------------------------------------------------------------------
// WMI: root\wmi BatteryStatus (é o que o driver de bateria ACPI publica)

#[derive(Deserialize, Debug)]
#[serde(rename = "BatteryStatus")]
#[serde(rename_all = "PascalCase")]
struct BatteryStatus {
    power_online: Option<bool>,
    charging: Option<bool>,
    discharging: Option<bool>,
    charge_rate: Option<i64>,
    discharge_rate: Option<i64>,
    voltage: Option<i64>,
    remaining_capacity: Option<i64>,
}

#[derive(Deserialize, Debug)]
#[serde(rename = "BatteryFullChargedCapacity")]
#[serde(rename_all = "PascalCase")]
struct BatteryFull {
    full_charged_capacity: Option<i64>,
}

fn read_power(w: &wmi::WMIConnection) -> Option<PowerM> {
    let b: Vec<BatteryStatus> = w.query().ok()?;
    let b = b.into_iter().next()?;
    let full: Vec<BatteryFull> = w.query().unwrap_or_default();
    Some(PowerM {
        online: b.power_online.unwrap_or(false),
        charging: b.charging.unwrap_or(false),
        discharging: b.discharging.unwrap_or(false),
        charge_mw: b.charge_rate.unwrap_or(0),
        discharge_mw: b.discharge_rate.unwrap_or(0),
        mv: b.voltage.unwrap_or(0),
        rem_mwh: b.remaining_capacity.unwrap_or(0),
        full_mwh: full.first().and_then(|f| f.full_charged_capacity).unwrap_or(0),
    })
}

// ---------------------------------------------------------------------------
// ADLX (crate `adlx`, bindings pré-gerados; carrega amdadlx64.dll só aqui)

struct Adlx {
    // ordem dos campos = ordem de drop: objetos antes do helper (senão o Terminate reclama)
    gpu: adlx::Gpu,
    pms: adlx::PerformanceMonitoringServices,
    _helper: adlx::AdlxHelper,
    stat: GpuStatic,
}

impl Adlx {
    fn new() -> anyhow::Result<Self> {
        use adlx::Interface;
        let helper = adlx::AdlxHelper::new()?;
        let pms = helper.system().performance_monitoring_services()?;
        let list = helper.system().gpus()?;
        let mut pick = None;
        for g in list.iter() {
            if g.is_external().unwrap_or(false) || g.pnp_string().map(|p| p.contains("VEN_1002")).unwrap_or(false) {
                pick = Some(g);
                break;
            }
        }
        drop(list);
        let gpu = pick.ok_or_else(|| anyhow::anyhow!("nenhuma GPU AMD"))?;
        let (bus, lanes) = match gpu.cast::<adlx::Gpu1>() {
            Ok(g1) => (
                match g1.pci_bus_type() {
                    Ok(3) => "PCIe".into(),
                    Ok(4) => "PCIe 2.0".into(),
                    Ok(5) => "PCIe 3.0".into(),
                    Ok(6) => "PCIe 4.0".into(),
                    Ok(x) => format!("tipo {x}"),
                    Err(_) => "?".into(),
                },
                g1.pci_bus_lane_width().unwrap_or(0),
            ),
            Err(_) => ("?".into(), 0),
        };
        let stat = GpuStatic {
            name: gpu.name().unwrap_or("?").to_string(),
            vram_total_mb: gpu.total_vram().unwrap_or(0),
            bus,
            lanes,
            luid: None,
        };
        Ok(Self { gpu, pms, _helper: helper, stat })
    }

    fn read(&mut self) -> anyhow::Result<GpuM> {
        let m = self.pms.current_gpu_metrics(&self.gpu)?;
        Ok(GpuM {
            temp: m.temperature().ok(),
            hot: m.hotspot_temperature().ok(),
            tbp: m.total_board_power().ok().or_else(|| m.power().ok()),
            clk: m.clock_speed().ok(),
            vclk: m.vram_clock_speed().ok(),
            fan: m.fan_speed().ok(),
            vram: m.vram().ok(),
            usage: m.usage().ok(),
            volt: m.voltage().ok(),
        })
    }
}

// ---------------------------------------------------------------------------
// PDH: GPU Engine (Copy = proxy do tráfego host<->GPU) e GPU Adapter Memory

use windows::core::PCWSTR;
use windows::Win32::System::Performance::*;

struct Pdh {
    q: PDH_HQUERY,
    eng: PDH_HCOUNTER,
    ded: PDH_HCOUNTER,
    shr: PDH_HCOUNTER,
    luid: String,
}

impl Drop for Pdh {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.q);
        }
    }
}

impl Pdh {
    fn new(luid: Option<String>) -> Option<Self> {
        let luid = luid?;
        unsafe {
            let mut q = PDH_HQUERY::default();
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut q) != 0 {
                return None;
            }
            let add = |path: &str| {
                let w = crate::util::wide(path);
                let mut c = PDH_HCOUNTER::default();
                let r = PdhAddEnglishCounterW(q, PCWSTR(w.as_ptr()), 0, &mut c);
                (r == 0).then_some(c)
            };
            let eng = add(&format!("\\GPU Engine(*{luid}*)\\Utilization Percentage"))?;
            let ded = add(&format!("\\GPU Adapter Memory({luid}_phys_0)\\Dedicated Usage"))?;
            let shr = add(&format!("\\GPU Adapter Memory({luid}_phys_0)\\Shared Usage"))?;
            PdhCollectQueryData(q);
            Some(Self { q, eng, ded, shr, luid })
        }
    }

    fn array(&self, c: PDH_HCOUNTER) -> Vec<(String, f64)> {
        unsafe {
            let mut size = 0u32;
            let mut count = 0u32;
            let r = PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut count, None);
            if r != PDH_MORE_DATA || size == 0 {
                return vec![];
            }
            let mut buf = vec![0u8; size as usize];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
                return vec![];
            }
            (0..count as usize)
                .map(|i| {
                    let it = &*items.add(i);
                    (it.szName.to_string().unwrap_or_default(), it.FmtValue.Anonymous.doubleValue)
                })
                .collect()
        }
    }

    fn sample(&mut self, s: &mut Sample) {
        unsafe {
            PdhCollectQueryData(self.q);
        }
        // Soma por motor (eng_N) entre processos; tipo = maior motor daquele tipo (como o Gerenciador de Tarefas).
        let mut per_eng: std::collections::HashMap<(String, String), f64> = Default::default();
        for (name, v) in self.array(self.eng) {
            if !name.contains(&self.luid) {
                continue;
            }
            let eng = name.split("_eng_").nth(1).unwrap_or("");
            let (num, ty) = eng.split_once("_engtype_").unwrap_or((eng, ""));
            *per_eng.entry((num.to_string(), ty.to_string())).or_default() += v;
        }
        let max_of = |t: &str| per_eng.iter().filter(|((_, ty), _)| ty == t).map(|(_, v)| *v).fold(0.0, f64::max);
        s.copy = max_of("Copy").min(100.0) as f32;
        s.d3 = max_of("3D").min(100.0) as f32;
        s.ded_mb = self.array(self.ded).first().map(|x| x.1).unwrap_or(0.0) / 1048576.0;
        s.shared_mb = self.array(self.shr).first().map(|x| x.1).unwrap_or(0.0) / 1048576.0;
    }
}
