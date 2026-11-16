//! Feedback dentro do app: monta um relatório (seções ligáveis), limpa tudo o que identifica a pessoa
//! ou a máquina e abre uma issue no GitHub pelo navegador da própria pessoa (URL pré-preenchida com os
//! formulários de .github/ISSUE_TEMPLATE). Nada de rede aqui: o texto completo vai para a área de
//! transferência, e a URL leva o que couber (~7000 bytes).

use crate::collect::{GpuStatic, Sample};
use crate::history::{Ev, Kind};
use crate::pnp::{self, Node, PVal, PortRole, Topology, Usb4Prop};
use regex_lite::{Captures, Regex};
use std::fmt::Write as _;
use std::sync::OnceLock;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO_NEW: &str = "https://github.com/LightWolfMan/egpu-painel/issues/new";
/// Teto da URL inteira. Navegadores aceitam mais, mas o GitHub recusa pedidos muito longos (414).
pub const URL_MAX: usize = 7000;
pub const CUT_NOTE: &str = "(relatório cortado — o completo está na área de transferência, cole aqui)";
const MSG_CUT_NOTE: &str = "(mensagem cortada — a completa está na área de transferência)";
/// GUID das chaves de propriedade USB4 do Windows: é o mesmo em todo PC, não identifica ninguém.
const USB4_FMTID: &str = "5DF7E321-1C1B-4CE2-B4FA-55F4A5BC2CB6";
const NE: &str = "não exposto";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FbKind {
    Problem,
    Idea,
    OtherDock,
}

impl FbKind {
    pub const ALL: [FbKind; 3] = [FbKind::Problem, FbKind::Idea, FbKind::OtherDock];
    pub fn label(self) -> &'static str {
        match self {
            FbKind::Problem => "Problema",
            FbKind::Idea => "Ideia",
            FbKind::OtherDock => "Meu dock é outro modelo",
        }
    }
    fn prefix(self) -> &'static str {
        match self {
            FbKind::Problem => "[Problema]",
            FbKind::Idea => "[Ideia]",
            FbKind::OtherDock => "[Meu dock]",
        }
    }
}

// ---------------------------------------------------------------------------
// Coleta

/// Queda/incidente resumido (o painel agrupa os eventos; aqui só o que vai no relatório).
pub struct Inc {
    pub start: i64,
    pub end: i64,
    pub sev: u8,
    pub is_drop: bool,
    pub pd_lost: bool,
    pub kinds: Vec<Kind>,
}

/// Mesmo agrupamento do Histórico do painel (eventos a ≤ 20 s um do outro), para a linha de comando.
pub fn group_incidents(events: &[Ev]) -> Vec<Inc> {
    let mut v: Vec<Inc> = vec![];
    for e in events.iter().filter(|e| e.kind.cat() != crate::history::Cat::App) {
        match v.last_mut() {
            Some(i) if e.ts - i.end <= 20_000 => {
                i.end = e.ts;
                i.sev = i.sev.max(e.kind.severity());
                i.is_drop |= e.kind.is_drop();
                i.pd_lost |= e.kind == Kind::AcOff;
                i.kinds.push(e.kind);
            }
            _ => v.push(Inc { start: e.ts, end: e.ts, sev: e.kind.severity(), is_drop: e.kind.is_drop(), pd_lost: e.kind == Kind::AcOff, kinds: vec![e.kind] }),
        }
    }
    v
}

/// Dicas para pré-preencher o formulário "Meu dock" (só dado lido, nunca inventado).
#[derive(Clone, Default)]
pub struct Hints {
    pub dock: Option<String>,
    pub gpu: Option<String>,
    pub notebook: Option<String>,
}

pub const SECTIONS: [&str; 4] = ["Sistema", "Dock e topologia", "Placa de vídeo", "Quedas recentes"];

#[derive(Clone, Default)]
pub struct Report {
    pub sections: [String; 4],
    pub hints: Hints,
}

