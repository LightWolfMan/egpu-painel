//! Ícone de bandeja em Win32 puro. Com a janela fechada o processo fica só aqui,
//! bloqueado em GetMessageW: zero polling, zero timers. Os eventos chegam por
//! callbacks do Windows (Event Log / PnP) e viram balões e troca de cor do ícone.

use crate::util::wide;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU8, Ordering};
use std::sync::Mutex;
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const CLASS: &str = "eGPUPainelTray";
const WM_TRAY: u32 = WM_APP + 1;
const WM_BALLOON: u32 = WM_APP + 2;
pub const WM_OPEN_REQ: u32 = WM_APP + 3;
const WM_ICON: u32 = WM_APP + 4;
const ID_OPEN: usize = 1;
const ID_QUIT: usize = 2;

static HWND_TRAY: AtomicIsize = AtomicIsize::new(0);
static PENDING: AtomicU8 = AtomicU8::new(0); // 1 abrir, 2 sair
pub static PANEL_OPEN: AtomicBool = AtomicBool::new(false);
pub static DOCK_ON: AtomicBool = AtomicBool::new(false);
static BALLOONS: Mutex<VecDeque<(String, String)>> = Mutex::new(VecDeque::new());
static TASKBAR_CREATED: AtomicIsize = AtomicIsize::new(0);

pub enum Cmd {
    Open,
    Quit,
}

fn hwnd() -> HWND {
    HWND(HWND_TRAY.load(Ordering::SeqCst) as _)
}

// ---------------------------------------------------------------------------
// Ícone gerado em código: dois nós ligados (note → dock). Cor do nó do dock = estado.

pub fn icon_rgba(size: u32, ok: bool) -> Vec<u8> {
    let s = size as f32;
    let mut px = vec![0u8; (size * size * 4) as usize];
    let dock = if ok { [63, 185, 80] } else { [248, 81, 73] };
    let cyan = [56, 189, 248];
    let bg = [22, 27, 34];
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // fundo: quadrado arredondado
            let r = s * 0.22;
            let qx = (fx - s / 2.0).abs() - (s / 2.0 - r);
            let qy = (fy - s / 2.0).abs() - (s / 2.0 - r);
            let d_bg = (qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0)) - r;
            let a_bg = (0.5 - d_bg).clamp(0.0, 1.0);
            let mut c = [bg[0] as f32, bg[1] as f32, bg[2] as f32];
            let blend = |c: &mut [f32; 3], col: [u8; 3], a: f32| {
                for i in 0..3 {
                    c[i] = c[i] * (1.0 - a) + col[i] as f32 * a;
                }
            };
            // ligação
            let (ax, ay, bx, by) = (s * 0.28, s * 0.66, s * 0.72, s * 0.34);
            let (vx, vy) = (bx - ax, by - ay);
            let t = (((fx - ax) * vx + (fy - ay) * vy) / (vx * vx + vy * vy)).clamp(0.0, 1.0);
            let dl = (fx - (ax + vx * t)).hypot(fy - (ay + vy * t)) - s * 0.055;
            blend(&mut c, [200, 210, 225], (0.5 - dl).clamp(0.0, 1.0));
            let dn = (fx - ax).hypot(fy - ay) - s * 0.15;
            blend(&mut c, cyan, (0.5 - dn).clamp(0.0, 1.0));
            let dd = (fx - bx).hypot(fy - by) - s * 0.17;
            blend(&mut c, dock, (0.5 - dd).clamp(0.0, 1.0));
            let i = ((y * size + x) * 4) as usize;
            px[i] = c[0] as u8;
            px[i + 1] = c[1] as u8;
            px[i + 2] = c[2] as u8;
            px[i + 3] = (a_bg * 255.0) as u8;
        }
    }
    px
}

fn make_hicon(ok: bool) -> HICON {
    // tamanho do ícone pequeno no DPI do sistema (16 px a 100%, 20 a 125%, 32 a 200%…)
    let size = unsafe {
        let dpi = windows::Win32::UI::HiDpi::GetDpiForSystem();
        windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXSMICON, dpi).max(16) as u32
    };
    let rgba = icon_rgba(size, ok);
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize);
        for i in 0..(size * size) as usize {
            // BGRA pré-multiplicado não é exigido para ícones; só troca R/B
            dst[i * 4] = rgba[i * 4 + 2];
            dst[i * 4 + 1] = rgba[i * 4 + 1];
            dst[i * 4 + 2] = rgba[i * 4];
            dst[i * 4 + 3] = rgba[i * 4 + 3];
        }
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let h = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        h
    }
}

fn nid() -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd(),
        uID: 1,
        ..Default::default()
    }
}

fn copy_into(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

fn tip_text() -> String {
    crate::i18n::apply(crate::config::reload().lang); // o painel (outro processo) pode ter trocado o idioma
    if DOCK_ON.load(Ordering::SeqCst) {
        crate::i18n::t("eGPU Painel — dock conectado", "eGPU Painel — dock connected").into()
    } else {
        crate::i18n::t("eGPU Painel — dock DESCONECTADO", "eGPU Painel — dock DISCONNECTED").into()
    }
}

fn add_icon() {
    let mut n = nid();
    n.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    n.uCallbackMessage = WM_TRAY;
    n.hIcon = make_hicon(DOCK_ON.load(Ordering::SeqCst));
    copy_into(&mut n.szTip, &tip_text());
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &n);
    }
}

