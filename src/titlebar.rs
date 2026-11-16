//! Barra de título nativa (Windows 11) na cor do tema, via DWM.
//! O modo claro/escuro da janela é do winit (ViewportCommand::SetTheme); aqui só a cor da
//! legenda e do texto. No Windows 10 essas duas chamadas falham: o erro é ignorado e a barra
//! fica a padrão.

use std::sync::Mutex;

use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR};

/// DWMWA_COLOR_DEFAULT: devolve a cor ao sistema.
const DEFAULT: u32 = 0xFFFF_FFFF;

/// Chave do último estado aplicado: (tema do painel, HWND, tema do sistema).
/// Reaplica só quando algum dos três muda (janela recriada ou Windows trocou de tema).
type Key = (u8, isize, Option<egui::Theme>);
static APPLIED: Mutex<Option<Key>> = Mutex::new(None);

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    // COLORREF = 0x00BBGGRR
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

/// HWND da janela do painel, direto do eframe (sem enumerar janelas).
fn hwnd(frame: &eframe::Frame) -> Option<HWND> {
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as _)),
        _ => None,
    }
}

/// Chamar a cada quadro: não faz nada enquanto tema, janela e tema do sistema não mudam.
/// `id` = Th as u8; `caption` = None devolve legenda e texto ao padrão (tema claro).
pub fn sync(ctx: &egui::Context, frame: &eframe::Frame, id: u8, caption: Option<(u8, u8, u8)>) {
    let Some(h) = hwnd(frame) else { return };
    let key = (id, h.0 as isize, ctx.system_theme());
    let mut applied = APPLIED.lock().unwrap();
    if *applied == Some(key) {
        return;
    }
    let (cap, text, theme) = match caption {
        Some((r, g, b)) => (rgb(r, g, b), rgb(255, 255, 255), egui::SystemTheme::Dark),
        None => (DEFAULT, DEFAULT, egui::SystemTheme::Light),
    };
    // modo claro/escuro (ícones dos botões, borda): pelo winit, que é o dono dele
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(theme));
    unsafe {
        let _ = DwmSetWindowAttribute(h, DWMWA_CAPTION_COLOR, &cap as *const u32 as *const _, 4);
        let _ = DwmSetWindowAttribute(h, DWMWA_TEXT_COLOR, &text as *const u32 as *const _, 4);
    }
    *applied = Some(key);
}