fn reg_str(sub: &str, name: &str) -> Option<String> {
    use windows::Win32::System::Registry::*;
    use windows::core::PCWSTR;
    let (k, v) = (crate::util::wide(sub), crate::util::wide(name));
    let mut buf = [0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, PCWSTR(k.as_ptr()), PCWSTR(v.as_ptr()), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut size)) };
    if r.is_err() {
        return None;
    }
    let s = crate::util::from_wide(&buf).split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
}

fn reg_dword(sub: &str, name: &str) -> Option<u32> {
    use windows::Win32::System::Registry::*;
    use windows::core::PCWSTR;
    let (k, v) = (crate::util::wide(sub), crate::util::wide(name));
    let (mut x, mut size) = (0u32, 4u32);
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, PCWSTR(k.as_ptr()), PCWSTR(v.as_ptr()), RRF_RT_REG_DWORD, None, Some((&mut x as *mut u32).cast()), Some(&mut size)) };
    r.is_ok().then_some(x)
}

/// Fabricante e modelo do notebook (BIOS/SMBIOS). Não é serial: é o nome do produto.
fn notebook() -> Option<String> {
    const B: &str = r"HARDWARE\DESCRIPTION\System\BIOS";
    let m = reg_str(B, "SystemManufacturer");
    let p = reg_str(B, "SystemProductName");
    match (m, p) {
        (Some(m), Some(p)) => Some(format!("{m} {p}")),
        (a, b) => a.or(b),
    }
}

fn sec_system(nb: Option<&str>) -> String {
    const NT: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build = reg_str(NT, "CurrentBuild");
    let nbuild = build.as_deref().and_then(|b| b.parse::<u32>().ok()).unwrap_or(0);
    // o ProductName do Windows 11 ainda diz "Windows 10"
    let mut win = reg_str(NT, "ProductName").unwrap_or_else(|| "Windows".into());
    if nbuild >= 22000 {
        win = win.replace("Windows 10", "Windows 11");
    }
    if let Some(v) = reg_str(NT, "DisplayVersion").or_else(|| reg_str(NT, "ReleaseId")) {
        win += &format!(" {v}");
    }
    match (&build, reg_dword(NT, "UBR")) {
        (Some(b), Some(u)) => win += &format!(" · build {b}.{u}"),
        (Some(b), None) => win += &format!(" · build {b}"),
        _ => {}
    }
    let cpu = reg_str(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString");
    let mut s = String::new();
    let _ = writeln!(s, "eGPU Painel: {VERSION}");
    let _ = writeln!(s, "Windows: {win}");
    let _ = writeln!(s, "Notebook: {}", nb.unwrap_or(NE));
    let _ = writeln!(s, "Processador: {}", cpu.as_deref().unwrap_or(NE));
    s
}

fn dstate(n: &Node) -> String {
    n.dstate.map(|d| pnp::dstate_name(d).to_string()).unwrap_or_else(|| NE.into())
}

fn pcie(n: &Node) -> String {
    match n.pcie {
        Some((cs, cw, ms, mw)) => format!("{} x{cw} (máx. {} x{mw})", pnp::pcie_speed_name(cs), pnp::pcie_speed_name(ms)),
        None => NE.into(),
    }
}

fn node_line(s: &mut String, label: &str, n: &Option<Node>) {
    match n {
        Some(n) => {
            let _ = writeln!(
                s,
                "{label}: {} [{}] · firmware {} · {} · PCIe {}",
                n.name,
                n.id,
                n.firmware.as_deref().unwrap_or(NE),
                dstate(n),
                pcie(n)
            );
        }
        None => {
            let _ = writeln!(s, "{label}: não encontrado");
        }
    }
}

fn role(r: &PortRole) -> &'static str {
    match r {
        PortRole::Gpu => "placa de vídeo",
        PortRole::UsbA => "USB-A",
        PortRole::TbExtra => "Thunderbolt extra",
        PortRole::Other => "outra",
    }
}

/// Valor de uma chave USB4 para o relatório. Identificador por aparelho (GUID, bytes longos, data)
/// não vai: só o tipo e o tamanho, para saber que ele existe.
fn usb4_value(p: &Usb4Prop) -> String {
    match &p.val {
        PVal::Guid(_) => "identificador único (removido)".into(),
        PVal::Time(_) => "data (removida)".into(),
        PVal::Bin(b) if p.pid != 11 && b.len() >= 8 => format!("{} bytes (removidos: podem identificar o aparelho)", b.len()),
        _ => p.decoded.clone(),
    }
}

