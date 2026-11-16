#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(unsafe_op_in_unsafe_fn)]
//! eGPU Painel — monitor e diagnóstico do dock eGPU Thunderbolt (foco na placa do dock).
//!
//! Modos:
//!   (sem args)   bandeja + abre o painel
//!   --bandeja    inicia só na bandeja
//!   --painel     só a janela (processo filho que a bandeja dispara; sai ao fechar)
//!
//! A bandeja é um processo mínimo (~2 MB, 0% CPU, bloqueado em GetMessageW) que escuta o
//! Event Log e o PnP e grava o histórico. A janela roda em outro processo, que termina ao
//! fechar: toda a memória do egui/OpenGL/ADLX volta para o sistema.
//!   --bench      roda o teste de banda D3D12 e imprime no console
//!   --perfil [N] mede o custo de cada parte do coletor (N voltas, padrão 20), sem janela
//!   --dump       imprime topologia + chaves USB4 decodificadas

mod bench;
mod collect;
mod config;
mod history;
#[macro_use]
mod i18n;
mod nvml;
mod pnp;
mod psu;
mod titlebar;
mod tray;
mod ui;
mod util;

use std::sync::atomic::Ordering;

/// Toda entrada nova na linha do tempo passa por aqui.
pub fn on_new_event(ev: &history::Ev) {
    use history::Kind::*;
    collect::repaint();
    let fresh = util::now_ms() - ev.ts < 60_000;
    if !fresh {
        return;
    }
    if history::is_panel() {
        return; // balões e ícone são da bandeja
    }
    match ev.kind {
        DockOn if ev.src == "PnP ao vivo" => tray::set_dock(true),
        DockOff | DockFail => tray::set_dock(false),
        _ => {}
    }
    if ev.kind.severity() >= 1 && ev.kind != PcieInfo && config::get().notify {
        tray::notify(
            format!("{} — {}", ev.kind.label(), util::fmt_local(ev.ts, true)),
            ev.msg.chars().take(200).collect(),
        );
    }
}

fn attach_console() {
    unsafe {
        let _ = windows::Win32::System::Console::AttachConsole(windows::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}

fn dump() {
    let t = pnp::read_topology();
    let show = |label: &str, n: &Option<pnp::Node>| {
        if let Some(n) = n {
            println!("{label}: {} [{}] fw={:?} D={:?} pcie={:?}", n.name, n.id, n.firmware, n.dstate, n.pcie);
        } else {
            println!("{label}: —");
        }
    };
    show("controlador host", &t.host_ctrl);
    show("roteador raiz", &t.host_router);
    show("roteador do dock", &t.dock_router);
    show("porta raiz PCIe", &t.root_port);
    show("switch do dock", &t.dock_switch);
    for p in &t.ports {
        println!("  porta {:?}: {} D={:?} filho={:?}", p.role, p.node.id, p.node.dstate, p.child.as_ref().map(|c| &c.name));
    }
    show("GPU", &t.gpu);
    for n in &t.usb_a {
        println!("  USB-A: {} [{}]", n.name, n.id);
    }
    println!("USB4 dock:");
    for p in &t.dock_usb4 {
        println!("  {:>2} {:<44} [{}] {}", p.pid, p.meaning, p.confidence, p.decoded);
    }
    println!("USB4 host:");
    for p in &t.host_usb4 {
        println!("  {:>2} {:<44} [{}] {}", p.pid, p.meaning, p.confidence, p.decoded);
    }
}

const TITLE: &str = "eGPU Painel";

/// Traz a janela do painel (de outro processo) para a frente.
fn focus_panel_window() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::*;
    let t = util::wide(TITLE);
    unsafe {
        match FindWindowW(None, windows::core::PCWSTR(t.as_ptr())) {
            Ok(h) if !h.is_invalid() => {
                let _ = ShowWindow(h, SW_RESTORE);
                let _ = SetForegroundWindow(h);
                true
            }
            _ => false,
        }
    }
}

static CHILD: std::sync::Mutex<Option<std::process::Child>> = std::sync::Mutex::new(None);

/// Abre o painel como processo filho (ou foca o que já está aberto).
fn open_panel() {
    let mut g = CHILD.lock().unwrap();
    if let Some(c) = g.as_mut() {
        if let Ok(None) = c.try_wait() {
            focus_panel_window();
            return;
        }
    }
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(windows::Win32::UI::WindowsAndMessaging::ASFW_ANY);
    }
    if let Ok(exe) = std::env::current_exe() {
        *g = std::process::Command::new(exe).arg("--painel").spawn().ok();
    }
}

