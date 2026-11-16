//! Coleta ao vivo — só existe enquanto a janela está aberta.
//! Uma thread, 1 amostra/s: GPU do dock pelo driver do fabricante (ADLX na AMD, NVML na NVIDIA; na
//! Intel não há fonte e só vale o PDH), PDH (motores Copy/3D, memória), WMI (bateria/PD),
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
    /// De onde vêm os dados ao vivo: "AMD ADLX", "NVIDIA NVML" ou "DXGI + PDH".
    pub src: &'static str,
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
            parts.push(crate::tf!(
                "energia externa {} , bateria {}%",
                "external power {} , battery {}%",
                match ps.ACLineStatus { 1 => crate::i18n::t("SIM", "YES"), 0 => crate::i18n::t("NÃO", "NO"), _ => "?" },
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

/// Partes medidas pelo modo `--perfil`.
pub const PARTS: [&str; 7] = ["ADLX read", "PDH collect", "PDH parse", "WMI bateria", "process_usage", "read_topology", "topo estado"];

#[derive(Default, Clone, Copy)]
pub struct Part {
    pub wall_ms: f64,
    pub cpu_ms: f64,
    pub calls: u32,
}

/// Cronômetro opcional: tempo de parede + CPU da thread (GetThreadTimes) por parte.
#[derive(Default)]
pub struct Timer {
    pub on: bool,
    pub parts: [Part; 7],
}

fn thread_cpu_ms() -> f64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
    unsafe {
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let _ = GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u);
        let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 10_000.0;
        ft(k) + ft(u)
    }
}

impl Timer {
    fn time<R>(&mut self, part: usize, f: impl FnOnce() -> R) -> R {
        if !self.on {
            return f();
        }
        let (w0, c0) = (Instant::now(), thread_cpu_ms());
        let r = f();
        let p = &mut self.parts[part];
        p.wall_ms += w0.elapsed().as_secs_f64() * 1000.0;
        p.cpu_ms += thread_cpu_ms() - c0;
        p.calls += 1;
        r
    }
}

struct Collector {
    t0: Instant,
    gpu: Option<GpuSrc>,
    pdh: Option<Pdh>,
    wmi: Option<wmi::WMIConnection>,
    last_cpu: f64,
    last_t: Instant,
    tick: u64,
    topo: Option<Topology>,
    /// Última leitura de bateria (WMI só a cada POWER_EVERY voltas; entre elas reaproveita).
    power: Option<PowerM>,
    full_mwh: Option<i64>,
}

/// Cadências (em voltas de 1 s). A topologia completa custa ~20-50 ms de CPU: só relê sob evento PnP
/// (DIRTY) ou a cada TOPO_FULL_EVERY; entre elas só atualiza o estado barato (D0/D3, enlace PCIe, presença).
const TOPO_FULL_EVERY: u64 = 30;
const TOPO_STATE_EVERY: u64 = 5;
const POWER_EVERY: u64 = 5;

/// Atualiza só o que muda sem evento: presença, estado D0/D3 e enlace PCIe atual. Devolve `true` se algum nó
/// sumiu/mudou de presença (aí vale a leitura completa).
fn refresh_states(t: &mut Topology) -> bool {
    use windows::Win32::Devices::Properties::DEVPKEY_Device_PowerData;
    let mut changed = false;
    let mut visit = |n: &mut pnp::Node| {
        let Some(dn) = pnp::locate(&n.id) else {
            changed = true;
            return;
        };
        let (st, problem) = pnp::status(dn);
        let present = st & 0x8 != 0 || st & 0x2 != 0;
        if present != n.present || problem != n.problem {
            changed = true;
        }
        n.present = present;
        n.problem = problem;
        n.dstate = match pnp::prop(dn, &DEVPKEY_Device_PowerData) {
            Some(pnp::PVal::Bin(b)) if b.len() >= 8 => Some(u32::from_le_bytes(b[4..8].try_into().unwrap())),
            _ => None,
        };
        if let Some(p) = n.pcie.as_mut() {
            if let (Some(ns), Some(nw)) = (
                pnp::prop(dn, &pnp::PCI_CUR_SPEED).and_then(|v| v.as_u32()),
                pnp::prop(dn, &pnp::PCI_CUR_WIDTH).and_then(|v| v.as_u32()),
            ) {
                p.0 = ns;
                p.1 = nw;
            }
        }
    };
    for n in [&mut t.host_ctrl, &mut t.host_router, &mut t.dock_router, &mut t.root_port, &mut t.dock_switch, &mut t.gpu] {
        if let Some(n) = n.as_mut() {
            visit(n);
        }
    }
    for n in t.gpu_chain.iter_mut().chain(t.usb_a.iter_mut()) {
        visit(n);
    }
    for p in t.ports.iter_mut() {
        visit(&mut p.node);
        if let Some(c) = p.child.as_mut() {
            visit(c);
        }
    }
    changed
}