fn usb4_lines(s: &mut String, title: &str, props: &[Usb4Prop]) {
    if props.is_empty() {
        let _ = writeln!(s, "{title}: {NE}");
        return;
    }
    let _ = writeln!(s, "{title} (chave {{{USB4_FMTID}}}):");
    for p in props {
        let _ = writeln!(s, "  {:>2} {} [{}] {}", p.pid, p.meaning, p.confidence, usb4_value(p));
    }
}

fn sec_dock(t: &Topology) -> String {
    let mut s = String::new();
    node_line(&mut s, "Controlador host", &t.host_ctrl);
    node_line(&mut s, "Roteador raiz", &t.host_router);
    node_line(&mut s, "Roteador do dock", &t.dock_router);
    node_line(&mut s, "Porta raiz PCIe", &t.root_port);
    node_line(&mut s, "Switch do dock", &t.dock_switch);
    for p in &t.ports {
        let _ = writeln!(
            s,
            "  Porta ({}): {} · {} · filho: {}",
            role(&p.role),
            p.node.id,
            dstate(&p.node),
            p.child.as_ref().map(|c| c.name.as_str()).unwrap_or("nenhum")
        );
    }
    for n in &t.usb_a {
        let _ = writeln!(s, "  USB-A: {} [{}]", n.name, n.id);
    }
    usb4_lines(&mut s, "USB4 do dock", &t.dock_usb4);
    usb4_lines(&mut s, "USB4 do host", &t.host_usb4);
    s
}

fn opt<T: std::fmt::Display>(v: Option<T>, unit: &str) -> String {
    v.map(|x| format!("{x} {unit}").trim_end().to_string()).unwrap_or_else(|| NE.into())
}

fn sec_gpu(t: &Topology, gst: Option<&GpuStatic>, last: Option<&Sample>) -> String {
    let mut s = String::new();
    match &t.gpu {
        Some(g) => {
            let _ = writeln!(s, "Placa: {} [{}]", g.name, g.id);
            let _ = writeln!(s, "Driver: {} ({})", g.driver.as_deref().unwrap_or(NE), g.driver_provider.as_deref().unwrap_or(NE));
            let _ = writeln!(s, "Energia: {} · PCIe {}", dstate(g), pcie(g));
        }
        None => s += "Placa: não encontrada atrás do dock\n",
    }
    for n in &t.gpu_chain {
        let _ = writeln!(s, "  Cadeia PCIe: {} [{}] · {} · PCIe {}", n.name, n.id, dstate(n), pcie(n));
    }
    match gst {
        Some(g) => {
            let _ = writeln!(s, "ADLX: {} · {} MB · {} x{}", g.name, g.vram_total_mb, if g.bus.is_empty() { NE } else { &g.bus }, g.lanes);
        }
        None => s += "ADLX: sem leitura (o relatório saiu sem a coleta ao vivo)\n",
    }
    match last.and_then(|l| l.gpu.map(|g| (l, g))) {
        Some((l, g)) => {
            let f0 = |v: Option<f64>, u: &str| opt(v.map(|x| format!("{x:.0}")), u);
            let _ = writeln!(
                s,
                "Amostra mais recente: temperatura {} · hotspot {} · consumo {} · clock {} · uso {} · VRAM {} · ventoinha {} · tensão {} · motor Copy {:.0} % · 3D {:.0} %",
                f0(g.temp, "°C"),
                f0(g.hot, "°C"),
                f0(g.tbp, "W"),
                opt(g.clk, "MHz"),
                f0(g.usage, "%"),
                opt(g.vram, "MB"),
                opt(g.fan, "RPM"),
                opt(g.volt, "mV"),
                l.copy,
                l.d3
            );
        }
        None => s += "Amostra mais recente: nenhuma\n",
    }
    s
}

