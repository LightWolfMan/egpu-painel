//! Linha do tempo persistente: eventos do Windows (Event Log), PnP ao vivo
//! (CM_Register_Notification) e o LastTDR do Overwatch. Tudo orientado a evento:
//! nenhum polling. Arquivo append-only em %LOCALAPPDATA%\eGPU-Painel\historico.jsonl.

use crate::util::{self, wide};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::System::EventLog::*;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    DockOn,
    DockOff,
    DockFail,
    GpuOn,
    GpuOff,
    UsbAOn,
    UsbAOff,
    UsbADev,
    PcieErr,
    PcieInfo,
    Whea,
    Tdr,
    TdrGame,
    GpuDriver,
    AcOn,
    AcOff,
    Sleep,
    Wake,
    StandbyIn,
    StandbyOut,
    Boot,
    Shutdown,
    Crash,
    Bench,
    Note,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cat {
    Dock,
    Gpu,
    Pcie,
    Tdr,
    Energia,
    UsbA,
    Sistema,
    App,
}

impl Kind {
    pub fn label(self) -> &'static str {
        use Kind::*;
        match self {
            DockOn => "Dock conectado",
            DockOff => "Dock desconectado",
            DockFail => "O dock caiu",
            GpuOn => "Placa de vídeo pronta",
            GpuOff => "Placa de vídeo sumiu",
            UsbAOn => "Porta USB-A pronta",
            UsbAOff => "Porta USB-A sumiu",
            UsbADev => "Algo conectado na USB-A",
            PcieErr => "Erro na conexão PCIe",
            PcieInfo => "Aviso do PCIe",
            Whea => "Erro de hardware",
            Tdr => "Driver de vídeo travou",
            TdrGame => "Jogo avisou travamento do vídeo",
            GpuDriver => "Aviso do driver de vídeo",
            AcOn => "Notebook passou a carregar",
            AcOff => "Notebook parou de carregar",
            Sleep => "Notebook suspenso",
            Wake => "Notebook voltou da suspensão",
            StandbyIn => "Notebook dormiu",
            StandbyOut => "Notebook acordou",
            Boot => "Windows ligou",
            Shutdown => "Windows desligou",
            Crash => "Desligamento inesperado",
            Bench => "Teste de banda",
            Note => "Aviso do painel",
        }
    }
    pub fn cat(self) -> Cat {
        use Kind::*;
        match self {
            DockOn | DockOff | DockFail => Cat::Dock,
            GpuOn | GpuOff | GpuDriver => Cat::Gpu,
            UsbAOn | UsbAOff | UsbADev => Cat::UsbA,
            PcieErr | PcieInfo | Whea => Cat::Pcie,
            Tdr | TdrGame => Cat::Tdr,
            AcOn | AcOff => Cat::Energia,
            Sleep | Wake | StandbyIn | StandbyOut | Boot | Shutdown | Crash => Cat::Sistema,
            Bench | Note => Cat::App,
        }
    }
    /// 0 = info, 1 = atenção, 2 = falha
    pub fn severity(self) -> u8 {
        use Kind::*;
        match self {
            DockFail | GpuOff | Tdr | TdrGame | Whea | PcieErr | Crash => 2,
            DockOff | UsbAOff | AcOff | GpuDriver | PcieInfo => 1,
            _ => 0,
        }
    }
    /// Eventos que merecem o painel "o que veio antes".
    pub fn is_drop(self) -> bool {
        matches!(self, Kind::DockOff | Kind::DockFail | Kind::GpuOff | Kind::Tdr | Kind::TdrGame | Kind::Crash)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Ev {
    /// Unix ms (UTC)
    pub ts: i64,
    pub kind: Kind,
    /// Fonte: canal do Event Log + ID, "PnP ao vivo", "Overwatch", ...
    pub src: String,
    pub msg: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev: Option<String>,
    /// Chave de deduplicação (canal#record, ow#ts, ...)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Estado capturado no instante (só eventos ao vivo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snap: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct State {
    /// Início da última varredura do Event Log (unix ms).
    scan_ms: i64,
    /// Dispositivos já vistos atrás da USB-A do dock: id -> nome
    usb_a: HashMap<String, String>,
}

pub struct History {
    inner: Mutex<Inner>,
}

/// No processo do painel (`--painel`) o histórico é só leitura: a bandeja é a única que assina
/// o Event Log/PnP e grava. O painel acompanha as linhas novas do arquivo (`tail`) e, quando gera
/// algo próprio (teste de banda, aviso), só acrescenta a linha; ela volta pelo `tail`.
static PANEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LAST_TAIL: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

pub fn set_panel_mode() {
    PANEL.store(true, std::sync::atomic::Ordering::SeqCst);
}
pub fn is_panel() -> bool {
    PANEL.load(std::sync::atomic::Ordering::SeqCst)
}

struct Inner {
    events: Vec<Ev>,
    keys: HashSet<String>,
    file: Option<std::fs::File>,
    state: State,
    /// Bytes do historico.jsonl já lidos (até a última linha completa).
    offset: u64,
}

static HIST: OnceLock<History> = OnceLock::new();

pub fn get() -> &'static History {
    HIST.get_or_init(|| {
        let dir = util::data_dir();
        let mut events = vec![];
        let mut keys = HashSet::new();
        let mut offset = 0u64;
        if let Ok(text) = std::fs::read_to_string(dir.join("historico.jsonl")) {
            offset = text.rfind('\n').map(|i| i + 1).unwrap_or(0) as u64;
            for line in text.lines() {
                if let Ok(ev) = serde_json::from_str::<Ev>(line) {
                    if let Some(k) = &ev.key {
                        keys.insert(k.clone());
                    }
                    events.push(ev);
                }
            }
        }
        events.sort_by_key(|e| e.ts);
        let state = std::fs::read_to_string(dir.join("estado.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("historico.jsonl"))
            .ok();
        History { inner: Mutex::new(Inner { events, keys, file, state, offset }) }
    })
}

impl History {
    /// Acrescenta um evento (deduplicado pela chave). Retorna true se era novo.
    pub fn push(&self, ev: Ev) -> bool {
        let mut g = self.inner.lock().unwrap();
        let panel = is_panel();
        if let Some(k) = &ev.key {
            let dup = if panel { g.keys.contains(k) } else { !g.keys.insert(k.clone()) };
            if dup {
                return false;
            }
        }
        if let Some(f) = g.file.as_mut() {
            if let Ok(mut line) = serde_json::to_string(&ev) {
                // uma única escrita por linha: dois processos acrescentam ao mesmo arquivo
                line.push('\n');
                let _ = f.write_all(line.as_bytes());
                let _ = f.flush();
            }
        }
        if panel {
            drop(g);
            LAST_TAIL.store(0, std::sync::atomic::Ordering::SeqCst);
            return true;
        }
        // mantém ordenado (quase sempre é o último)
        let pos = g.events.partition_point(|e| e.ts <= ev.ts);
        g.events.insert(pos, ev.clone());
        drop(g);
        crate::on_new_event(&ev);
        true
    }

    /// Painel: lê as linhas que a bandeja (ou o próprio painel) acrescentou. No máximo 1×/s.
    pub fn tail(&self) {
        use std::io::{Read, Seek, SeekFrom};
        use std::sync::atomic::Ordering;
        let now = util::now_ms();
        if now - LAST_TAIL.load(Ordering::SeqCst) < 1000 {
            return;
        }
        LAST_TAIL.store(now, Ordering::SeqCst);
        let path = util::data_dir().join("historico.jsonl");
        let Ok(len) = std::fs::metadata(&path).map(|m| m.len()) else { return };
        let mut fresh = vec![];
        {
            let mut g = self.inner.lock().unwrap();
            if len <= g.offset {
                return;
            }
            let Ok(mut f) = std::fs::File::open(&path) else { return };
            if f.seek(SeekFrom::Start(g.offset)).is_err() {
                return;
            }
            let mut buf = vec![];
            if f.take(len - g.offset).read_to_end(&mut buf).is_err() {
                return;
            }
            let Some(end) = buf.iter().rposition(|b| *b == b'\n') else { return };
            g.offset += end as u64 + 1;
            for line in String::from_utf8_lossy(&buf[..end]).lines() {
                let Ok(ev) = serde_json::from_str::<Ev>(line) else { continue };
                if let Some(k) = &ev.key {
                    if !g.keys.insert(k.clone()) {
                        continue;
                    }
                }
                let pos = g.events.partition_point(|e| e.ts <= ev.ts);
                g.events.insert(pos, ev.clone());
                fresh.push(ev);
            }
        }
        for ev in &fresh {
            crate::on_new_event(ev);
        }
    }

    pub fn snapshot(&self) -> Vec<Ev> {
        self.inner.lock().unwrap().events.clone()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().events.len()
    }

    /// Eventos nos `window_ms` anteriores a `ts` (o que veio antes da queda).
    pub fn before(&self, ts: i64, window_ms: i64) -> Vec<Ev> {
        let g = self.inner.lock().unwrap();
        g.events.iter().filter(|e| e.ts < ts && e.ts >= ts - window_ms).cloned().collect()
    }

    pub fn remember_usb_a(&self, devs: &[(String, String)]) {
        let mut g = self.inner.lock().unwrap();
        for (id, name) in devs {
            g.state.usb_a.insert(id.to_uppercase(), name.clone());
        }
        save_state(&g.state);
    }

    fn usb_a_name(&self, id: &str) -> Option<String> {
        self.inner.lock().unwrap().state.usb_a.get(&id.to_uppercase()).cloned()
    }

    fn scan_ms(&self) -> i64 {
        self.inner.lock().unwrap().state.scan_ms
    }

    fn set_scan_ms(&self, ms: i64) {
        let mut g = self.inner.lock().unwrap();
        g.state.scan_ms = ms;
        save_state(&g.state);
    }
}

fn save_state(s: &State) {
    if is_panel() {
        return;
    }
    if let Ok(j) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(util::data_dir().join("estado.json"), j);
    }
}

// ---------------------------------------------------------------------------
// Classificação de dispositivos

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum DevRole {
    DockRouter,
    DockSwitch,
    Gpu,
    UsbA,
    Other,
}

pub fn dev_role(id: &str) -> DevRole {
    let u = id.to_uppercase();
    if u.starts_with("USB4\\") && u.contains("PID_15EF") {
        DevRole::DockRouter
    } else if u.starts_with("PCI\\VEN_8086&DEV_15EF") && u.contains("\\U") {
        DevRole::DockSwitch
    } else if u.starts_with("PCI\\VEN_1002&DEV_7480") {
        DevRole::Gpu
    } else if u.starts_with("PCI\\VEN_8086&DEV_15F0") {
        DevRole::UsbA
    } else {
        DevRole::Other
    }
}

fn role_text(r: DevRole) -> &'static str {
    match r {
        DevRole::DockRouter => "roteador Thunderbolt do dock",
        DevRole::DockSwitch => "switch PCIe do dock",
        DevRole::Gpu => "Radeon RX 7600 XT",
        DevRole::UsbA => "controlador USB-A do dock",
        DevRole::Other => "dispositivo",
    }
}

