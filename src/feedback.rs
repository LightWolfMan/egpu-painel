//! Feedback dentro do app: monta um relatório (seções ligáveis), limpa tudo o que identifica a pessoa
//! ou a máquina e abre uma issue no GitHub pelo navegador da própria pessoa (URL pré-preenchida com os
//! formulários de .github/ISSUE_TEMPLATE). Nada de rede aqui: a URL leva o que couber (~6000 bytes) e,
//! se o relatório não couber, ele vai para a área de transferência.

use crate::collect::{GpuStatic, Sample};
use crate::i18n::t;
use crate::history::{Ev, Kind};
use crate::pnp::{self, Node, PVal, PortRole, Topology, Usb4Prop};
use regex_lite::{Captures, Regex};
use std::fmt::Write as _;
use std::sync::OnceLock;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO_NEW: &str = "https://github.com/LightWolfMan/egpu-painel/issues/new";
/// Teto da URL inteira. Medido em 2026-11-16: sem login, o GitHub responde 302 (vai para o login) até
/// ~6900 bytes e 500 a partir de ~7000; uma URL de ~6000 bytes chega inteira ao navegador padrão pelo
/// ShellExecute. 6000 deixa folga.
pub const URL_MAX: usize = 6000;
pub const CUT_NOTE: &str = "(relatório cortado — o completo está na área de transferência, cole aqui)";
const MSG_CUT_NOTE: &str = "(mensagem cortada — a completa está na área de transferência, cole aqui)";
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
            FbKind::Problem => t("Problema", "Problem"),
            FbKind::Idea => t("Ideia", "Idea"),
            FbKind::OtherDock => t("Meu dock é outro modelo", "My dock is another model"),
        }
    }
    /// (formulário, campo da mensagem): todos os formulários têm também o campo "relatorio".
    fn form(self) -> (&'static str, &'static str) {
        match self {
            FbKind::Problem => ("bug.yml", "descricao"),
            FbKind::Idea => ("ideia.yml", "ideia"),
            FbKind::OtherDock => ("meu-dock.yml", "o-que-acontece"),
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

/// Títulos das seções dentro do relatório (vai para o mantenedor: fica em português).
const SECTIONS: [&str; 4] = ["Sistema", "Dock e topologia", "Placa de vídeo", "Quedas recentes"];

/// Nomes das seções nas caixas do painel (idioma da interface).
pub fn section_label(i: usize) -> &'static str {
    [t("Sistema", "System"), t("Dock e topologia", "Dock and topology"), t("Placa de vídeo", "Graphics card"), t("Quedas recentes", "Recent drops")][i]
}

#[derive(Clone, Default)]
pub struct Report {
    pub sections: [String; 4],
    /// Fabricante e modelo do notebook: só para a limpeza (ver `sanitize`).
    pub hw: Option<String>,
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

/// ID de instância sem a instância: `ENUM\\hardware\\instância` → `ENUM\\hardware\\…`. A instância
/// (serial, posição no barramento) sai na origem; a regex de `sanitize` fica só de rede de segurança.
fn strip_instance(id: &str) -> String {
    match id.rsplit_once('\\') {
        Some((hw, _)) if hw.contains('\\') => format!("{hw}\\…"),
        _ => id.into(),
    }
}

fn node_line(s: &mut String, label: &str, n: &Option<Node>) {
    match n {
        Some(n) => {
            let _ = writeln!(
                s,
                "{label}: {} [{}] · firmware {} · {} · PCIe {}",
                n.name,
                strip_instance(&n.id),
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

/// Valor de uma chave USB4 para o relatório, por lista de permissão: o valor só sai para as chaves
/// conhecidas que não identificam o aparelho (nomes e IDs do DROM, rota, profundidade, enlace) e para
/// inteiros pequenos. O resto (texto, lista, bytes, 64 bits, GUID, data) sai só com o tipo e o
/// tamanho, para saber que existe.
fn usb4_value(p: &Usb4Prop) -> String {
    let known = matches!(p.pid, 8..=11 | 13..=18 | 20 | 21);
    match &p.val {
        PVal::U8(_) | PVal::U16(_) | PVal::U32(_) | PVal::Bool(_) => p.decoded.clone(),
        PVal::Str(_) | PVal::List(_) | PVal::Bin(_) if known => p.decoded.clone(),
        PVal::Str(x) => format!("texto, {} bytes (removido)", x.len()),
        PVal::List(l) => format!("lista, {} itens (removida)", l.len()),
        PVal::Bin(b) => format!("binário, {} bytes (removido)", b.len()),
        PVal::U64(_) => "inteiro de 64 bits (removido)".into(),
        PVal::Guid(_) => "identificador único (removido)".into(),
        PVal::Time(_) => "data (removida)".into(),
        PVal::Other(ty, n) => format!("tipo 0x{ty:X}, {n} bytes (removido)"),
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

/// Nome genérico do INF (DeviceDesc), não o FriendlyName: num celular o Windows grava como
/// FriendlyName o nome que o dono deu ao aparelho ("Galaxy de Maria").
fn generic_name(n: &Node) -> &str {
    if n.desc.is_empty() { NE } else { &n.desc }
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
            strip_instance(&p.node.id),
            dstate(&p.node),
            p.child.as_ref().map(generic_name).unwrap_or("nenhum")
        );
    }
    for n in &t.usb_a {
        let _ = writeln!(s, "  USB-A: {} [{}]", generic_name(n), strip_instance(&n.id));
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
            let _ = writeln!(s, "Placa: {} [{}]", g.name, strip_instance(&g.id));
            let _ = writeln!(s, "Driver: {} ({})", g.driver.as_deref().unwrap_or(NE), g.driver_provider.as_deref().unwrap_or(NE));
            let _ = writeln!(s, "Energia: {} · PCIe {}", dstate(g), pcie(g));
        }
        None => s += "Placa: não encontrada atrás do dock\n",
    }
    for n in &t.gpu_chain {
        let _ = writeln!(s, "  Cadeia PCIe: {} [{}] · {} · PCIe {}", n.name, strip_instance(&n.id), dstate(n), pcie(n));
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

/// Em baldes de dia, de propósito: somados à data da issue (pública), minuto ou hora mostrariam
/// quando a pessoa liga, desliga e tira o notebook da tomada.
pub fn ago(ms: i64) -> String {
    match ms.max(0) / 86_400_000 {
        0 => "nas últimas 24 h".into(),
        1 => "ontem".into(),
        d => format!("há {d} dias"),
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
        // sem queda, ligar/desligar/dormir/acordar é só rotina: não vai
        let routine = |k: &Kind| matches!(k, Kind::Sleep | Kind::Wake | Kind::StandbyIn | Kind::StandbyOut | Kind::Boot | Kind::Shutdown | Kind::AcOn);
        for k in i.kinds.iter().filter(|k| i.is_drop || !routine(k)) {
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
    Report { sections: [sec_system(nb.as_deref()), sec_dock(t), sec_gpu(t, gst, last), sec_drops(incs, crate::util::now_ms())], hw: nb }
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
    sanitize(&out.replace('\u{A0}', " "), r.hw.as_deref())
}

// ---------------------------------------------------------------------------
// Limpeza

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|s| !s.trim().is_empty())
}

/// Limpa com os nomes desta sessão (usuário, computador, perfil). `hw` = fabricante/modelo do notebook:
/// se o nome de usuário for uma palavra desse texto ("Dell"), só a ocorrência exata do nome do produto
/// fica; em qualquer outro lugar o usuário sai.
pub fn sanitize(s: &str, hw: Option<&str>) -> String {
    sanitize_with(s, env("USERPROFILE").as_deref(), env("USERNAME").as_deref(), env("COMPUTERNAME").as_deref(), hw)
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Troca `word` (sem diferenciar caixa, char a char) quando ela é palavra inteira.
fn replace_word(t: &str, word: &str, with: &str) -> String {
    let w: Vec<char> = word.trim().chars().collect();
    if w.len() < 2 {
        return t.to_string();
    }
    let eq = |a: char, b: char| a == b || a.to_lowercase().eq(b.to_lowercase());
    let c: Vec<char> = t.chars().collect();
    let mut out = String::with_capacity(t.len());
    let mut i = 0;
    while i < c.len() {
        let hit = i + w.len() <= c.len()
            && c[i..i + w.len()].iter().zip(&w).all(|(&a, &b)| eq(a, b))
            && !(i > 0 && is_word(c[i - 1]))
            && !c.get(i + w.len()).is_some_and(|&x| is_word(x));
        if hit {
            out += with;
            i += w.len();
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
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
        // o nome exato do produto ("Dell Inc. Latitude 3450") fica atrás de um marcador; do resto o usuário sai
        let hw = hw.map(str::trim).filter(|h| replace_word(h, u, "\u{0}") != *h);
        if let Some(h) = hw {
            t = replace_word(&t, h, "\u{1}");
        }
        t = replace_word(&t, u, "<usuário>");
        if let Some(h) = hw {
            t = t.replace('\u{1}', h);
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

fn title(kind: FbKind, message: &str) -> String {
    let first = message.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let mut t: String = first.chars().take(60).collect();
    if first.chars().count() > 60 {
        // corta na última palavra inteira
        if let Some(p) = t.rfind(' ') {
            t.truncate(p);
        }
        t.push('…');
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

/// URL da issue nova com o formulário certo pré-preenchido. `message` e `report` chegam já limpos (o
/// que a prévia mostra) e nada além deles vai. Devolve (url, mensagem cortada, relatório cortado).
pub fn issue_url(kind: FbKind, message: &str, report: &str) -> (String, bool, bool) {
    let message = message.trim();
    let (form, msg_field) = kind.form();
    let mut q: Vec<(&str, String)> = vec![("template", form.into()), ("title", title(kind, message))];
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
    // a mensagem tem prioridade, mas não pode comer a URL inteira
    let (mut msg, msg_cut) = fit(message, URL_MAX / 2);
    if msg_cut {
        msg = format!("{msg}\n\n{MSG_CUT_NOTE}");
    }
    if !msg.is_empty() {
        q.push((msg_field, msg));
    }
    let report = report.trim_end();
    if report.is_empty() {
        return (join(&q), msg_cut, false);
    }
    let base = join(&q).len() + "&relatorio=".len();
    let (rep, rep_cut) = if base + pct(report).len() <= URL_MAX {
        (report.to_string(), false)
    } else {
        let note = format!("\n\n{CUT_NOTE}");
        let (r, _) = fit(report, URL_MAX.saturating_sub(base + pct(&note).len()));
        (format!("{r}{note}"), true)
    };
    q.push(("relatorio", rep));
    (join(&q), msg_cut, rep_cut)
}

/// Mensagem + relatório (os dois já limpos): é a prévia e o que "Copiar relatório" copia.
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
        // usuário com nome do fabricante: só o nome exato do produto fica; solto e no caminho sai
        let s = sanitize_with(r"Notebook: Dell Inc. Vostro 5620 · C:\Users\Dell\x · oi Dell", Some(r"C:\Users\Dell"), Some("Dell"), None, Some("Dell Inc. Vostro 5620"));
        assert_eq!(s, r"Notebook: Dell Inc. Vostro 5620 · C:\Users\<usuário>\x · oi <usuário>");
        // fora do ASCII a comparação é por char: caixa e fronteira de palavra continuam valendo
        assert_eq!(sanitize_with("İ notebook-dell-a e NOTEBOOK-DELL-A, notebook-dell-ab", None, None, Some("NOTEBOOK-DELL-A"), None), "İ <computador> e <computador>, notebook-dell-ab");
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
        let (u, mc, rc) = issue_url(FbKind::OtherDock, "Meu dock é um Razer Core X", "## Sistema\nx\n");
        assert!(!mc && !rc);
        assert!(u.starts_with("https://github.com/LightWolfMan/egpu-painel/issues/new?template=meu-dock.yml&title=%5BMeu%20dock%5D%20Meu%20dock%20%C3%A9%20um%20Razer%20Core%20X"));
        assert!(u.contains("&o-que-acontece=") && u.contains("&relatorio=%23%23%20Sistema"));
        // só a mensagem e o relatório: nenhum campo paralelo que a prévia não mostra
        assert!(!u.contains("modelo=") && !u.contains("placa=") && !u.contains("notebook="));
        let big: String = (0..2000).map(|i| format!("linha {i} com acentuação\n")).collect();
        for k in FbKind::ALL {
            let (u, mc, rc) = issue_url(k, "oi", &big);
            assert!(!mc && rc, "{k:?}");
            assert!(u.len() <= URL_MAX, "{k:?}: {}", u.len());
            assert!(u.len() > URL_MAX - 400, "{k:?} aproveita o espaço: {}", u.len());
            assert!(u.contains(&pct(CUT_NOTE)), "{k:?}");
        }
        let (u, mc, rc) = issue_url(FbKind::Problem, &"m".repeat(9000), &big);
        assert!(mc && rc && u.len() <= URL_MAX);
        let (u, _, _) = issue_url(FbKind::Idea, "uma ideia", "r");
        assert!(u.contains("?template=ideia.yml&title=%5BIdeia%5D%20uma%20ideia&ideia=uma%20ideia&relatorio=r"));
    }

    #[test]
    fn title_cut() {
        let t = title(FbKind::Problem, &"palavra ".repeat(20));
        assert!(t.starts_with("[Problema] palavra") && t.ends_with('…') && t.chars().count() <= 72);
        assert_eq!(title(FbKind::OtherDock, ""), "[Meu dock]");
    }

    #[test]
    fn instance_and_usb4_allowlist() {
        assert_eq!(strip_instance(r"USB\VID_1&PID_2\AB(12)CD"), r"USB\VID_1&PID_2\…");
        assert_eq!(strip_instance(r"FOO\BAR\x'y"), r"FOO\BAR\…");
        assert_eq!(strip_instance("sem barra"), "sem barra");
        let p = |pid, val| Usb4Prop { pid, val, meaning: "", confidence: "", decoded: "VALOR".into() };
        assert_eq!(usb4_value(&p(10, PVal::Str("Module 2".into()))), "VALOR");
        assert_eq!(usb4_value(&p(17, PVal::U32(0x15EF))), "VALOR");
        assert_eq!(usb4_value(&p(3, PVal::Str("SERIAL123".into()))), "texto, 9 bytes (removido)");
        assert_eq!(usb4_value(&p(4, PVal::U64(0x1234_5678_9ABC))), "inteiro de 64 bits (removido)");
        assert_eq!(usb4_value(&p(5, PVal::Bin(vec![1, 2]))), "binário, 2 bytes (removido)");
        assert_eq!(usb4_value(&p(6, PVal::List(vec!["a".into()]))), "lista, 1 itens (removida)");
    }

    #[test]
    fn ago_buckets() {
        assert_eq!(ago(7 * 60_000), "nas últimas 24 h");
        assert_eq!(ago(30 * 3_600_000), "ontem");
        assert_eq!(ago(5 * 86_400_000 + 1), "há 5 dias");
    }
}