/// "há 3 h" grosseiro de propósito: sem data nem hora exatas.
pub fn ago(ms: i64) -> String {
    let m = ms.max(0) / 60_000;
    if m < 1 {
        "há menos de 1 min".into()
    } else if m < 60 {
        format!("há {m} min")
    } else if m < 48 * 60 {
        format!("há {} h", m / 60)
    } else {
        format!("há {} dias", m / 1440)
    }
}

fn sec_drops(incs: &[Inc], now: i64) -> String {
    const DAY: i64 = 86_400_000;
    let probs: Vec<&Inc> = incs.iter().filter(|i| i.sev >= 1 && now - i.end <= 7 * DAY).collect();
    let drops = |win: i64| probs.iter().filter(|i| i.is_drop && now - i.end <= win).count();
    let mut s = String::new();
    let _ = writeln!(s, "Quedas: {} nas últimas 24 h · {} em 7 dias", drops(DAY), drops(7 * DAY));
    let _ = writeln!(s, "Incidentes com aviso ou falha em 7 dias: {}", probs.len());
    for i in probs.iter().rev().take(15) {
        let mut kinds: Vec<&str> = vec![];
        for k in &i.kinds {
            if !kinds.contains(&k.label()) {
                kinds.push(k.label());
            }
        }
        let _ = writeln!(
            s,
            "- {} · duração {} s · {}{} · {}",
            ago(now - i.start),
            (i.end - i.start) / 1000,
            if i.is_drop { "queda" } else { "sem queda" },
            if i.pd_lost { " · notebook parou de carregar" } else { "" },
            kinds.join(", ")
        );
    }
    if probs.len() > 15 {
        let _ = writeln!(s, "(mais {} incidentes antigos omitidos)", probs.len() - 15);
    }
    s
}

pub fn build(t: &Topology, gst: Option<&GpuStatic>, last: Option<&Sample>, incs: &[Inc]) -> Report {
    let nb = notebook();
    let dock = {
        let get = |pid| t.dock_usb4.iter().find(|p| p.pid == pid).and_then(|p| p.val.as_str().map(str::to_string));
        match (get(9), get(10)) {
            (Some(a), Some(b)) => Some(format!("{a} {b}").split_whitespace().collect::<Vec<_>>().join(" ")),
            _ => t.dock_router.as_ref().map(|n| n.name.clone()),
        }
    };
    Report {
        sections: [sec_system(nb.as_deref()), sec_dock(t), sec_gpu(t, gst, last), sec_drops(incs, crate::util::now_ms())],
        hints: Hints { dock, gpu: t.gpu.as_ref().map(|g| g.name.clone()), notebook: nb },
    }
}

/// Seções marcadas, juntas e limpas: é exatamente o relatório que vai.
pub fn compose(r: &Report, on: &[bool; 4]) -> String {
    let mut out = String::new();
    for (i, body) in r.sections.iter().enumerate() {
        if on[i] {
            if !out.is_empty() {
                out.push('\n');
            }
            let _ = writeln!(out, "## {}", SECTIONS[i]);
            out += body;
        }
    }
    sanitize(&out.replace('\u{A0}', " "), r.hints.notebook.as_deref())
}

// ---------------------------------------------------------------------------
// Limpeza

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|s| !s.trim().is_empty())
}

