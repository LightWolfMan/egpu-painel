//! Preferências do usuário (%LOCALAPPDATA%\eGPU-Painel\config.json) e opções que alteram o sistema.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
pub struct Config {
    /// Balões na bandeja quando o dock cai / há erro.
    pub notify: bool,
    /// Animação econômica (menos quadros por segundo).
    pub eco: bool,
    /// 0 = segue o Windows, 1 = claro, 2 = escuro
    #[serde(default)]
    pub theme: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self { notify: true, eco: false, theme: 0 }
    }
}

static CFG: Mutex<Option<Config>> = Mutex::new(None);

fn path() -> std::path::PathBuf {
    crate::util::data_dir().join("config.json")
}

pub fn get() -> Config {
    let mut g = CFG.lock().unwrap();
    *g.get_or_insert_with(|| {
        std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    })
}

pub fn set(c: Config) {
    *CFG.lock().unwrap() = Some(c);
    if let Ok(j) = serde_json::to_string_pretty(&c) {
        let _ = std::fs::write(path(), j);
    }
}

// ---------------------------------------------------------------------------
// Iniciar com o Windows: HKCU\...\Run (sem admin), entra direto na bandeja.

use windows::core::PCWSTR;
use windows::Win32::System::Registry::*;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "eGPU Painel";

pub fn autostart_enabled() -> bool {
    let k = crate::util::wide(RUN_KEY);
    let v = crate::util::wide(VALUE);
    unsafe {
        let mut size = 0u32;
        RegGetValueW(HKEY_CURRENT_USER, PCWSTR(k.as_ptr()), PCWSTR(v.as_ptr()), RRF_RT_REG_SZ, None, None, Some(&mut size)).is_ok()
    }
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    let k = crate::util::wide(RUN_KEY);
    let v = crate::util::wide(VALUE);
    unsafe {
        let mut h = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(k.as_ptr()), Some(0), KEY_SET_VALUE, &mut h).ok().map_err(|e| e.to_string())?;
        let r = if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let cmd = crate::util::wide(&format!("\"{}\" --bandeja", exe.display()));
            let bytes = std::slice::from_raw_parts(cmd.as_ptr() as *const u8, cmd.len() * 2);
            RegSetValueExW(h, PCWSTR(v.as_ptr()), Some(0), REG_SZ, Some(bytes))
        } else {
            RegDeleteValueW(h, PCWSTR(v.as_ptr()))
        };
        let _ = RegCloseKey(h);
        r.ok().map_err(|e| e.to_string())
    }
}

/// Exporta a linha do tempo inteira em CSV na pasta de dados e devolve o caminho.
pub fn export_csv() -> Result<std::path::PathBuf, String> {
    let evs = crate::history::get().snapshot();
    let mut out = String::from("data_local;tipo;categoria;fonte;mensagem;dispositivo;estado_no_instante\n");
    let esc = |s: &str| s.replace(';', ",").replace('\n', " ");
    for e in evs {
        out.push_str(&format!(
            "{};{};{:?};{};{};{};{}\n",
            crate::util::fmt_local(e.ts, true),
            e.kind.label(),
            e.kind.cat(),
            esc(&e.src),
            esc(&e.msg),
            esc(e.dev.as_deref().unwrap_or("")),
            esc(e.snap.as_deref().unwrap_or(""))
        ));
    }
    let p = crate::util::data_dir().join(format!("historico-{}.csv", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    // BOM para o Excel abrir acentos corretamente
    std::fs::write(&p, [&[0xEF, 0xBB, 0xBF][..], out.as_bytes()].concat()).map_err(|e| e.to_string())?;
    Ok(p)
}

pub fn open_folder(p: &std::path::Path) {
    let _ = std::process::Command::new("explorer.exe").arg(p).spawn();
}