fn close_panel() {
    if let Some(mut c) = CHILD.lock().unwrap().take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |a: &str| args.iter().any(|x| x == a);
    i18n::init();

    if has("--dump") {
        attach_console();
        dump();
        return;
    }
    if has("--perfil") {
        attach_console();
        let n = args.iter().skip_while(|a| *a != "--perfil").nth(1).and_then(|x| x.parse().ok()).unwrap_or(20);
        collect::profile(n);
        return;
    }
    if has("--icone") {
        // grava o ícone do painel em .ico (16/32/48/256 px, DIB 32 bpp) — usado pelo atalho
        let out = args.iter().skip_while(|a| *a != "--icone").nth(1).cloned().unwrap_or_else(|| "egpu-painel.ico".into());
        let _ = std::fs::write(&out, ico_bytes(&[16, 32, 48, 256]));
        return;
    }
    if has("--fonte") {
        attach_console();
        let model = args.iter().skip_while(|a| *a != "--fonte").nth(1).cloned().unwrap_or_default();
        match psu::lookup(&model) {
            Ok(f) => println!(
                "{model}: {:?} W ({}x) | +12 V {:?} A ({}x) | 80 PLUS {:?} ({}x) via {}",
                f.watts, f.hits.0, f.rail12_a, f.hits.1, f.rating, f.hits.2, f.engine
            ),
            Err(e) => println!("{model}: falhou: {e}"),
        }
        return;
    }
    if has("--bench") {
        attach_console();
        match bench::run(|s| eprintln!("  {s}")) {
            Ok(r) => println!(
                "{}: host→GPU {:.2} GB/s {:?} | GPU→host {:.2} GB/s {:?} ({} MiB por medição)",
                r.adapter, r.h2d_gbs, r.h2d_runs, r.d2h_gbs, r.d2h_runs, r.per_run_mb
            ),
            Err(e) => println!("falhou: {e}"),
        }
        return;
    }

    if has("--painel") {
        let name = util::wide("Local\\eGPU-Painel-janela");
        let _m = unsafe { windows::Win32::System::Threading::CreateMutexW(None, true, windows::core::PCWSTR(name.as_ptr())) };
        if unsafe { windows::Win32::Foundation::GetLastError() } == windows::Win32::Foundation::ERROR_ALREADY_EXISTS {
            focus_panel_window();
            return;
        }
        history::set_panel_mode();
        history::get();
        tray::DOCK_ON.store(pnp::present_ids("USB4").iter().any(|id| history::dev_role(id) == history::DevRole::DockRouter), Ordering::SeqCst);
        ui::run();
        return;
    }

    // instância única (bandeja)
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let name = util::wide("Local\\eGPU-Painel-instancia");
    let _mutex = unsafe {
        windows::Win32::System::Threading::CreateMutexW(None, true, windows::core::PCWSTR(name.as_ptr()))
    };
    if unsafe { windows::Win32::Foundation::GetLastError() } == windows::Win32::Foundation::ERROR_ALREADY_EXISTS {
        tray::signal_existing();
        return;
    }

    history::get();
    tray::DOCK_ON.store(pnp::present_ids("USB4").iter().any(|id| history::dev_role(id) == history::DevRole::DockRouter), Ordering::SeqCst);
    history::start_pnp();
    std::thread::spawn(|| {
        let n = history::start_event_log();
        history::scan_overwatch();
        let t = pnp::read_topology();
        history::record_arrivals(&t);
        if n > 0 {
            collect::repaint();
        }
    });
    tray::create();

    if !has("--bandeja") {
        open_panel();
    }
    util::trim_working_set();
    loop {
        match tray::wait() {
            tray::Cmd::Open => open_panel(),
            tray::Cmd::Quit => break,
        }
    }
    close_panel();
    tray::destroy();
}

/// Monta um .ico com imagens DIB 32 bpp (BGRA de baixo para cima + máscara AND vazia).
fn ico_bytes(sizes: &[u32]) -> Vec<u8> {
    let imgs: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&n| {
            let rgba = tray::icon_rgba(n, true);
            let mut b = Vec::new();
            for v in [40u32, n, n * 2, 1 | (32 << 16), 0, 0, 0, 0, 0, 0] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            for y in (0..n).rev() {
                for x in 0..n {
                    let i = ((y * n + x) * 4) as usize;
                    b.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
                }
            }
            let mask_row = (n.div_ceil(32) * 4) as usize;
            b.extend(std::iter::repeat_n(0u8, mask_row * n as usize));
            b
        })
        .collect();
    let mut out = vec![0, 0, 1, 0];
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut off = 6 + 16 * sizes.len() as u32;
    for (n, img) in sizes.iter().zip(&imgs) {
        let d = if *n >= 256 { 0 } else { *n as u8 };
        out.extend_from_slice(&[d, d, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&off.to_le_bytes());
        off += img.len() as u32;
    }
    for img in imgs {
        out.extend(img);
    }
    out
}