// ---------------------------------------------------------------------------
// Event Log

const XPATH_SYSTEM_POWER: &str = "Provider[@Name='Microsoft-Windows-Kernel-Power'] and (EventID=105 or EventID=42 or EventID=107 or EventID=506 or EventID=507 or EventID=41)";
const XPATH_SYSTEM_GPU: &str = "(Provider[@Name='Display'] and EventID=4101) or ((Provider[@Name='amdkmdag'] or Provider[@Name='amdwddmg'] or Provider[@Name='amduw23g']) and (Level=1 or Level=2 or Level=3))";
const XPATH_SYSTEM_MISC: &str = "Provider[@Name='Microsoft-Windows-WHEA-Logger'] or (Provider[@Name='Microsoft-Windows-Kernel-General'] and (EventID=12 or EventID=13)) or (Provider[@Name='EventLog'] and EventID=6008)";

fn build_query(since_ms: Option<i64>) -> String {
    let t = since_ms
        .map(|ms| {
            let dt = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms).unwrap_or_default();
            format!(" and TimeCreated[@SystemTime&gt;='{}']", dt.format("%Y-%m-%dT%H:%M:%S%.3fZ"))
        })
        .unwrap_or_default();
    format!(
        "<QueryList>\
<Query Id='0' Path='System'>\
<Select Path='System'>*[System[({XPATH_SYSTEM_POWER}){t}]]</Select>\
<Select Path='System'>*[System[({XPATH_SYSTEM_GPU}){t}]]</Select>\
<Select Path='System'>*[System[({XPATH_SYSTEM_MISC}){t}]]</Select>\
</Query>\
<Query Id='1' Path='Microsoft-Windows-Kernel-PnP/Device Management'>\
<Select Path='Microsoft-Windows-Kernel-PnP/Device Management'>*[System[(EventID=1010 or EventID=1011){t}]]</Select>\
</Query>\
<Query Id='2' Path='Microsoft-Windows-PCI/Operational'>\
<Select Path='Microsoft-Windows-PCI/Operational'>*[System[(Level=1 or Level=2 or Level=3 or EventID=2){t}]]</Select>\
</Query>\
<Query Id='3' Path='Microsoft-Windows-Kernel-WHEA/Errors'>\
<Select Path='Microsoft-Windows-Kernel-WHEA/Errors'>*[System[(Level&gt;=0){t}]]</Select>\
</Query>\
</QueryList>"
    )
}