impl Collector {
    fn new() -> Self {
        let vd = pnp::dock_gpu_id().and_then(|id| pnp::ven_dev(&id));
        let info = crate::bench::dock_adapter_info();
        let luid = info.as_ref().map(|i| i.0.clone());
        let gpu = GpuSrc::open(vd).map_err(note).ok();
        if let Some(mut st) = gpu_static(gpu.as_ref(), info.as_ref()) {
            st.luid = luid.clone();
            if let Some(l) = LIVE.lock().unwrap().as_mut() {
                l.gpu_static = Some(st);
            }
        }
        let pdh = Pdh::new(luid.clone());
        if luid.is_none() {
            note(crate::i18n::t("Adaptador da GPU do dock não encontrado via DXGI: contadores do motor Copy indisponíveis", "Dock GPU adapter not found via DXGI: Copy engine counters unavailable").into());
        }
        let wmi = wmi::WMIConnection::with_namespace_path("root\\wmi")
            .map_err(|e| note(crate::tf!("WMI root\\wmi indisponível: {e}", "WMI root\\wmi unavailable: {e}")))
            .ok();
        Self { t0: Instant::now(), gpu, pdh, wmi, last_cpu: crate::util::process_usage().2, last_t: Instant::now(), tick: 0, topo: None, power: None, full_mwh: None }
    }

    /// Uma volta de coleta. `live`: publica em LIVE/repaint (falso no modo --perfil).
    fn step(&mut self, tm: &mut Timer, live: bool) -> Sample {
        let tick = self.tick;
        let mut full = DIRTY.swap(false, Ordering::SeqCst) || self.topo.is_none() || tick % TOPO_FULL_EVERY == 0;
        let mut publish = false;
        if !full && tick % TOPO_STATE_EVERY == 0 {
            if let Some(t) = self.topo.as_mut() {
                full = tm.time(6, || refresh_states(t));
                publish = true;
            }
        }
        if full {
            let topo = tm.time(5, pnp::read_topology);
            if live && tick == 0 {
                crate::history::record_arrivals(&topo);
            }
            self.topo = Some(topo);
            publish = true;
        }
        if publish {
            if let Some(l) = LIVE.lock().unwrap().as_mut() {
                l.topo = self.topo.clone();
            }
        }
        let mut s = Sample { t: self.t0.elapsed().as_secs_f64(), ..Default::default() };
        let vd = self.topo.as_ref().and_then(|t| t.gpu.as_ref()).and_then(|n| pnp::ven_dev(&n.id));
        if let Some(g) = self.gpu.as_mut() {
            match tm.time(0, || g.read()) {
                Ok(g) => s.gpu = Some(g),
                Err(_) => {
                    // GPU sumiu (dock caiu?) — reabre na próxima volta
                    self.gpu = None;
                }
            }
        } else if tick % 5 == 0 {
            self.gpu = GpuSrc::open(vd).ok();
        }
        if let Some(p) = self.pdh.as_mut() {
            p.sample(&mut s, tm);
            // sem fonte do fabricante (Intel, ou NVIDIA sem NVML): uso do motor 3D e VRAM do PDH; o resto fica não exposto
            if self.gpu.is_none() && vd.is_some_and(|(v, _)| v != 0x1002) {
                s.gpu = Some(GpuM { usage: Some(s.d3 as f64), vram: Some(s.ded_mb as i32), ..Default::default() });
            }
        }
        if let Some(w) = &self.wmi {
            if tick % POWER_EVERY == 0 {
                let full = self.full_mwh;
                self.power = tm.time(3, || read_power(w, full));
                if let Some(p) = &self.power {
                    // capacidade de carga plena: lida uma vez (só muda em escala de meses)
                    self.full_mwh = (p.full_mwh > 0).then_some(p.full_mwh);
                }
            }
            s.power = self.power;
        }
        let (ws, _, cpu) = tm.time(4, crate::util::process_usage);
        let dt = self.last_t.elapsed().as_secs_f64() * 1000.0;
        s.cpu_pct = (((cpu - self.last_cpu) / dt) * 100.0 / num_cpus()) as f32;
        s.ws_mb = ws;
        self.last_cpu = cpu;
        self.last_t = Instant::now();
        if live {
            if let Some(l) = LIVE.lock().unwrap().as_mut() {
                l.samples.push_back(s);
                while l.samples.len() > HISTORY {
                    l.samples.pop_front();
                }
                l.last = Some(s);
            }
            repaint();
        }
        self.tick += 1;
        s
    }
}

