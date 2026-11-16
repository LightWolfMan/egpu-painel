//! Barra de título nativa (Windows 11) na cor do tema, via DWM.
//! No Windows 10 as chamadas de cor falham: o erro é ignorado e a barra fica a padrão.

use std::sync::atomic::{AtomicIsize, AtomicU8, Ordering};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindow};

const TITLE: &str = "eGPU Painel";
/// DWMWA_COLOR_DEFAULT: devolve a cor ao sistema.
const DEFAULT: u32 = 0xFFFF_FFFF;

static HWND_CACHE: AtomicIsize = AtomicIsize::new(0);
/// Último tema aplicado (255 = nenhum ainda).
static APPLIED: AtomicU8 = AtomicU8::new(255);

unsafe extern "system" fn find(h: HWND, lp: LPARAM) -> BOOL {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    if pid == unsafe { GetCurrentProcessId() } {
        let mut buf = [0u16; 64];
        let n = unsafe { GetWindowTextW(h, &mut buf) }.max(0) as usize;
        if String::from_utf16_lossy(&buf[..n]) == TITLE {
            unsafe { *(lp.0 as *mut isize) = h.0 as isize };
            return BOOL(0);
        }
    }
    BOOL(1)
}

/// HWND da janela do painel neste processo (em cache enquanto ela existir).
fn own_window() -> Option<HWND> {
    let c = HWND_CACHE.load(Ordering::Relaxed);
    if c != 0 && unsafe { IsWindow(Some(HWND(c as _))) }.as_bool() {
        return Some(HWND(c as _));
    }
    let mut out: isize = 0;
    let _ = unsafe { EnumWindows(Some(find), LPARAM(&mut out as *mut isize as isize)) };
    if out == 0 {
        return None;
    }
    if out != c {
        // janela nova (ou recriada): reaplica a cor
        APPLIED.store(255, Ordering::Relaxed);
    }
    HWND_CACHE.store(out, Ordering::Relaxed);
    Some(HWND(out as _))
}

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    // COLORREF = 0x00BBGGRR
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

/// Aplica a cor da legenda quando o tema muda (ou a janela aparece). `id` = Th as u8.
/// `caption` = None devolve legenda e texto ao padrão do sistema (tema claro).
pub fn sync(id: u8, caption: Option<(u8, u8, u8)>) {
    if APPLIED.load(Ordering::Relaxed) == id && HWND_CACHE.load(Ordering::Relaxed) != 0 {
        // confere a cada quadro só o cache, sem enumerar janelas
        let c = HWND_CACHE.load(Ordering::Relaxed);
        if unsafe { IsWindow(Some(HWND(c as _))) }.as_bool() {
            return;
        }
    }
    let Some(h) = own_window() else { return };
    if APPLIED.load(Ordering::Relaxed) == id {
        return;
    }
    let (cap, text, dark) = match caption {
        Some((r, g, b)) => (rgb(r, g, b), rgb(255, 255, 255), 1i32),
        None => (DEFAULT, DEFAULT, 0i32),
    };
    unsafe {
        let _ = DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const i32 as *const _, 4);
        let _ = DwmSetWindowAttribute(h, DWMWA_CAPTION_COLOR, &cap as *const u32 as *const _, 4);
        let _ = DwmSetWindowAttribute(h, DWMWA_TEXT_COLOR, &text as *const u32 as *const _, 4);
    }
    APPLIED.store(id, Ordering::Relaxed);
}
