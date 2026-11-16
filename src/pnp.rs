//! Leitura de propriedades PnP (cfgmgr32) e montagem da topologia real
//! note -> USB4 host -> cabo -> roteador do dock -> switch PCIe -> GPU / xHCI / porta TB.

use crate::util::{filetime_to_unix_ms, from_multi_wide, from_wide, wide};
use windows::core::{GUID, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Devices::Properties::*;
use windows::Win32::Foundation::DEVPROPKEY;

/// fmtid das propriedades USB4 que o Windows expõe sem documentação.
pub const USB4_FMTID: GUID = GUID::from_u128(0x5df7e321_1c1b_4ce2_b4fa_55f4a5bc2cb6);
const PCI_FMTID: GUID = GUID::from_u128(0x3ab22e31_8264_4b4e_9af5_a8d2d8e33e62);

const fn pci(pid: u32) -> DEVPROPKEY {
    DEVPROPKEY { fmtid: PCI_FMTID, pid }
}
pub const PCI_DEVICE_TYPE: DEVPROPKEY = pci(1);
pub const PCI_CUR_SPEED: DEVPROPKEY = pci(9);
pub const PCI_CUR_WIDTH: DEVPROPKEY = pci(10);
pub const PCI_MAX_SPEED: DEVPROPKEY = pci(11);
pub const PCI_MAX_WIDTH: DEVPROPKEY = pci(12);

#[derive(Clone, Debug, PartialEq)]
pub enum PVal {
    Str(String),
    List(Vec<String>),
    U32(u32),
    U16(u16),
    U8(u8),
    U64(u64),
    Bool(bool),
    Bin(Vec<u8>),
    Time(i64),
    Guid(String),
    Other(u32, usize),
}

impl PVal {
    pub fn as_u32(&self) -> Option<u32> {
        match self {
            PVal::U32(v) => Some(*v),
            PVal::U16(v) => Some(*v as u32),
            PVal::U8(v) => Some(*v as u32),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            PVal::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_time(&self) -> Option<i64> {
        match self {
            PVal::Time(t) => Some(*t),
            _ => None,
        }
    }
    pub fn type_name(&self) -> &'static str {
        match self {
            PVal::Str(_) => "String",
            PVal::List(_) => "StringList",
            PVal::U32(_) => "UInt32",
            PVal::U16(_) => "UInt16",
            PVal::U8(_) => "Byte",
            PVal::U64(_) => "UInt64",
            PVal::Bool(_) => "Boolean",
            PVal::Bin(_) => "Binary",
            PVal::Time(_) => "FileTime",
            PVal::Guid(_) => "Guid",
            PVal::Other(..) => "?",
        }
    }
    pub fn show(&self) -> String {
        match self {
            PVal::Str(s) => s.clone(),
            PVal::List(v) => v.join(", "),
            PVal::U32(v) => format!("{v} (0x{v:X})"),
            PVal::U16(v) => format!("{v} (0x{v:04X})"),
            PVal::U8(v) => format!("{v}"),
            PVal::U64(v) => format!("{v}"),
            PVal::Bool(b) => if *b { "verdadeiro".into() } else { "falso".into() },
            PVal::Bin(b) => b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" "),
            PVal::Time(t) => crate::util::fmt_local(*t, false),
            PVal::Guid(g) => g.clone(),
            PVal::Other(t, n) => format!("tipo 0x{t:X}, {n} bytes"),
        }
    }
}

pub fn locate(id: &str) -> Option<u32> {
    let w = wide(id);
    let mut dn = 0u32;
    let r = unsafe { CM_Locate_DevNodeW(&mut dn, PCWSTR(w.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) };
    (r == CR_SUCCESS).then_some(dn)
}

pub fn device_id(dn: u32) -> String {
    let mut buf = [0u16; 512];
    unsafe {
        let _ = CM_Get_Device_IDW(dn, &mut buf, 0);
    }
    from_wide(&buf)
}

pub fn parent(dn: u32) -> Option<u32> {
    let mut p = 0;
    (unsafe { CM_Get_Parent(&mut p, dn, 0) } == CR_SUCCESS).then_some(p)
}

pub fn children(dn: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut c = 0;
    if unsafe { CM_Get_Child(&mut c, dn, 0) } != CR_SUCCESS {
        return out;
    }
    out.push(c);
    loop {
        let mut s = 0;
        if unsafe { CM_Get_Sibling(&mut s, c, 0) } != CR_SUCCESS {
            break;
        }
        out.push(s);
        c = s;
    }
    out
}

pub fn status(dn: u32) -> (u32, u32) {
    let mut st = CM_DEVNODE_STATUS_FLAGS(0);
    let mut prob = CM_PROB(0);
    unsafe {
        let _ = CM_Get_DevNode_Status(&mut st, &mut prob, dn, 0);
    }
    (st.0, prob.0)
}

pub fn prop(dn: u32, key: &DEVPROPKEY) -> Option<PVal> {
    unsafe {
        let mut ty = DEVPROPTYPE(0);
        let mut size = 0u32;
        let _ = CM_Get_DevNode_PropertyW(dn, key, &mut ty, None, &mut size, 0);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        if CM_Get_DevNode_PropertyW(dn, key, &mut ty, Some(buf.as_mut_ptr()), &mut size, 0) != CR_SUCCESS {
            return None;
        }
        Some(decode(ty.0, &buf[..size as usize]))
    }
}

fn decode(ty: u32, b: &[u8]) -> PVal {
    let u16s = || b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>();
    match ty {
        0x12 => PVal::Str(from_wide(&u16s())),
        0x2012 => PVal::List(from_multi_wide(&u16s())),
        0x07 if b.len() >= 4 => PVal::U32(u32::from_le_bytes(b[..4].try_into().unwrap())),
        0x05 if b.len() >= 2 => PVal::U16(u16::from_le_bytes([b[0], b[1]])),
        0x03 if !b.is_empty() => PVal::U8(b[0]),
        0x09 if b.len() >= 8 => PVal::U64(u64::from_le_bytes(b[..8].try_into().unwrap())),
        0x11 if !b.is_empty() => PVal::Bool(b[0] != 0),
        0x1003 => PVal::Bin(b.to_vec()),
        0x10 if b.len() >= 8 => PVal::Time(filetime_to_unix_ms(u64::from_le_bytes(b[..8].try_into().unwrap()))),
        0x0D if b.len() >= 16 => {
            let d1 = u32::from_le_bytes(b[0..4].try_into().unwrap());
            let d2 = u16::from_le_bytes([b[4], b[5]]);
            let d3 = u16::from_le_bytes([b[6], b[7]]);
            PVal::Guid(format!(
                "{{{d1:08X}-{d2:04X}-{d3:04X}-{:02X}{:02X}-{}}}",
                b[8], b[9],
                b[10..16].iter().map(|x| format!("{x:02X}")).collect::<String>()
            ))
        }
        t => PVal::Other(t, b.len()),
    }
}

/// Todas as chaves de propriedade do nó (usado para varrer as chaves USB4 não documentadas).
pub fn all_keys(dn: u32) -> Vec<DEVPROPKEY> {
    unsafe {
        let mut n = 0u32;
        let _ = CM_Get_DevNode_Property_Keys(dn, None, &mut n, 0);
        if n == 0 {
            return vec![];
        }
        let mut keys = vec![DEVPROPKEY::default(); n as usize];
        if CM_Get_DevNode_Property_Keys(dn, Some(keys.as_mut_ptr()), &mut n, 0) != CR_SUCCESS {
            return vec![];
        }
        keys.truncate(n as usize);
        keys
    }
}

/// IDs presentes de um enumerador ("PCI", "USB4", "USB").
pub fn present_ids(enumerator: &str) -> Vec<String> {
    let f = wide(enumerator);
    let flags = CM_GETIDLIST_FILTER_ENUMERATOR | CM_GETIDLIST_FILTER_PRESENT;
    unsafe {
        let mut len = 0u32;
        if CM_Get_Device_ID_List_SizeW(&mut len, PCWSTR(f.as_ptr()), flags) != CR_SUCCESS || len == 0 {
            return vec![];
        }
        let mut buf = vec![0u16; len as usize];
        if CM_Get_Device_ID_ListW(PCWSTR(f.as_ptr()), &mut buf, flags) != CR_SUCCESS {
            return vec![];
        }
        from_multi_wide(&buf)
    }
}

// ---------------------------------------------------------------------------
// Nó genérico

#[derive(Clone, Debug, Default)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub present: bool,
    pub problem: u32,
    pub firmware: Option<String>,
    pub driver: Option<String>,
    pub driver_provider: Option<String>,
    pub last_arrival: Option<i64>,
    pub last_removal: Option<i64>,
    pub location: Option<String>,
    /// (atual GT/s código, largura atual, max código, largura max)
    pub pcie: Option<(u32, u32, u32, u32)>,
    pub children: Vec<String>,
    /// Estado de energia mais recente (DEVPKEY_Device_PowerData): 1=D0 .. 4=D3
    pub dstate: Option<u32>,
    pub removal_policy: Option<u32>,
    /// Máscaras/flags de erro PCIe expostas pelo pci.sys (pid -> valor)
    pub pci_err: Vec<(&'static str, String)>,
}

pub fn read_node(dn: u32) -> Node {
    let s = |k: &DEVPROPKEY| prop(dn, k).and_then(|v| v.as_str().map(String::from));
    let (st, problem) = status(dn);
    let name = s(&DEVPKEY_Device_FriendlyName)
        .or_else(|| s(&DEVPKEY_Device_DeviceDesc))
        .unwrap_or_default();
    let pcie = match (
        prop(dn, &PCI_CUR_SPEED).and_then(|v| v.as_u32()),
        prop(dn, &PCI_CUR_WIDTH).and_then(|v| v.as_u32()),
    ) {
        (Some(cs), Some(cw)) => Some((
            cs,
            cw,
            prop(dn, &PCI_MAX_SPEED).and_then(|v| v.as_u32()).unwrap_or(0),
            prop(dn, &PCI_MAX_WIDTH).and_then(|v| v.as_u32()).unwrap_or(0),
        )),
        _ => None,
    };
    Node {
        id: device_id(dn),
        name,
        present: st & 0x8 != 0 || st & 0x2 != 0, // DN_STARTED | DN_DRIVER_LOADED
        problem,
        firmware: s(&DEVPKEY_Device_FirmwareVersion),
        driver: s(&DEVPKEY_Device_DriverVersion),
        driver_provider: s(&DEVPKEY_Device_DriverProvider),
        last_arrival: prop(dn, &DEVPKEY_Device_LastArrivalDate).and_then(|v| v.as_time()),
        last_removal: prop(dn, &DEVPKEY_Device_LastRemovalDate).and_then(|v| v.as_time()),
        location: s(&DEVPKEY_Device_LocationInfo),
        pcie,
        children: children(dn).into_iter().map(device_id).collect(),
        dstate: match prop(dn, &DEVPKEY_Device_PowerData) {
            Some(PVal::Bin(b)) if b.len() >= 8 => Some(u32::from_le_bytes(b[4..8].try_into().unwrap())),
            _ => None,
        },
        removal_policy: prop(dn, &DEVPKEY_Device_RemovalPolicy).and_then(|v| v.as_u32()),
        pci_err: [
            ("AER presente", 17u32),
            ("Máscara de erros não corrigíveis", 19),
            ("Severidade não corrigíveis", 20),
            ("Máscara de erros corrigíveis", 21),
            ("ECRC", 22),
            ("Error reporting", 23),
            ("Tipo de dispositivo PCIe", 1),
            ("Versão PCIe Express", 13),
            ("Serial (DSN)", 40),
        ]
        .iter()
        .filter_map(|(n, pid)| prop(dn, &pci(*pid)).map(|v| (*n, v.show())))
        .collect(),
    }
}

pub fn dstate_name(d: u32) -> &'static str {
    match d {
        1 => "D0 (ativo)",
        2 => "D1",
        3 => "D2",
        4 => "D3 (economia)",
        _ => "?",
    }
}

pub fn pci_type_name(t: u32) -> &'static str {
    // DevProp_PciDevice_DeviceType_* (pciprop.h)
    match t {
        0 => "PCI convencional",
        1 => "PCI-X",
        2 => "Endpoint PCIe",
        3 => "Endpoint PCIe legado",
        4 => "Endpoint integrado ao root complex",
        5 => "PCIe tratado como PCI",
        6 => "Ponte PCI",
        7 => "Ponte PCI-X",
        8 => "Porta raiz PCIe",
        9 => "Porta upstream de switch",
        10 => "Porta downstream de switch",
        11 => "Ponte PCIe→PCI-X",
        12 => "Ponte PCI-X→PCIe",
        13 => "Ponte PCIe tratada como PCI",
        14 => "Coletor de eventos",
        _ => "?",
    }
}