fn run() {
    let mut c = Collector::new();
    let mut tm = Timer::default();
    while RUNNING.load(Ordering::SeqCst) {
        let start = Instant::now();
        c.step(&mut tm, true);
        // dorme em fatias curtas para fechar rápido
        let target = Duration::from_millis(1000);
        while start.elapsed() < target && RUNNING.load(Ordering::SeqCst) && !DIRTY.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Modo `--perfil`: roda o trabalho do coletor `n` vezes (1 s entre voltas), sem UI, e imprime tempos por parte.
pub fn profile(n: u32) {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let proc_ms = || unsafe {
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let _ = GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
        let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 10_000.0;
        ft(k) + ft(u)
    };
    let mut c = Collector::new();
    let mut tm = Timer::default();
    // volta de aquecimento (primeira leitura PDH/WMI é atípica) fora da medição
    c.step(&mut tm, false);
    std::thread::sleep(Duration::from_millis(1000));
    tm.on = true;
    let (p0, th0, w0) = (proc_ms(), thread_cpu_ms(), Instant::now());
    let mut last = Sample::default();
    for _ in 0..n {
        let start = Instant::now();
        last = c.step(&mut tm, false);
        if let Some(d) = Duration::from_millis(1000).checked_sub(start.elapsed()) {
            std::thread::sleep(d);
        }
    }
    let secs = w0.elapsed().as_secs_f64();
    let (p1, th1) = (proc_ms(), thread_cpu_ms());
    println!("{n} voltas em {secs:.1} s (cadência real do coletor; aquecimento descartado)");
    println!("{:<14} {:>8} {:>12} {:>12} {:>14}", "parte", "chamadas", "parede ms/ch", "CPU ms/ch", "CPU ms por s");
    let mut tot = 0.0;
    for (i, name) in PARTS.iter().enumerate() {
        let p = tm.parts[i];
        let k = p.calls.max(1) as f64;
        tot += p.cpu_ms;
        println!("{:<14} {:>8} {:>12.2} {:>12.2} {:>14.2}", name, p.calls, p.wall_ms / k, p.cpu_ms / k, p.cpu_ms / secs);
    }
    println!("soma das partes (CPU da thread): {:.2} ms/s", tot / secs);
    println!("thread inteira (GetThreadTimes): {:.2} ms/s | processo inteiro (GetProcessTimes): {:.2} ms/s", (th1 - th0) / secs, (p1 - p0) / secs);
    println!("(GetThreadTimes tem granularidade de tick ~15,6 ms: valores por chamada sao medias estatisticas)");
    // Microbench: cada parte em lote (ms de CPU por chamada com resolução sub-tick, pois somam-se K chamadas).
    println!("
Lotes (K chamadas seguidas; CPU = GetThreadTimes do lote / K):");
    let mut lote = |name: &str, k: u32, f: &mut dyn FnMut()| {
        let (w, cpu) = (Instant::now(), thread_cpu_ms());
        for _ in 0..k {
            f();
        }
        let (w, cpu) = (w.elapsed().as_secs_f64() * 1000.0, thread_cpu_ms() - cpu);
        println!("{:<14} K={:<5} parede {:>8.3} ms/ch   CPU {:>8.3} ms/ch", name, k, w / k as f64, cpu / k as f64);
    };
    let mut sm = Sample::default();
    if let Some(a) = c.gpu.as_mut() {
        lote("ADLX read", 300, &mut || {
            let _ = a.read();
        });
    }
    if let Some(p) = c.pdh.as_mut() {
        lote("PDH collect", 100, &mut || p.bench_collect());
        lote("PDH parse", 100, &mut || p.bench_parse(&mut sm));
    }
    if let Some(w) = &c.wmi {
        lote("WMI bateria", 30, &mut || {
            let _ = read_power(w, None);
        });
    }
    lote("process_usage", 3000, &mut || {
        let _ = crate::util::process_usage();
    });
    lote("read_topology", 10, &mut || {
        let _ = pnp::read_topology();
    });
    if let Some(mut t) = c.topo.clone() {
        lote("topo estado", 100, &mut || {
            let _ = refresh_states(&mut t);
        });
    }
    println!("ultima amostra: copy {:.1}% 3d {:.1}% ded {:.0} MB shared {:.0} MB gpu {} power {}", last.copy, last.d3, last.ded_mb, last.shared_mb, last.gpu.is_some(), last.power.is_some());
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

fn read_power(w: &wmi::WMIConnection, known_full: Option<i64>) -> Option<PowerM> {
    let b: Vec<BatteryStatus> = w.query().ok()?;
    let b = b.into_iter().next()?;
    let full: Vec<BatteryFull> = if known_full.is_some() { vec![] } else { w.query().unwrap_or_default() };
    Some(PowerM {
        online: b.power_online.unwrap_or(false),
        charging: b.charging.unwrap_or(false),
        discharging: b.discharging.unwrap_or(false),
        charge_mw: b.charge_rate.unwrap_or(0),
        discharge_mw: b.discharge_rate.unwrap_or(0),
        mv: b.voltage.unwrap_or(0),
        rem_mwh: b.remaining_capacity.unwrap_or(0),
        full_mwh: known_full.or_else(|| full.first().and_then(|f| f.full_charged_capacity)).unwrap_or(0),
    })
}

// ---------------------------------------------------------------------------
// Fonte ao vivo da GPU do dock, conforme o fabricante (Vendor ID do nó PnP)

enum GpuSrc {
    Adlx(Adlx),
    Nvml(crate::nvml::Nvml),
}

impl GpuSrc {
    /// Erro = texto para as notas do painel.
    fn open(vd: Option<(u16, u16)>) -> Result<Self, String> {
        match vd {
            Some((0x1002, dev)) => Adlx::new(dev).map(Self::Adlx).map_err(|e| crate::tf!("ADLX indisponível: {e}", "ADLX unavailable: {e}")),
            Some((0x10DE, dev)) => crate::nvml::Nvml::open(0x10DE, dev).map(Self::Nvml).map_err(|e| crate::tf!("NVML indisponível: {e}", "NVML unavailable: {e}")),
            Some((v, _)) => Err(crate::tf!("GPU VEN_{v:04X}: sem fonte do fabricante; temperatura, consumo e clock não expostos (uso 3D e VRAM via PDH)", "GPU VEN_{v:04X}: no vendor source; temperature, power and clock not exposed (3D usage and VRAM via PDH)")),
            None => Err(crate::i18n::t("GPU do dock não encontrada", "dock GPU not found").into()),
        }
    }

    fn read(&mut self) -> anyhow::Result<GpuM> {
        match self {
            Self::Adlx(a) => a.read(),
            Self::Nvml(n) => n.read(),
        }
    }
}

/// Dados fixos da placa: do driver do fabricante, ou do DXGI (nome e VRAM dedicada) quando não há.
fn gpu_static(src: Option<&GpuSrc>, dxgi: Option<&(String, String, u32)>) -> Option<GpuStatic> {
    match src {
        Some(GpuSrc::Adlx(a)) => Some(a.stat.clone()),
        Some(GpuSrc::Nvml(n)) => Some(GpuStatic {
            name: if n.name.is_empty() { dxgi.map(|d| d.1.clone()).unwrap_or_default() } else { n.name.clone() },
            vram_total_mb: if n.vram_total_mb > 0 { n.vram_total_mb } else { dxgi.map(|d| d.2).unwrap_or(0) },
            src: "NVIDIA NVML",
            ..Default::default()
        }),
        None => dxgi.map(|(_, name, mb)| GpuStatic { name: name.clone(), vram_total_mb: *mb, src: "DXGI + PDH", ..Default::default() }),
    }
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
    /// `dev`: Device ID da GPU do dock (a mesma no ADLX, mesmo com uma iGPU AMD no notebook).
    fn new(dev: u16) -> anyhow::Result<Self> {
        use adlx::Interface;
        let want = format!("VEN_1002&DEV_{dev:04X}");
        let helper = adlx::AdlxHelper::new()?;
        let pms = helper.system().performance_monitoring_services()?;
        let list = helper.system().gpus()?;
        let mut pick = None;
        for g in list.iter() {
            if g.pnp_string().map(|p| p.to_uppercase().contains(&want)).unwrap_or(false) || g.is_external().unwrap_or(false) {
                pick = Some(g);
                break;
            }
        }
        drop(list);
        let gpu = pick.ok_or_else(|| anyhow::anyhow!(crate::i18n::t("nenhuma GPU AMD", "no AMD GPU")))?;
        let (bus, lanes) = match gpu.cast::<adlx::Gpu1>() {
            Ok(g1) => (
                match g1.pci_bus_type() {
                    Ok(3) => "PCIe".into(),
                    Ok(4) => "PCIe 2.0".into(),
                    Ok(5) => "PCIe 3.0".into(),
                    Ok(6) => "PCIe 4.0".into(),
                    Ok(x) => crate::tf!("tipo {x}", "type {x}"),
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
            src: "AMD ADLX",
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

    fn sample(&mut self, s: &mut Sample, tm: &mut Timer) {
        tm.time(1, || unsafe {
            PdhCollectQueryData(self.q);
        });
        tm.time(2, || self.parse(s));
    }

    fn bench_collect(&mut self) {
        unsafe {
            PdhCollectQueryData(self.q);
        }
    }

    fn bench_parse(&mut self, s: &mut Sample) {
        self.parse(s);
    }

    fn parse(&mut self, s: &mut Sample) {
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