struct Raw {
    provider: String,
    id: u32,
    level: u32,
    channel: String,
    record: u64,
    ts: i64,
    data: Vec<(String, String)>,
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn between<'a>(s: &'a str, a: &str, b: &str) -> Option<&'a str> {
    let i = s.find(a)? + a.len();
    let j = s[i..].find(b)? + i;
    Some(&s[i..j])
}

fn parse_xml(x: &str) -> Option<Raw> {
    let provider = between(x, "<Provider Name='", "'")?.to_string();
    let id_part = between(x, "<EventID", "</EventID>")?;
    let id = id_part.rsplit('>').next()?.trim().parse().ok()?;
    let level = between(x, "<Level>", "</Level>").and_then(|s| s.parse().ok()).unwrap_or(4);
    let channel = between(x, "<Channel>", "</Channel>").unwrap_or("").to_string();
    let record = between(x, "<EventRecordID>", "</EventRecordID>").and_then(|s| s.parse().ok()).unwrap_or(0);
    let st = between(x, "<TimeCreated SystemTime='", "'")?;
    let ts = chrono::DateTime::parse_from_rfc3339(st).ok()?.timestamp_millis();
    let mut data = vec![];
    if let Some(ed) = between(x, "<EventData>", "</EventData>").or_else(|| between(x, "<UserData>", "</UserData>")) {
        let mut rest = ed;
        while let Some(i) = rest.find("<Data") {
            rest = &rest[i + 5..];
            let name = if rest.starts_with(" Name='") {
                between(rest, " Name='", "'").unwrap_or("").to_string()
            } else {
                String::new()
            };
            let Some(gt) = rest.find('>') else { break };
            if rest[..gt].ends_with('/') {
                data.push((name, String::new()));
                rest = &rest[gt + 1..];
                continue;
            }
            let body = &rest[gt + 1..];
            let Some(end) = body.find("</Data>") else { break };
            data.push((name, unescape(&body[..end])));
            rest = &body[end..];
        }
    }
    Some(Raw { provider, id, level, channel, record, ts, data })
}

