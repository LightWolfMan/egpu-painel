#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(unsafe_op_in_unsafe_fn)]
//! eGPU Painel — monitor e diagnóstico do dock eGPU Thunderbolt (foco na placa do dock).
//!
//! Modos:
//!   (sem args)   abre o painel e fica na bandeja ao fechar
//!   --bandeja    inicia só na bandeja
//!   --bench      roda o teste de banda D3D12 e imprime no console
//!   --dump       imprime topologia + chaves USB4 decodificadas

mod bench;
mod collect;
mod history;
mod pnp;
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
    match ev.kind {
        DockOn if ev.src == "PnP ao vivo" => tray::set_dock(true),
        DockOff | DockFail => tray::set_dock(false),
        _ => {}
    }
    if ev.kind.severity() >= 1 && ev.kind != PcieInfo {
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |a: &str| args.iter().any(|x| x == a);

    if has("--dump") {
        attach_console();
        dump();
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

    // instância única
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

    let mut open = !has("--bandeja");
    loop {
        if open {
            tray::PANEL_OPEN.store(true, Ordering::SeqCst);
            ui::run();
            tray::PANEL_OPEN.store(false, Ordering::SeqCst);
            util::trim_working_set();
            if let Some(tray::Cmd::Quit) = tray::take_pending() {
                break;
            }
        }
        match tray::wait() {
            tray::Cmd::Open => open = true,
            tray::Cmd::Quit => break,
        }
    }
    tray::destroy();
}
