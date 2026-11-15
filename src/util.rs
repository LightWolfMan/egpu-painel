//! Utilidades pequenas: strings largas, tempo, caminhos, consumo do processo.

use std::path::PathBuf;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Lista de strings REG_MULTI_SZ (terminada por dois NULs).
pub fn from_multi_wide(buf: &[u16]) -> Vec<String> {
    buf.split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

/// FILETIME (100 ns desde 1601) -> milissegundos Unix.
pub fn filetime_to_unix_ms(ft: u64) -> i64 {
    (ft as i64 - 116_444_736_000_000_000) / 10_000
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn fmt_local(ms: i64, with_ms: bool) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ms).single() {
        Some(t) if with_ms => t.format("%d/%m %H:%M:%S%.3f").to_string(),
        Some(t) => t.format("%d/%m/%Y %H:%M:%S").to_string(),
        None => "?".into(),
    }
}

pub fn fmt_ago(ms: i64) -> String {
    let d = (now_ms() - ms).max(0) / 1000;
    if d < 60 {
        format!("há {d} s")
    } else if d < 3600 {
        format!("há {} min", d / 60)
    } else if d < 86400 {
        format!("há {} h {} min", d / 3600, (d % 3600) / 60)
    } else {
        format!("há {} d {} h", d / 86400, (d % 86400) / 3600)
    }
}

/// %LOCALAPPDATA%\eGPU-Painel
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let d = base.join("eGPU-Painel");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Pasta Documentos real (respeita redirecionamento do OneDrive).
pub fn documents_dir() -> Option<PathBuf> {
    use windows::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as _));
        s.map(PathBuf::from)
    }
}

/// Consumo do próprio processo: (working set MB, privados MB, CPU total em ms).
pub fn process_usage() -> (f64, f64, f64) {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    unsafe {
        let h = GetCurrentProcess();
        let mut pmc = PROCESS_MEMORY_COUNTERS_EX::default();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let _ = GetProcessMemoryInfo(h, &mut pmc as *mut _ as *mut _, pmc.cb);
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let _ = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u);
        let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 10_000.0;
        (
            pmc.WorkingSetSize as f64 / 1048576.0,
            pmc.PrivateUsage as f64 / 1048576.0,
            ft(k) + ft(u),
        )
    }
}

/// Devolve as páginas ociosas ao sistema depois de fechar a janela.
pub fn trim_working_set() {
    use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
    use windows::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let _ = EmptyWorkingSet(GetCurrentProcess());
    }
}