pub fn pcie_speed_name(code: u32) -> &'static str {
    match code {
        1 => "Gen1\u{A0}2,5\u{A0}GT/s",
        2 => "Gen2\u{A0}5\u{A0}GT/s",
        3 => "Gen3\u{A0}8\u{A0}GT/s",
        4 => "Gen4\u{A0}16\u{A0}GT/s",
        5 => "Gen5\u{A0}32\u{A0}GT/s",
        _ => "?",
    }
}

// ---------------------------------------------------------------------------
// Propriedades USB4 não documentadas: decodificação empírica

#[derive(Clone, Debug)]
pub struct Usb4Prop {
    pub pid: u32,
    pub val: PVal,
    pub meaning: &'static str,
    /// "certo", "forte", "hipótese", "desconhecido"
    pub confidence: &'static str,
    pub decoded: String,
}

/// Tabela empírica montada comparando o roteador do dock (Titan Ridge) com o
/// roteador raiz (host Intel Gen12) e com os IDs de hardware publicados pelo PnP.
/// A evidência de cada linha está em LOGBOOK.md (2026-11-15).
pub fn decode_usb4(pid: u32, v: &PVal) -> (&'static str, &'static str, String) {
    let n = v.as_u32();
    match pid {
        9 => ("Fabricante (DROM)", "certo", v.show()),
        10 => ("Modelo (DROM)", "certo", v.show()),
        13 => ("Vendor ID (DROM)", "forte", n.map(|x| format!("0x{x:04X}{}", vendor(x))).unwrap_or_default()),
        14 => ("Model ID (DROM)", "forte", n.map(|x| format!("0x{x:04X}")).unwrap_or_default()),
        16 => ("Vendor ID do roteador", "certo", n.map(|x| format!("0x{x:04X}{}", vendor(x))).unwrap_or_default()),
        17 => ("Device ID do roteador", "certo", n.map(|x| format!("0x{x:04X}{}", chip(x))).unwrap_or_default()),
        18 => ("Revisão do roteador", "certo", n.map(|x| format!("REV_{x:04X}")).unwrap_or_default()),
        11 => ("Route string (porta por nível)", "forte", match v {
            PVal::Bin(b) => {
                let hops: Vec<String> = b.iter().take_while(|&&x| x != 0).map(|x| format!("porta {x}")).collect();
                if hops.is_empty() { "raiz (o próprio host)".into() } else { format!("host → {}", hops.join(" → ")) }
            }
            _ => v.show(),
        }),
        15 => ("Profundidade na árvore USB4", "forte", n.map(|x| format!("nível {x}")).unwrap_or_default()),
        8 => ("Roteador TBT3 legado", "forte", v.show()),
        20 => ("Velocidade por faixa (código)", "hipótese", n.map(link_speed_code).unwrap_or_default()),
        21 => ("Faixas ativas no enlace", "hipótese", n.map(|x| format!("{x} faixa(s)")).unwrap_or_default()),
        19 => ("Desconhecida (dock 16 / host 32)", "desconhecido", v.show()),
        2 => ("Desconhecida — máscara? (dock 7 / host 15)", "desconhecido", v.show()),
        12 => ("Desconhecida (igual em host e dock: 0x4D0001)", "desconhecido", v.show()),
        _ => ("Desconhecida", "desconhecido", v.show()),
    }
}