fn update_icon() {
    let mut n = nid();
    n.uFlags = NIF_ICON | NIF_TIP;
    n.hIcon = make_hicon(DOCK_ON.load(Ordering::SeqCst));
    copy_into(&mut n.szTip, &tip_text());
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &n);
        let _ = DestroyIcon(n.hIcon);
    }
}

fn show_balloon(title: &str, text: &str) {
    crate::i18n::apply(crate::config::reload().lang);
    let mut n = nid();
    n.uFlags = NIF_INFO;
    n.dwInfoFlags = NIIF_WARNING;
    copy_into(&mut n.szInfoTitle, title);
    copy_into(&mut n.szInfo, text);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &n);
    }
}

fn menu(h: HWND) {
    crate::i18n::apply(crate::config::reload().lang); // menu montado a cada abertura, no idioma atual
    unsafe {
        let m = CreatePopupMenu().unwrap();
        let open = wide(crate::i18n::t("Abrir painel", "Open panel"));
        let quit = wide(crate::i18n::t("Sair", "Quit"));
        let _ = AppendMenuW(m, MF_STRING, ID_OPEN, PCWSTR(open.as_ptr()));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(m, MF_STRING, ID_QUIT, PCWSTR(quit.as_ptr()));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(h);
        let _ = TrackPopupMenu(m, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, Some(0), h, None);
        let _ = DestroyMenu(m);
    }
}

fn request(cmd: u8) {
    PENDING.store(cmd, Ordering::SeqCst);
    if PANEL_OPEN.load(Ordering::SeqCst) {
        // o painel está rodando o loop do winit: avisa por ele
        match cmd {
            1 => crate::ui::request_focus(),
            2 => crate::ui::request_close(),
            _ => {}
        }
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            match lp.0 as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => request(1),
                WM_RBUTTONUP | WM_CONTEXTMENU => menu(h),
                _ => {}
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            match wp.0 & 0xFFFF {
                ID_OPEN => request(1),
                ID_QUIT => request(2),
                _ => {}
            }
            LRESULT(0)
        }
        WM_OPEN_REQ => {
            request(1);
            LRESULT(0)
        }
        WM_BALLOON => {
            let item = BALLOONS.lock().unwrap().pop_front();
            if let Some((t, s)) = item {
                show_balloon(&t, &s);
            }
            LRESULT(0)
        }
        WM_ICON => {
            update_icon();
            LRESULT(0)
        }
        m if m as isize == TASKBAR_CREATED.load(Ordering::SeqCst) && m != 0 => {
            add_icon(); // Explorer reiniciou
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

pub fn create() {
    unsafe {
        let inst = GetModuleHandleW(None).unwrap_or_default();
        let cls = wide(CLASS);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: inst.into(),
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let tc = wide("TaskbarCreated");
        TASKBAR_CREATED.store(RegisterWindowMessageW(PCWSTR(tc.as_ptr())) as isize, Ordering::SeqCst);
        // janela oculta (não message-only, para receber TaskbarCreated)
        let h = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(cls.as_ptr()),
            PCWSTR(cls.as_ptr()),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(inst.into()),
            None,
        )
        .expect("janela da bandeja");
        HWND_TRAY.store(h.0 as isize, Ordering::SeqCst);
    }
    add_icon();
}

pub fn destroy() {
    unsafe {
        let n = nid();
        let _ = Shell_NotifyIconW(NIM_DELETE, &n);
    }
}

/// Bloqueia em GetMessageW até o usuário pedir algo. Sem timeout.
pub fn wait() -> Cmd {
    unsafe {
        let mut msg = MSG::default();
        loop {
            match PENDING.swap(0, Ordering::SeqCst) {
                1 => return Cmd::Open,
                2 => return Cmd::Quit,
                _ => {}
            }
            if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                return Cmd::Quit;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Depois que o painel fecha: consome um pedido pendente (ex.: "Sair" clicado com o painel aberto).
pub fn take_pending() -> Option<Cmd> {
    match PENDING.swap(0, Ordering::SeqCst) {
        1 => Some(Cmd::Open),
        2 => Some(Cmd::Quit),
        _ => None,
    }
}

pub fn notify(title: String, text: String) {
    BALLOONS.lock().unwrap().push_back((title, text));
    unsafe {
        let _ = PostMessageW(Some(hwnd()), WM_BALLOON, WPARAM(0), LPARAM(0));
    }
}

pub fn set_dock(on: bool) {
    if DOCK_ON.swap(on, Ordering::SeqCst) != on {
        unsafe {
            let _ = PostMessageW(Some(hwnd()), WM_ICON, WPARAM(0), LPARAM(0));
        }
    }
}

/// Segunda instância: pede para a primeira abrir o painel.
pub fn signal_existing() -> bool {
    unsafe {
        let cls = wide(CLASS);
        if let Ok(h) = FindWindowW(PCWSTR(cls.as_ptr()), PCWSTR::null()) {
            let _ = PostMessageW(Some(h), WM_OPEN_REQ, WPARAM(0), LPARAM(0));
            return true;
        }
    }
    false
}