fn d<'a>(r: &'a Raw, name: &str) -> &'a str {
    r.data.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str()).unwrap_or("")
}

fn compact_data(r: &Raw, max: usize) -> String {
    r.data
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .take(max)
        .map(|(n, v)| if n.is_empty() { v.clone() } else { format!("{n}={v}") })
        .collect::<Vec<_>>()
        .join(", ")
}

fn classify(r: &Raw, hist: &History) -> Option<Ev> {
    let src = format!("{} #{}", short_channel(&r.channel), r.id);
    let key = Some(format!("{}#{}", r.channel, r.record));
    let mk = |kind, msg: String, dev: Option<String>| Some(Ev { ts: r.ts, kind, src: src.clone(), msg, dev, key: key.clone(), snap: None });
    match (r.provider.as_str(), r.id) {
        ("Microsoft-Windows-Kernel-PnP", 1010 | 1011) => {
            let dev = d(r, "DeviceInstanceId").to_string();
            let count = d(r, "DeviceCount");
            let role = dev_role(&dev);
            let failing = r.id == 1011;
            let why = if failing { "removido inesperadamente: o Windows o relatou como FALHANDO" } else { "removido: ausente no barramento" };
            let extra = if count.parse::<u32>().unwrap_or(1) > 1 { format!(" ({count} dispositivos dependentes caíram junto)") } else { String::new() };
            match role {
                DevRole::DockRouter | DevRole::DockSwitch => mk(
                    if failing { Kind::DockFail } else { Kind::DockOff },
                    format!("{} {}{}", cap(role_text(role)), why, extra),
                    Some(dev),
                ),
                DevRole::Gpu => mk(Kind::GpuOff, format!("GPU {why}{extra}"), Some(dev)),
                DevRole::UsbA => mk(Kind::UsbAOff, format!("Controlador USB-A do dock {why}{extra}"), Some(dev)),
                DevRole::Other => {
                    let name = hist.usb_a_name(&dev)?;
                    mk(Kind::UsbADev, format!("{name} (porta USB-A do dock) {why}"), Some(dev))
                }
            }
        }
        ("Microsoft-Windows-Kernel-Power", 105) => {
            let on = d(r, "AcOnline") == "true";
            let cap_now: f64 = d(r, "RemainingCapacity").parse().unwrap_or(0.0);
            let full: f64 = d(r, "FullChargeCapacity").parse().unwrap_or(0.0);
            let pct = if full > 0.0 { format!(", bateria {:.0}%", cap_now / full * 100.0) } else { String::new() };
            if on {
                mk(Kind::AcOn, format!("Note voltou a receber energia externa (PD){pct}"), None)
            } else {
                mk(Kind::AcOff, format!("Note passou para a bateria: a energia externa (PD do dock) sumiu{pct}"), None)
            }
        }
        ("Microsoft-Windows-Kernel-Power", 42) => mk(Kind::Sleep, format!("Sistema entrando em suspensão ({})", compact_data(r, 2)), None),
        ("Microsoft-Windows-Kernel-Power", 107) => mk(Kind::Wake, "Sistema saiu da suspensão".into(), None),
        ("Microsoft-Windows-Kernel-Power", 506) => mk(Kind::StandbyIn, format!("Espera moderna (motivo {}, monitor externo={})", d(r, "Reason"), d(r, "ExternalMonitorConnectedState")), None),
        ("Microsoft-Windows-Kernel-Power", 507) => mk(Kind::StandbyOut, format!("Saiu da espera moderna (motivo {})", d(r, "Reason")), None),
        ("Microsoft-Windows-Kernel-Power", 41) => mk(Kind::Crash, "Reinício sem desligamento limpo (Kernel-Power 41)".into(), None),
        ("EventLog", 6008) => mk(Kind::Crash, "O desligamento anterior foi inesperado (EventLog 6008)".into(), None),
        ("Microsoft-Windows-Kernel-General", 12) => mk(Kind::Boot, "Windows iniciou".into(), None),
        ("Microsoft-Windows-Kernel-General", 13) => mk(Kind::Shutdown, "Windows desligou".into(), None),
        ("Display", 4101) => mk(Kind::Tdr, format!("O driver de vídeo parou de responder e foi recuperado ({})", compact_data(r, 2)), None),
        (p, _) if p.starts_with("amd") => mk(Kind::GpuDriver, format!("{p} #{} nível {}: {}", r.id, r.level, compact_data(r, 3)), None),
        ("Microsoft-Windows-WHEA-Logger", id) => mk(Kind::Whea, format!("WHEA-Logger {id}: {}", compact_data(r, 4)), None),
        ("Microsoft-Windows-Kernel-WHEA", id) => mk(Kind::Whea, format!("Kernel-WHEA {id}: {}", compact_data(r, 4)), None),
        ("Microsoft-Windows-PCI", 2) => {
            // "Cancel state transition" — interessa quando ocorre colado numa queda.
            let msg = r.data.iter().find(|(n, _)| n.is_empty() || n == "Message").map(|(_, v)| v.clone()).unwrap_or_else(|| compact_data(r, 3));
            mk(Kind::PcieInfo, msg, None)
        }
        ("Microsoft-Windows-PCI", id) => mk(Kind::PcieErr, format!("PCI #{id} nível {}: {}", r.level, compact_data(r, 4)), None),
        _ => None,
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn short_channel(c: &str) -> &str {
    match c {
        "Microsoft-Windows-Kernel-PnP/Device Management" => "Kernel-PnP",
        "Microsoft-Windows-PCI/Operational" => "PCI",
        "Microsoft-Windows-Kernel-WHEA/Errors" => "WHEA",
        other => other,
    }
}

unsafe fn render_xml(h: EVT_HANDLE) -> Option<String> {
    let mut used = 0u32;
    let mut count = 0u32;
    let _ = EvtRender(None, h, EvtRenderEventXml.0, 0, None, &mut used, &mut count);
    if used == 0 {
        return None;
    }
    let mut buf = vec![0u16; (used as usize).div_ceil(2) + 1];
    EvtRender(None, h, EvtRenderEventXml.0, (buf.len() * 2) as u32, Some(buf.as_mut_ptr() as _), &mut used, &mut count).ok()?;
    Some(util::from_wide(&buf))
}

fn handle_event(h: EVT_HANDLE) {
    let hist = get();
    if let Some(xml) = unsafe { render_xml(h) } {
        if let Some(raw) = parse_xml(&xml) {
            if let Some(mut ev) = classify(&raw, hist) {
                if ev.kind.is_drop() && (util::now_ms() - ev.ts) < 60_000 {
                    ev.snap = crate::collect::snapshot_text();
                }
                hist.push(ev);
            }
        }
    }
}

unsafe extern "system" fn on_evt(action: EVT_SUBSCRIBE_NOTIFY_ACTION, _ctx: *const core::ffi::c_void, ev: EVT_HANDLE) -> u32 {
    if action == EvtSubscribeActionDeliver {
        handle_event(ev);
    }
    0
}

/// Varre o histórico desde `since` e depois assina os eventos futuros.
/// Retorna quantos eventos novos entraram na varredura.
pub fn start_event_log() -> usize {
    let hist = get();
    let started = util::now_ms();
    // 1) assina eventos futuros primeiro (a deduplicação cobre a sobreposição)
    let q = wide(&build_query(None));
    let sub = unsafe {
        EvtSubscribe(
            None,
            None,
            PCWSTR::null(),
            PCWSTR(q.as_ptr()),
            None,
            None,
            Some(on_evt),
            EvtSubscribeToFutureEvents.0 | EvtSubscribeTolerateQueryErrors.0,
        )
    };
    match sub {
        Ok(h) => std::mem::forget(h), // vive até o fim do processo
        Err(e) => eprintln!("EvtSubscribe falhou: {e}"),
    }
    // 2) backfill: 30 dias na primeira vez, depois desde a última varredura
    let last = hist.scan_ms();
    let since = if last > 0 { last - 10 * 60_000 } else { started - 30 * 86_400_000 };
    let q = wide(&build_query(Some(since)));
    let mut added = 0;
    unsafe {
        if let Ok(rs) = EvtQuery(None, PCWSTR::null(), PCWSTR(q.as_ptr()), EvtQueryChannelPath.0 | 0x1000 /* TolerateQueryErrors */) {
            let mut batch = [0isize; 64];
            loop {
                let mut n = 0u32;
                if EvtNext(rs, &mut batch, 2000, 0, &mut n).is_err() || n == 0 {
                    break;
                }
                for &h in &batch[..n as usize] {
                    let h = EVT_HANDLE(h);
                    if let Some(xml) = render_xml(h) {
                        if let Some(raw) = parse_xml(&xml) {
                            if let Some(ev) = classify(&raw, hist) {
                                if hist.push(ev) {
                                    added += 1;
                                }
                            }
                        }
                    }
                    let _ = EvtClose(h);
                }
            }
            let _ = EvtClose(rs);
        }
    }
    hist.set_scan_ms(started);
    added
}

// ---------------------------------------------------------------------------
// PnP ao vivo

unsafe extern "system" fn on_pnp(
    _h: HCMNOTIFICATION,
    _ctx: *const core::ffi::c_void,
    action: CM_NOTIFY_ACTION,
    data: *const CM_NOTIFY_EVENT_DATA,
    size: u32,
) -> u32 {
    if data.is_null() || size < 8 {
        return 0;
    }
    let started = action == CM_NOTIFY_ACTION_DEVICEINSTANCESTARTED;
    let removed = action == CM_NOTIFY_ACTION_DEVICEINSTANCEREMOVED;
    if !started && !removed {
        return 0;
    }
    // InstanceId é um WCHAR[] de tamanho variável logo após o cabeçalho.
    let p = std::ptr::addr_of!((*data).u.DeviceInstance.InstanceId) as *const u16;
    let max = ((size as usize).saturating_sub(8)) / 2;
    let slice = std::slice::from_raw_parts(p, max);
    let id = util::from_wide(slice);
    pnp_event(&id, started);
    0
}

fn pnp_event(id: &str, arrived: bool) {
    let hist = get();
    let role = dev_role(id);
    let now = util::now_ms();
    let (kind, msg) = match (role, arrived) {
        (DevRole::DockRouter | DevRole::DockSwitch, true) => (Kind::DockOn, format!("{} iniciado", cap(role_text(role)))),
        (DevRole::DockRouter | DevRole::DockSwitch, false) => (Kind::DockOff, format!("{} saiu do sistema", cap(role_text(role)))),
        (DevRole::Gpu, true) => (Kind::GpuOn, "GPU iniciada pelo driver".into()),
        (DevRole::Gpu, false) => (Kind::GpuOff, "GPU saiu do sistema".into()),
        (DevRole::UsbA, true) => (Kind::UsbAOn, "Controlador USB-A do dock iniciado".into()),
        (DevRole::UsbA, false) => (Kind::UsbAOff, "Controlador USB-A do dock saiu do sistema".into()),
        (DevRole::Other, _) => {
            let up = id.to_uppercase();
            if !up.starts_with("USB\\") || up.contains("&MI_") {
                return;
            }
            if arrived {
                // está atrás do xHCI do dock?
                let Some(dn) = crate::pnp::locate(id) else { return };
                let mut cur = dn;
                let mut under = false;
                for _ in 0..8 {
                    match crate::pnp::parent(cur) {
                        Some(p) => {
                            if dev_role(&crate::pnp::device_id(p)) == DevRole::UsbA {
                                under = true;
                                break;
                            }
                            cur = p;
                        }
                        None => break,
                    }
                }
                if !under {
                    return;
                }
                let name = crate::pnp::read_node(dn).name;
                hist.remember_usb_a(&[(id.to_string(), name.clone())]);
                (Kind::UsbADev, format!("{name} conectado na USB-A do dock"))
            } else {
                let Some(name) = hist.usb_a_name(id) else { return };
                (Kind::UsbADev, format!("{name} desconectado da USB-A do dock"))
            }
        }
    };
    let snap = if !arrived { crate::collect::snapshot_text() } else { None };
    crate::collect::topology_dirty();
    hist.push(Ev {
        ts: now,
        kind,
        src: "PnP ao vivo".into(),
        msg,
        dev: Some(id.to_string()),
        key: Some(format!("pnp#{}#{}#{}", id.to_uppercase(), arrived as u8, now)),
        snap,
    });
}

pub fn start_pnp() {
    unsafe {
        let mut f: CM_NOTIFY_FILTER = std::mem::zeroed();
        f.cbSize = std::mem::size_of::<CM_NOTIFY_FILTER>() as u32;
        f.Flags = CM_NOTIFY_FILTER_FLAG_ALL_DEVICE_INSTANCES;
        f.FilterType = CM_NOTIFY_FILTER_TYPE_DEVICEINSTANCE;
        let mut h = HCMNOTIFICATION::default();
        let r = CM_Register_Notification(&f, None, Some(on_pnp), &mut h);
        if r != CR_SUCCESS {
            eprintln!("CM_Register_Notification falhou: {r:?}");
        }
        // o handle vive até o fim do processo
    }
}

/// Garante que a chegada atual do dock/GPU (DEVPKEY_Device_LastArrivalDate) esteja na linha do tempo,
/// mesmo que tenha acontecido com o painel fechado.
pub fn record_arrivals(t: &crate::pnp::Topology) {
    let hist = get();
    let mut put = |node: &Option<crate::pnp::Node>, kind: Kind, what: &str| {
        if let Some(n) = node {
            if let Some(ts) = n.last_arrival {
                hist.push(Ev {
                    ts,
                    kind,
                    src: "DEVPKEY_Device_LastArrivalDate".into(),
                    msg: format!("{what} chegou (data registrada pelo PnP)"),
                    dev: Some(n.id.clone()),
                    key: Some(format!("arr#{}#{}", n.id.to_uppercase(), ts / 1000)),
                    snap: None,
                });
            }
        }
    };
    put(&t.dock_router, Kind::DockOn, "Roteador Thunderbolt do dock");
    put(&t.gpu, Kind::GpuOn, "GPU");
    let devs: Vec<(String, String)> = t.usb_a.iter().map(|n| (n.id.clone(), n.name.clone())).collect();
    if !devs.is_empty() {
        hist.remember_usb_a(&devs);
    }
}

// ---------------------------------------------------------------------------
// Overwatch: LastTDR em Documentos\Overwatch\Settings\Settings_v0.ini

pub fn scan_overwatch() {
    let Some(docs) = util::documents_dir() else { return };
    let path = docs.join("Overwatch").join("Settings").join("Settings_v0.ini");
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("LastTDR") {
            let v = rest.trim_start_matches([' ', '=']).trim().trim_matches('"');
            if let Ok(secs) = v.parse::<i64>() {
                get().push(Ev {
                    ts: secs * 1000,
                    kind: Kind::TdrGame,
                    src: "Overwatch Settings_v0.ini".into(),
                    msg: "Overwatch gravou LastTDR: o jogo perdeu o dispositivo D3D (TDR ou GPU removida)".into(),
                    dev: None,
                    key: Some(format!("ow#{secs}")),
                    snap: None,
                });
            }
        }
    }
}

pub fn note(kind: Kind, msg: String) {
    get().push(Ev { ts: util::now_ms(), kind, src: "eGPU Painel".into(), msg, dev: None, key: None, snap: None });
}