fn vendor(x: u32) -> &'static str {
    match x {
        0x8086 => " (Intel)",
        0x8087 => " (Intel, ID USB4 do host)",
        _ => "",
    }
}

fn chip(x: u32) -> &'static str {
    match x {
        0x15EF => " (JHL7440 Titan Ridge DD)",
        0xA73E => " (controlador TBT4 integrado Intel)",
        _ => "",
    }
}

/// Codificação do campo "Current Link Speed" do USB4 (Lane Adapter CS1):
/// 0x8 = Gen2 10 Gb/s, 0x4 = Gen3 20 Gb/s, 0x2 = Gen4 40 Gb/s.
/// A hipótese é que o Windows publica esse campo cru; a validação vem do
/// teste de banda (ver LOGBOOK).
pub fn link_speed_code(x: u32) -> String {
    match x {
        0x8 => "0x8 → Gen2/TB3 10\u{A0}Gb/s por faixa (se for o campo do spec USB4)".into(),
        0x4 => "0x4 → Gen3/TB3 20\u{A0}Gb/s por faixa".into(),
        0x2 => "0x2 → Gen4 40\u{A0}Gb/s por faixa".into(),
        x => format!("{x} (código desconhecido)"),
    }
}

pub fn usb4_props(dn: u32) -> Vec<Usb4Prop> {
    let mut v: Vec<Usb4Prop> = all_keys(dn)
        .into_iter()
        .filter(|k| k.fmtid == USB4_FMTID)
        .filter_map(|k| {
            let val = prop(dn, &k)?;
            let (meaning, confidence, decoded) = decode_usb4(k.pid, &val);
            Some(Usb4Prop { pid: k.pid, val, meaning, confidence, decoded })
        })
        .collect();
    v.sort_by_key(|p| p.pid);
    v
}

