//! Fonte de alimentação do dock: o modelo é informado pelo usuário, as specs são buscadas na web
//! (só quando ele pede) e a carga é estimada em tempo real a partir da potência da GPU (ADLX)
//! e do que mais o dock alimenta. A fonte ATX não tem barramento de dados ligado ao dock,
//! então tudo aqui é estimativa, e o painel diz isso.

use regex_lite::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct Psu {
    pub model: String,
    /// Potência nominal (W).
    pub watts: u32,
    /// Corrente máxima no +12 V (A). 0 = desconhecida.
    pub rail12_a: f32,
    /// Certificação 80 PLUS: "", "White", "Bronze", "Silver", "Gold", "Platinum", "Titanium".
    pub rating: String,
    /// "web" (achado na busca) ou "manual".
    pub origin: String,
}

fn path() -> std::path::PathBuf {
    crate::util::data_dir().join("fonte.json")
}

pub fn load() -> Psu {
    std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save(p: &Psu) {
    if let Ok(j) = serde_json::to_string_pretty(p) {
        let _ = std::fs::write(path(), j);
    }
}

pub const RATINGS: [&str; 7] = ["", "White", "Bronze", "Silver", "Gold", "Platinum", "Titanium"];

/// Resultado da busca: valores mais citados nas páginas de resultado.
#[derive(Clone, Default)]
pub struct Found {
    pub watts: Option<u32>,
    pub rail12_a: Option<f32>,
    pub rating: Option<String>,
    /// Quantas vezes cada valor apareceu (para mostrar a confiança).
    pub hits: (usize, usize, usize),
    pub engine: &'static str,
    /// Todos os candidatos a +12 V com contagem (filtrados depois pela coerência com a potência).
    pub amps_all: Vec<(f32, usize)>,
}

/// Baixa uma página com o curl.exe do Windows, sem janela de console.
fn fetch(url: &str) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("curl.exe")
        .args(["-sL", "--compressed", "--max-time", "15", "-A", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) eGPU-Painel", url])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| format!("curl indisponível: {e}"))?;
    if !out.status.success() {
        return Err(format!("sem resposta da busca (curl {})", out.status.code().unwrap_or(-1)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Tira tags e entidades simples: sobra o texto dos títulos e trechos dos resultados.
fn text_of(html: &str) -> String {
    let tags = Regex::new(r"(?s)<script.*?</script>|<style.*?</style>|<[^>]+>").unwrap();
    tags.replace_all(html, " ").replace("&nbsp;", " ").replace("&amp;", "&").replace("&#x2B;", "+").replace("&#43;", "+")
}

fn top<K: std::hash::Hash + Eq + Clone>(m: &HashMap<K, usize>) -> Option<(K, usize)> {
    m.iter().max_by_key(|(_, n)| **n).map(|(k, n)| (k.clone(), *n))
}

/// Extrai potência, +12 V e 80 PLUS do texto, contando as ocorrências.
pub fn parse(text: &str, model: &str) -> Found {
    let mut w: HashMap<u32, usize> = HashMap::new();
    let mut a: HashMap<String, usize> = HashMap::new();
    let mut r: HashMap<String, usize> = HashMap::new();
    for c in Regex::new(r"\b(\d{3,4})\s?(?:W|watts?)\b").unwrap().captures_iter(text) {
        if let Ok(v) = c[1].parse::<u32>() {
            if (200..=2000).contains(&v) {
                *w.entry(v).or_default() += 1;
            }
        }
    }
    for c in Regex::new(r"(?i)\+?\s?12\s?V[^0-9]{0,24}?(\d{2,3}(?:[.,]\d{1,2})?)\s?A\b").unwrap().captures_iter(text) {
        let v = c[1].replace(',', ".");
        if let Ok(f) = v.parse::<f32>() {
            if (10.0..=170.0).contains(&f) {
                *a.entry(format!("{f:.1}")).or_default() += 1;
            }
        }
    }
    for c in Regex::new(r"(?i)80\s?(?:PLUS|\+)\s*(Titanium|Platinum|Gold|Silver|Bronze|White|Standard)?").unwrap().captures_iter(text) {
        let lvl = c.get(1).map(|m| m.as_str().to_lowercase()).unwrap_or_else(|| "white".into());
        let lvl = if lvl == "standard" { "white".into() } else { lvl };
        let mut ch = lvl.chars();
        let cap = ch.next().map(|f| f.to_uppercase().collect::<String>() + ch.as_str()).unwrap_or_default();
        *r.entry(cap).or_default() += 1;
    }
    // número de 3–4 dígitos no próprio modelo (RM650x, MWE 550, SF750…) é quase sempre a potência
    let in_model = Regex::new(r"(\d{3,4})").unwrap().captures_iter(model).filter_map(|c| c[1].parse::<u32>().ok()).find(|v| (200..=2000).contains(v));
    if let Some(v) = in_model {
        *w.entry(v).or_default() += 3;
    }
    for lvl in ["Titanium", "Platinum", "Gold", "Silver", "Bronze"] {
        if model.to_lowercase().contains(&lvl.to_lowercase()) {
            *r.entry(lvl.to_string()).or_default() += 10;
        }
    }
    r.retain(|_, n| *n >= 2);
    let tw = top(&w);
    let ta = top(&a);
    let tr = top(&r);
    Found {
        watts: tw.map(|x| x.0),
        rail12_a: ta.as_ref().and_then(|x| x.0.parse().ok()),
        rating: tr.as_ref().map(|x| x.0.clone()),
        hits: (tw.map(|x| x.1).unwrap_or(0), ta.map(|x| x.1).unwrap_or(0), tr.map(|x| x.1).unwrap_or(0)),
        engine: "",
        amps_all: a.iter().filter_map(|(k, n)| k.parse().ok().map(|v| (v, *n))).collect(),
    }
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Links dos resultados orgânicos (DuckDuckGo embrulha em /l/?uddg=…; Bing usa <h2><a href>).
fn result_links(html: &str) -> Vec<String> {
    let mut v: Vec<String> = Regex::new(r#"uddg=([^&"']+)"#).unwrap().captures_iter(html).map(|c| pct_decode(&c[1])).collect();
    if v.is_empty() {
        v = Regex::new(r#"<h2[^>]*><a[^>]+href="(https?://[^"]+)""#).unwrap().captures_iter(html).map(|c| c[1].replace("&amp;", "&")).collect();
    }
    v.retain(|u| u.starts_with("http") && !u.contains("duckduckgo.com") && !u.contains("bing.com") && !u.ends_with(".pdf"));
    v.dedup();
    v
}

/// Busca as specs na web (bloqueante: chame numa thread).
pub fn lookup(model: &str) -> Result<Found, String> {
    let q = enc(&format!("{model} power supply specifications 12V watts 80 PLUS"));
    let engines: [(&'static str, String); 2] = [
        ("DuckDuckGo", format!("https://html.duckduckgo.com/html/?q={q}")),
        ("Bing", format!("https://www.bing.com/search?q={q}&setlang=en")),
    ];
    let mut last_err = String::from("nenhum resultado");
    for (name, url) in engines {
        match fetch(&url) {
            Ok(html) => {
                let mut f = parse(&text_of(&html), model);
                // +12 V só vale se for coerente com a potência (fonte moderna: quase tudo sai no +12 V)
                let plausible = |a: f32, w: Option<u32>| match w {
                    Some(w) => (0.88..=1.02).contains(&(a * 12.0 / w as f32)),
                    None => true,
                };
                let mut cands: Vec<(f32, usize)> = f.amps_all.clone();
                let pick = |c: &[(f32, usize)], w: Option<u32>| c.iter().filter(|(a, _)| plausible(*a, w)).max_by_key(|(_, n)| *n).copied();
                if pick(&cands, f.watts).is_none() {
                    // os trechos quase nunca trazem o +12 V: abre as primeiras páginas de resultado
                    for u in result_links(&html).into_iter().take(4) {
                        if let Ok(page) = fetch(&u) {
                            cands.extend(parse(&text_of(&page), "").amps_all);
                            if pick(&cands, f.watts).is_some() {
                                break;
                            }
                        }
                    }
                }
                match pick(&cands, f.watts) {
                    Some((a, n)) => {
                        f.rail12_a = Some(a);
                        f.hits.1 = n;
                    }
                    None => {
                        f.rail12_a = None;
                        f.hits.1 = 0;
                    }
                }
                if f.watts.is_some() || f.rail12_a.is_some() || f.rating.is_some() {
                    f.engine = name;
                    return Ok(f);
                }
                last_err = format!("{name} não trouxe specs reconhecíveis");
            }
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

// ---------------------------------------------------------------------------
// Cálculos

/// Eficiência típica (rede 115 V) por certificação, a 20 / 50 / 100 % de carga.
fn eff_points(rating: &str) -> [f32; 3] {
    match rating {
        "Bronze" => [0.82, 0.85, 0.82],
        "Silver" => [0.85, 0.88, 0.85],
        "Gold" => [0.87, 0.90, 0.87],
        "Platinum" => [0.90, 0.92, 0.89],
        "Titanium" => [0.92, 0.94, 0.90],
        "White" => [0.80, 0.80, 0.80],
        _ => [0.78, 0.82, 0.80],
    }
}

pub fn efficiency(rating: &str, frac: f32) -> f32 {
    let [a, b, c] = eff_points(rating);
    let f = frac.clamp(0.05, 1.2);
    if f <= 0.5 { a + (b - a) * ((f - 0.2).max(0.0) / 0.3) } else { b + (c - b) * ((f - 0.5) / 0.5) }
}

/// Entradas medidas/estimadas do momento.
#[derive(Clone, Copy, Default)]
pub struct Inputs {
    /// Potência total da placa (ADLX), W.
    pub gpu_w: f32,
    /// Notebook recebendo energia do dock?
    pub pd_on: bool,
    /// Taxa de carga da bateria (W, positiva carregando).
    pub batt_w: f32,
    pub usb_devices: usize,
    pub tb_in_use: bool,
}

#[derive(Clone, Copy, Default)]
pub struct Load {
    pub gpu: f32,
    pub pd: f32,
    pub dock: f32,
    pub usb: f32,
    /// Carga DC estimada agora.
    pub now: f32,
    /// Pior caso sustentado (GPU no limite da placa + PD no máximo + portas no máximo).
    pub worst: f32,
    /// Pico transitório estimado (milissegundos) da GPU somado ao resto.
    pub spike: f32,
    pub frac: f32,
    pub amps12: f32,
    pub wall: f32,
    pub eff: f32,
}

/// Limite de potência da RX 7600 XT (TBP de referência).
pub const GPU_TBP_MAX: f32 = 190.0;

pub fn compute(p: &Psu, i: &Inputs) -> Load {
    let dock = 6.0; // JHL7440 + switch PCIe + controlador USB: estimativa
    let usb = if i.usb_devices > 0 { 2.5 } else { 0.0 } + if i.tb_in_use { 7.5 } else { 0.0 };
    // notebook pelo PD: carga da bateria + consumo base estimado do sistema, até os 85 W anunciados
    let pd = if i.pd_on { (i.batt_w.max(0.0) + 18.0).min(85.0) } else { 0.0 };
    let gpu = i.gpu_w.max(0.0);
    let now = gpu + pd + dock + usb;
    let worst = GPU_TBP_MAX.max(gpu) + 85.0 + dock + 4.5 + 15.0;
    let spike = GPU_TBP_MAX.max(gpu) * 1.6 + 85.0 + dock + usb;
    let watts = p.watts.max(1) as f32;
    let frac = now / watts;
    let eff = efficiency(&p.rating, frac);
    Load { gpu, pd, dock, usb, now, worst, spike, frac, amps12: now * 0.97 / 12.0, wall: now / eff, eff }
}