/// Limpa com os nomes desta sessão (usuário, computador, perfil). `hw` = fabricante/modelo do notebook:
/// se o nome de usuário for uma palavra desse texto ("Dell"), ele é dado de hardware e fica no texto
/// solto (nos caminhos sai sempre).
pub fn sanitize(s: &str, hw: Option<&str>) -> String {
    sanitize_with(s, env("USERPROFILE").as_deref(), env("USERNAME").as_deref(), env("COMPUTERNAME").as_deref(), hw)
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Troca `word` (sem diferenciar caixa) quando ela é palavra inteira.
fn replace_word(t: &str, word: &str, with: &str) -> String {
    let w = word.trim();
    if w.chars().count() < 2 {
        return t.to_string();
    }
    let lt = t.to_lowercase();
    let lw = w.to_lowercase();
    // to_lowercase pode mudar o tamanho em bytes fora do ASCII; nesse caso compara por char
    if lt.len() != t.len() {
        return t.to_string().replace(w, with);
    }
    let mut out = String::with_capacity(t.len());
    let mut i = 0;
    while let Some(p) = lt[i..].find(&lw) {
        let a = i + p;
        let b = a + lw.len();
        let before = t[..a].chars().next_back();
        let after = t[b..].chars().next();
        out += &t[i..a];
        if before.is_some_and(is_word) || after.is_some_and(is_word) {
            out += &t[a..b];
        } else {
            out += with;
        }
        i = b;
    }
    out += &t[i..];
    out
}

pub fn sanitize_with(s: &str, profile: Option<&str>, user: Option<&str>, pc: Option<&str>, hw: Option<&str>) -> String {
    static PROFILES: OnceLock<Regex> = OnceLock::new();
    static MAIL: OnceLock<Regex> = OnceLock::new();
    static GUID: OnceLock<Regex> = OnceLock::new();
    static MAC: OnceLock<Regex> = OnceLock::new();
    static DEVID: OnceLock<Regex> = OnceLock::new();
    static HEXRUN: OnceLock<Regex> = OnceLock::new();
    let mut t = s.to_string();
    // qualquer perfil: X:\Users\<nome>
    t = re(&PROFILES, r#"(?i)\b([a-z]:[\\/]+users[\\/]+)[^\\/\s"'<>|:*?]+"#).replace_all(&t, "${1}<usuário>").into_owned();
    // o perfil exato desta sessão, se estiver fora de X:\Users (com \ ou / e em qualquer caixa)
    if let Some(p) = profile.map(str::trim).filter(|p| p.len() >= 3) {
        let parts: Vec<String> = p.split(['\\', '/']).filter(|x| !x.is_empty()).map(regex_lite::escape).collect();
        if parts.len() >= 2 {
            if let Ok(r) = Regex::new(&format!(r#"(?i){}[^\\/\s"'<>|:*?]*"#, parts.join(r"[\\/]+"))) {
                t = r.replace_all(&t, "<perfil do usuário>").into_owned();
            }
        }
    }
    t = re(&MAIL, r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}").replace_all(&t, "<e-mail>").into_owned();
    t = re(&GUID, r"\{?[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}\}?")
        .replace_all(&t, |c: &Captures| {
            let m = &c[0];
            if m.trim_matches(['{', '}']).eq_ignore_ascii_case(USB4_FMTID) { m.to_string() } else { "{…}".to_string() }
        })
        .into_owned();
    t = re(&MAC, r"\b[0-9A-Fa-f]{2}(?:[:-][0-9A-Fa-f]{2}){5}\b").replace_all(&t, "<MAC>").into_owned();
    // ID de instância: ENUMERADOR\hardware\instância → a instância (serial, posição no barramento) sai
    t = re(&DEVID, r#"(?i)\b(PCI|USB4|USB|HID|ACPI|SWD|BTHENUM|BTHLE|DISPLAY|HDAUDIO|SCSI|STORAGE|THUNDERBOLT|TBT|USBSTOR)\\([^\\\s\[\]()"',;]+)\\[^\s\[\]()"',;]+"#)
        .replace_all(&t, "${1}\\${2}\\…")
        .into_owned();
    // corrida longa de bytes em hexa ("AA BB CC ..."): costuma ser UUID ou serial
    t = re(&HEXRUN, r"\b(?:[0-9A-Fa-f]{2} ){7,}[0-9A-Fa-f]{2}\b").replace_all(&t, "<bytes removidos>").into_owned();
    // nome do PC antes do usuário: "DELL-PC" não pode virar "<usuário>-PC"
    if let Some(pc) = pc {
        t = replace_word(&t, pc, "<computador>");
    }
    if let Some(u) = user {
        let hw_word = hw.is_some_and(|h| replace_word(h, u, "\u{0}") != h);
        if !hw_word {
            t = replace_word(&t, u, "<usuário>");
        }
    }
    t
}

// ---------------------------------------------------------------------------
// URL e texto completo

/// Percent-encoding RFC 3986: só os não reservados (A-Z a-z 0-9 - . _ ~) ficam; o resto vira %XX por byte UTF-8.
pub fn pct(s: &str) -> String {
    let mut o = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            o.push(b as char);
        } else {
            let _ = write!(o, "%{b:02X}");
        }
    }
    o
}

fn title(kind: FbKind, message: &str, hints: &Hints) -> String {
    let first = message.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let mut t: String = first.chars().take(60).collect();
    if first.chars().count() > 60 {
        // corta na última palavra inteira
        if let Some(p) = t.rfind(' ') {
            t.truncate(p);
        }
        t.push('…');
    }
    if t.is_empty() {
        t = match kind {
            FbKind::OtherDock => hints.dock.clone().unwrap_or_default(),
            _ => String::new(),
        };
    }
    format!("{} {t}", kind.prefix()).trim_end().to_string()
}

/// Maior prefixo de `s` cujo percent-encoding cabe em `budget` bytes (corta numa quebra de linha se der).
fn fit(s: &str, budget: usize) -> (String, bool) {
    if pct(s).len() <= budget {
        return (s.to_string(), false);
    }
    let mut used = 0;
    let mut end = 0;
    for (i, c) in s.char_indices() {
        let mut b = [0u8; 4];
        let n = c.encode_utf8(&mut b).bytes().map(|x| if x.is_ascii_alphanumeric() || b"-._~".contains(&x) { 1 } else { 3 }).sum::<usize>();
        if used + n > budget {
            break;
        }
        used += n;
        end = i + c.len_utf8();
    }
    let cut = &s[..end];
    let cut = match cut.rfind('\n') {
        Some(p) if p > end / 2 => &cut[..p],
        _ => cut,
    };
    (cut.to_string(), true)
}

/// URL da issue nova com o formulário certo pré-preenchido. `true` = algo foi cortado para caber.
pub fn issue_url(kind: FbKind, message: &str, report: &str, hints: &Hints) -> (String, bool) {
    let message = message.trim();
    let mut q: Vec<(&str, String)> = vec![];
    // (campo da mensagem, campo do relatório)
    let (msg_field, rep_field) = match kind {
        FbKind::Problem => {
            q.push(("template", "bug.yml".into()));
            ("descricao", "relatorio")
        }
        FbKind::OtherDock => {
            q.push(("template", "meu-dock.yml".into()));
            for (k, v) in [("modelo", &hints.dock), ("placa", &hints.gpu), ("notebook", &hints.notebook)] {
                if let Some(v) = v {
                    q.push((k, v.clone()));
                }
            }
            ("o-que-acontece", "relatorio")
        }
        // sem formulário: issue em branco, tudo no corpo
        FbKind::Idea => ("body", "body"),
    };
    q.push(("title", title(kind, message, hints)));
    let join = |q: &[(&str, String)]| -> String {
        let mut u = String::from(REPO_NEW);
        for (i, (k, v)) in q.iter().enumerate() {
            u.push(if i == 0 { '?' } else { '&' });
            u += k;
            u.push('=');
            u += &pct(v);
        }
        u
    };
    let mut cut = false;
    // a mensagem tem prioridade, mas não pode comer a URL inteira
    let (mut msg, mcut) = fit(message, URL_MAX / 2);
    if mcut {
        msg = format!("{msg}\n\n{MSG_CUT_NOTE}");
        cut = true;
    }
    let report = report.trim_end();
    let wrap = |r: &str| if kind == FbKind::Idea { format!("{msg}\n\n## Relatório\n\n```text\n{r}\n```") } else { r.to_string() };
    if msg_field != rep_field {
        q.push((msg_field, msg.clone()));
    }
    let base = join(&q).len() + 1 + rep_field.len() + 1;
    let full = wrap(report);
    let rep = if base + pct(&full).len() <= URL_MAX {
        full
    } else {
        cut = true;
        let note = format!("\n\n{CUT_NOTE}");
        let overhead = pct(&wrap("")).len() + pct(&note).len();
        let (r, _) = fit(report, URL_MAX.saturating_sub(base + overhead));
        wrap(&format!("{r}{note}"))
    };
    if report.is_empty() && kind != FbKind::Idea {
        return (join(&q), cut);
    }
    q.push((rep_field, rep));
    (join(&q), cut)
}

/// Mensagem (como a pessoa escreveu) + relatório limpo: o que vai para a área de transferência.
pub fn full_text(kind: FbKind, message: &str, report: &str) -> String {
    let mut s = format!("Feedback do eGPU Painel {VERSION} · {}\n", kind.label());
    if !message.trim().is_empty() {
        s += &format!("\n{}\n", message.trim());
    }
    if !report.trim().is_empty() {
        s += "\n---\n\n";
        s += report.trim_end();
        s.push('\n');
    }
    s
}

/// Relatório completo para a linha de comando (--relatorio), sem coleta ao vivo.
pub fn cli_report() -> String {
    let t = pnp::read_topology();
    let incs = group_incidents(&crate::history::get().snapshot());
    compose(&build(&t, None, None, &incs), &[true; 4])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(s: &str) -> String {
        sanitize_with(s, Some(r"C:\Users\Fulano"), Some("Fulano"), Some("DESKTOP-AB12CD"), None)
    }

    #[test]
    fn profile_paths() {
        assert_eq!(clean(r"C:\Users\Fulano\AppData\Local\eGPU-Painel"), r"C:\Users\<usuário>\AppData\Local\eGPU-Painel");
        assert_eq!(clean(r"c:\users\FULANO\x"), r"c:\users\<usuário>\x");
        assert_eq!(clean("C:/Users/fulano/Desktop"), "C:/Users/<usuário>/Desktop");
        assert_eq!(clean(r"D:\USERS\Beltrano\Jogos"), r"D:\USERS\<usuário>\Jogos");
        assert_eq!(clean(r"salvo em C:\Users\Fulano e deu erro"), r"salvo em C:\Users\<usuário> e deu erro");
        // perfil fora de C:\Users só sai porque sabemos o valor exato
        assert_eq!(sanitize_with(r"E:\Perfis\ana\x", Some(r"E:\Perfis\ana"), None, None, None), r"<perfil do usuário>\x");
    }

    #[test]
    fn names_mail_mac() {
        assert_eq!(clean("host desktop-ab12cd."), "host <computador>.");
        assert_eq!(clean("Fulano, FULANO e fulano"), "<usuário>, <usuário> e <usuário>");
        assert_eq!(clean("Fulanos e Beltrano ficam"), "Fulanos e Beltrano ficam");
        assert_eq!(clean("escreva para fulano.silva+x@exemplo.com.br hoje"), "escreva para <e-mail> hoje");
        assert_eq!(clean("MAC 3C-A9-F4-12-AB-0E e 3c:a9:f4:12:ab:0e"), "MAC <MAC> e <MAC>");
        // usuário com nome do fabricante: no texto solto fica (é hardware), no caminho sai
        let s = sanitize_with(r"Notebook: Dell Inc. Vostro 5620 · C:\Users\Dell\x", Some(r"C:\Users\Dell"), Some("Dell"), None, Some("Dell Inc. Vostro 5620"));
        assert_eq!(s, r"Notebook: Dell Inc. Vostro 5620 · C:\Users\<usuário>\x");
        assert_eq!(sanitize_with("a casa e o jogo", None, Some("a"), Some("e"), None), "a casa e o jogo");
        let x = clean(r"C:\Users\Fulano\x DESKTOP-AB12CD Fulano");
        assert_eq!(clean(&x), x);
    }

    #[test]
    fn device_ids_and_guids() {
        let gpu = "AMD Radeon RX 7600 XT [PCI\\VEN_1002&DEV_7480&SUBSYS_76051EAE&REV_C0\\8&19DC89AD&0&000000080038] fw=None";
        assert_eq!(clean(gpu), "AMD Radeon RX 7600 XT [PCI\\VEN_1002&DEV_7480&SUBSYS_76051EAE&REV_C0\\…] fw=None");
        assert_eq!(clean(r"PCI\VEN_8086&DEV_15EF&SUBSYS_00008086&REV_06\UDAFC008A09C9A00000 · D0"), r"PCI\VEN_8086&DEV_15EF&SUBSYS_00008086&REV_06\… · D0");
        assert_eq!(clean(r"[USB4\ROOT_DEVICE_ROUTER&VID_8086&PID_A73E\4&276DC1FB&0&0]"), r"[USB4\ROOT_DEVICE_ROUTER&VID_8086&PID_A73E\…]");
        assert_eq!(clean(r"USB\VID_152D&PID_0578\MSFT30740100010214"), r"USB\VID_152D&PID_0578\…");
        // a chave USB4 do Windows fica; GUID de aparelho sai
        assert_eq!(clean("chave {5DF7E321-1C1B-4CE2-B4FA-55F4A5BC2CB6}"), "chave {5DF7E321-1C1B-4CE2-B4FA-55F4A5BC2CB6}");
        assert_eq!(clean("uuid {0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9} e 0A1B2C3D-4E5F-6071-8293-A4B5C6D7E8F9"), "uuid {…} e {…}");
        assert_eq!(clean("bytes 01 23 45 67 89 AB CD EF 10 32"), "bytes <bytes removidos>");
        // coisas curtas que não são identificadores ficam
        assert_eq!(clean("0x4D0001 · REV_0006 · 7 (0x7)"), "0x4D0001 · REV_0006 · 7 (0x7)");
        let x = clean(gpu);
        assert_eq!(clean(&x), x);
    }

    #[test]
    fn encoder() {
        assert_eq!(pct("abc-._~XYZ09"), "abc-._~XYZ09");
        assert_eq!(pct("a b&c=d?e#f/g+h%"), "a%20b%26c%3Dd%3Fe%23f%2Fg%2Bh%25");
        assert_eq!(pct("ção"), "%C3%A7%C3%A3o");
        assert_eq!(pct("—\n🎮"), "%E2%80%94%0A%F0%9F%8E%AE");
        assert_eq!(pct("[Meu dock]"), "%5BMeu%20dock%5D");
    }

    #[test]
    fn url_shape_and_cap() {
        let h = Hints { dock: Some("Intel Module 2 PD 85W".into()), gpu: Some("AMD Radeon RX 7600 XT".into()), notebook: None };
        let (u, cut) = issue_url(FbKind::OtherDock, "Meu dock é um Razer Core X", "## Sistema\nx\n", &h);
        assert!(!cut);
        assert!(u.starts_with("https://github.com/LightWolfMan/egpu-painel/issues/new?template=meu-dock.yml&modelo=Intel%20Module"));
        assert!(u.contains("&title=%5BMeu%20dock%5D%20Meu%20dock%20%C3%A9%20um%20Razer%20Core%20X"));
        assert!(u.contains("&o-que-acontece=") && u.contains("&relatorio=%23%23%20Sistema"));
        let big: String = (0..2000).map(|i| format!("linha {i} com acentuação\n")).collect();
        for k in FbKind::ALL {
            let (u, cut) = issue_url(k, "oi", &big, &h);
            assert!(cut, "{k:?}");
            assert!(u.len() <= URL_MAX, "{k:?}: {}", u.len());
            assert!(u.len() > URL_MAX - 400, "{k:?} aproveita o espaço: {}", u.len());
            assert!(u.contains(&pct(CUT_NOTE)), "{k:?}");
        }
        let (u, cut) = issue_url(FbKind::Problem, &"m".repeat(9000), &big, &h);
        assert!(cut && u.len() <= URL_MAX);
        let (u, _) = issue_url(FbKind::Idea, "uma ideia", "r", &h);
        assert!(u.contains("?title=%5BIdeia%5D%20uma%20ideia&body=uma%20ideia"));
    }

    #[test]
    fn title_cut() {
        let h = Hints::default();
        let t = title(FbKind::Problem, &"palavra ".repeat(20), &h);
        assert!(t.starts_with("[Problema] palavra") && t.ends_with('…') && t.chars().count() <= 72);
        assert_eq!(title(FbKind::OtherDock, "", &Hints { dock: Some("Razer Core X".into()), ..Default::default() }), "[Meu dock] Razer Core X");
    }
}