// ---------------------------------------------------------------------------
// Topologia

#[derive(Clone, Debug, Default)]
pub struct Topology {
    pub host_ctrl: Option<Node>,     // PCI ...A73E (controlador USB4 do note)
    pub host_router: Option<Node>,   // USB4\ROOT_DEVICE_ROUTER
    pub host_usb4: Vec<Usb4Prop>,
    pub dock_router: Option<Node>,   // USB4\VID_8086&PID_15EF...
    pub dock_usb4: Vec<Usb4Prop>,
    pub root_port: Option<Node>,     // porta raiz PCIe do note que leva o túnel
    pub dock_switch: Option<Node>,   // switch upstream 15EF
    pub ports: Vec<DockPort>,        // portas downstream do switch
    pub gpu: Option<Node>,
    pub gpu_chain: Vec<Node>,        // switch interno da placa (1478/1479)
    pub usb_a: Vec<Node>,            // dispositivos atrás do xHCI do dock
    pub read_ms: i64,
}

#[derive(Clone, Debug)]
pub struct DockPort {
    pub node: Node,
    pub role: PortRole,
    pub child: Option<Node>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PortRole {
    Gpu,
    UsbA,
    TbExtra,
    Other,
}

fn is_dock_router(id: &str) -> bool {
    id.starts_with("USB4\\") && !id.contains("ROOT_DEVICE_ROUTER") && !id.contains("VIRTUAL_POWER")
}

pub fn read_topology() -> Topology {
    let mut t = Topology { read_ms: crate::util::now_ms(), ..Default::default() };

    // --- lado USB4
    for id in present_ids("USB4") {
        let Some(dn) = locate(&id) else { continue };
        if id.contains("ROOT_DEVICE_ROUTER") {
            t.host_usb4 = usb4_props(dn);
            if let Some(p) = parent(dn) {
                t.host_ctrl = Some(read_node(p));
            }
            t.host_router = Some(read_node(dn));
        } else if is_dock_router(&id) && t.dock_router.is_none() {
            t.dock_usb4 = usb4_props(dn);
            t.dock_router = Some(read_node(dn));
        }
    }

    // --- lado PCIe: acha a GPU AMD tunelada e sobe a cadeia de pais
    let pci = present_ids("PCI");
    let gpu_id = pci.iter().find(|id| {
        id.starts_with("PCI\\VEN_1002&DEV_") && locate(id).map(|dn| {
            prop(dn, &DEVPKEY_Device_Class).and_then(|v| v.as_str().map(|s| s == "Display")).unwrap_or(false)
        }).unwrap_or(false)
    });
    // Sobe a partir da GPU, ou, sem GPU, a partir do xHCI do dock.
    let start = gpu_id.cloned().or_else(|| pci.iter().find(|id| id.contains("DEV_15F0")).cloned());
    if let Some(start) = start {
        if let Some(mut dn) = locate(&start) {
            if gpu_id.is_some() {
                t.gpu = Some(read_node(dn));
            }
            // cadeia até a porta raiz (pai cujo pai não é PCI)
            let mut chain = vec![];
            while let Some(p) = parent(dn) {
                let pid = device_id(p);
                if !pid.starts_with("PCI\\") {
                    break;
                }
                chain.push(p);
                dn = p;
            }
            // chain: [.., dock DS, dock US, root port]
            if let Some(&rp) = chain.last() {
                t.root_port = Some(read_node(rp));
            }
            if chain.len() >= 2 {
                let us = chain[chain.len() - 2];
                t.dock_switch = Some(read_node(us));
                // GPU chain: o que fica entre o DS do dock e a GPU (switch interno da placa)
                if gpu_id.is_some() && chain.len() >= 3 {
                    t.gpu_chain = chain[..chain.len() - 3].iter().rev().map(|&d| read_node(d)).collect();
                }
                for ds in children(us) {
                    let node = read_node(ds);
                    let kids = children(ds);
                    let child = kids.first().map(|&c| read_node(c));
                    let role = match &child {
                        Some(c) if c.id.starts_with("PCI\\VEN_1002") => PortRole::Gpu,
                        Some(c) if is_xhci(kids[0]) => PortRole::UsbA,
                        None => PortRole::TbExtra,
                        _ => PortRole::Other,
                    };
                    if role == PortRole::UsbA {
                        t.usb_a = usb_descendants(kids[0]);
                    }
                    t.ports.push(DockPort { node, role, child });
                }
            }
        }
    }
    t
}

fn is_xhci(dn: u32) -> bool {
    prop(dn, &DEVPKEY_Device_Class).and_then(|v| v.as_str().map(|s| s == "USB")).unwrap_or(false)
        && device_id(dn).starts_with("PCI\\")
}

/// Dispositivos USB "de verdade" atrás do xHCI do dock (pula hubs raiz e filhos de interface).
pub fn usb_descendants(xhci: u32) -> Vec<Node> {
    let mut out = vec![];
    let mut stack = children(xhci);
    while let Some(dn) = stack.pop() {
        let id = device_id(dn);
        let is_root_hub = id.starts_with("USB\\ROOT_HUB");
        if !is_root_hub && id.starts_with("USB\\") && !id.contains("&MI_") {
            out.push(read_node(dn));
        }
        if is_root_hub || id.starts_with("USB\\") && !id.contains("&MI_") {
            stack.extend(children(dn));
        }
    }
    out
}

/// IDs de todos os descendentes de um nó (para saber, na remoção, o que estava atrás do dock).
pub fn descendant_ids(dn: u32) -> Vec<String> {
    let mut out = vec![];
    let mut stack = children(dn);
    while let Some(d) = stack.pop() {
        out.push(device_id(d).to_uppercase());
        stack.extend(children(d));
    }
    out
}
