//! Interface egui/eframe (renderizador glow). Foco na placa do dock: topologia animada,
//! enlace USB4/TB, switch PCIe e portas, PD, estabilidade e linha do tempo de quedas.

use crate::bench::BenchResult;
use crate::collect::{self, Sample};
use crate::history::{self, Cat, Ev, Kind};
use crate::pnp::{self, DockPort, Node, PortRole, Topology};
use crate::util::{fmt_ago, fmt_local, now_ms};
use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Paleta


/// Paleta com tema claro/escuro. As cores de destaque escurecem no tema claro para manter contraste.
mod pal {
    use eframe::egui::Color32;
    use std::sync::atomic::{AtomicBool, Ordering};
    pub static LIGHT: AtomicBool = AtomicBool::new(false);
    fn l() -> bool {
        LIGHT.load(Ordering::Relaxed)
    }
    const fn c(r: u8, g: u8, b: u8) -> Color32 {
        Color32::from_rgb(r, g, b)
    }
    macro_rules! tone {
        ($name:ident, $light:expr, $dark:expr) => {
            pub fn $name() -> Color32 {
                if l() { $light } else { $dark }
            }
        };
    }
    tone!(bg, c(242, 244, 247), c(13, 17, 23));
    tone!(card, c(255, 255, 255), c(22, 27, 34));
    tone!(card2, c(244, 246, 249), c(30, 36, 45));
    tone!(card3, c(235, 242, 252), c(26, 33, 44));
    tone!(well, c(246, 248, 251), c(16, 20, 26));
    tone!(grid, c(226, 230, 236), c(33, 39, 47));
    tone!(border, c(214, 220, 227), c(48, 54, 61));
    tone!(fg, c(28, 33, 40), c(230, 237, 243));
    tone!(dim, c(90, 99, 110), c(139, 148, 158));
    tone!(faint, c(150, 158, 168), c(88, 96, 105));
    tone!(cyan, c(2, 132, 199), c(56, 189, 248));
    tone!(green, c(26, 127, 55), c(63, 185, 80));
    tone!(amber, c(176, 112, 0), c(210, 153, 34));
    tone!(red, c(207, 34, 46), c(248, 81, 73));
    tone!(violet, c(130, 80, 223), c(163, 113, 247));
    tone!(amd, c(200, 30, 40), c(237, 70, 70));
    tone!(intel, c(0, 104, 181), c(64, 140, 230));
    tone!(bolt, c(181, 136, 0), c(245, 196, 0));
    tone!(pcie, c(100, 125, 160), c(120, 150, 190));
}

fn cat_color(c: Cat) -> Color32 {
    match c {
        Cat::Dock => pal::cyan(),
        Cat::Gpu => pal::amd(),
        Cat::Pcie => pal::violet(),
        Cat::Tdr => pal::red(),
        Cat::Energia => pal::bolt(),
        Cat::UsbA => pal::green(),
        Cat::Sistema => pal::dim(),
        Cat::App => pal::faint(),
    }
}

fn sev_color(s: u8) -> Color32 {
    match s {
        2 => pal::red(),
        1 => pal::amber(),
        _ => pal::dim(),
    }
}

// ---------------------------------------------------------------------------
// Ponte com a bandeja

static UI_CTX: Mutex<Option<egui::Context>> = Mutex::new(None);

pub fn request_focus() {
    if let Some(c) = UI_CTX.lock().unwrap().as_ref() {
        c.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        c.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        c.send_viewport_cmd(egui::ViewportCommand::Focus);
        c.request_repaint();
    }
}

pub fn request_close() {
    if let Some(c) = UI_CTX.lock().unwrap().as_ref() {
        c.send_viewport_cmd(egui::ViewportCommand::Close);
        c.request_repaint();
    }
}

pub fn run() {
    let icon = egui::IconData { rgba: crate::tray::icon_rgba(64, crate::tray::DOCK_ON.load(std::sync::atomic::Ordering::SeqCst)), width: 64, height: 64 };
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("eGPU Painel")
            .with_app_id("egpu-painel")
            .with_inner_size([1360.0, 800.0])
            .with_min_inner_size([980.0, 640.0])
            .with_icon(Arc::new(icon)),
        renderer: eframe::Renderer::Glow,
        run_and_return: true,
        persist_window: false,
        ..Default::default()
    };
    let r = eframe::run_native("eGPU Painel", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))));
    if let Err(e) = r {
        crate::history::note(Kind::Note, format!("Falha ao abrir a janela: {e}"));
    }
    *UI_CTX.lock().unwrap() = None;
}

// ---------------------------------------------------------------------------

#[derive(Clone)]
enum BenchState {
    Idle,
    Running(String),
    Done(BenchResult),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Sel {
    Info(Topic),
    Host,
    Cable,
    Dock,
    Port(usize),
    Gpu,
}

#[derive(Clone, Copy, PartialEq)]
enum Range {
    H24,
    D7,
    D30,
    All,
}

impl Range {
    fn ms(self) -> i64 {
        match self {
            Range::H24 => 86_400_000,
            Range::D7 => 7 * 86_400_000,
            Range::D30 => 30 * 86_400_000,
            Range::All => i64::MAX / 4,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Range::H24 => "24 h",
            Range::D7 => "7 dias",
            Range::D30 => "30 dias",
            Range::All => "tudo",
        }
    }
}

struct Incident {
    start: i64,
    end: i64,
    events: Vec<Ev>,
    sev: u8,
    is_drop: bool,
    pd_lost: bool,
}

pub struct App {
    collector: Option<std::thread::JoinHandle<()>>,
    bench: Arc<Mutex<BenchState>>,
    sel: Option<Sel>,
    range: Range,
    cats: Vec<(Cat, bool)>,
    hist_len: usize,
    events: Vec<Ev>,
    incidents: Vec<Incident>,
    open_inc: Option<i64>,
    only_problems: bool,
    flash: Option<String>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        *UI_CTX.lock().unwrap() = Some(ctx.clone());
        style(&ctx);
        history::scan_overwatch();
        let last_bench = history::get()
            .snapshot()
            .into_iter()
            .rev()
            .find(|e| e.kind == Kind::Bench)
            .and_then(|e| parse_bench(&e.msg));
        Self {
            collector: Some(collect::start(ctx)),
            bench: Arc::new(Mutex::new(last_bench.map(BenchState::Done).unwrap_or(BenchState::Idle))),
            sel: None,
            range: Range::D7,
            cats: vec![
                (Cat::Dock, true),
                (Cat::Energia, true),
                (Cat::Pcie, true),
                (Cat::Tdr, true),
                (Cat::Gpu, true),
                (Cat::UsbA, true),
                (Cat::Sistema, true),
                (Cat::App, false),
            ],
            hist_len: usize::MAX,
            events: vec![],
            incidents: vec![],
            open_inc: None,
            only_problems: false,
            flash: None,
        }
    }

    fn refresh_history(&mut self) {
        let h = history::get();
        let n = h.len();
        if n == self.hist_len {
            return;
        }
        self.hist_len = n;
        self.events = h.snapshot();
        // incidentes: eventos a ≤ 20 s um do outro (switch e roteador do dock saem com segundos de diferença)
        let mut inc: Vec<Incident> = vec![];
        for e in self.events.iter().filter(|e| e.kind.cat() != Cat::App || e.kind == Kind::Bench) {
            match inc.last_mut() {
                Some(i) if e.ts - i.end <= 20_000 && e.kind != Kind::Bench => {
                    i.end = e.ts;
                    i.sev = i.sev.max(e.kind.severity());
                    i.is_drop |= e.kind.is_drop();
                    i.pd_lost |= e.kind == Kind::AcOff;
                    i.events.push(e.clone());
                }
                _ => inc.push(Incident {
                    start: e.ts,
                    end: e.ts,
                    sev: e.kind.severity(),
                    is_drop: e.kind.is_drop(),
                    pd_lost: e.kind == Kind::AcOff,
                    events: vec![e.clone()],
                }),
            }
        }
        self.incidents = inc;
    }

    fn start_bench(&self, ctx: &egui::Context) {
        let st = self.bench.clone();
        let ctx = ctx.clone();
        *st.lock().unwrap() = BenchState::Running("iniciando…".into());
        std::thread::spawn(move || {
            let p = |s: &str| {
                *st.lock().unwrap() = BenchState::Running(s.to_string());
                ctx.request_repaint();
            };
            let r = crate::bench::run(p);
            *st.lock().unwrap() = match r {
                Ok(r) => {
                    history::note(
                        Kind::Bench,
                        format!(
                            "host→GPU {:.2} GB/s · GPU→host {:.2} GB/s ({} MiB por medição, fila COPY D3D12)",
                            r.h2d_gbs, r.d2h_gbs, r.per_run_mb
                        ),
                    );
                    BenchState::Done(r)
                }
                Err(e) => BenchState::Failed(e),
            };
            ctx.request_repaint();
        });
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(h) = self.collector.take() {
            collect::stop(h);
        }
    }
}

fn parse_bench(msg: &str) -> Option<BenchResult> {
    let nums: Vec<f64> = msg
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .filter_map(|s| s.parse().ok())
        .collect();
    (nums.len() >= 2).then(|| BenchResult {
        adapter: String::new(),
        h2d_gbs: nums[0],
        d2h_gbs: nums[1],
        h2d_runs: vec![],
        d2h_runs: vec![],
        per_run_mb: 1024,
    })
}

fn fonts(ctx: &egui::Context) {
    let mut f = egui::FontDefinitions::default();
    let dir = std::path::Path::new(r"C:\Windows\Fonts");
    let mut front = vec![];
    for (name, file) in [("segoe", "segoeui.ttf"), ("segoe_sym", "seguisym.ttf")] {
        if let Ok(bytes) = std::fs::read(dir.join(file)) {
            f.font_data.insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
            front.push(name.to_string());
        }
    }
    if let Some(fam) = f.families.get_mut(&egui::FontFamily::Proportional) {
        // Segoe UI na frente; símbolos depois dos padrões (fallback para setas/bolinhas)
        if front.first().map(|s| s == "segoe").unwrap_or(false) {
            fam.insert(0, "segoe".into());
        }
        if front.iter().any(|s| s == "segoe_sym") {
            fam.push("segoe_sym".into());
        }
    }
    if let Some(fam) = f.families.get_mut(&egui::FontFamily::Monospace) {
        if front.iter().any(|s| s == "segoe_sym") {
            fam.push("segoe_sym".into());
        }
    }
    ctx.set_fonts(f);
}

fn style(ctx: &egui::Context) {
    fonts(ctx);
    apply_theme(ctx, pal::LIGHT.load(std::sync::atomic::Ordering::Relaxed));
}

fn apply_theme(ctx: &egui::Context, light: bool) {
    pal::LIGHT.store(light, std::sync::atomic::Ordering::Relaxed);
    let mut v = if light { egui::Visuals::light() } else { egui::Visuals::dark() };
    v.panel_fill = pal::bg();
    v.window_fill = pal::card();
    v.extreme_bg_color = pal::well();
    v.faint_bg_color = pal::card2();
    v.override_text_color = Some(pal::fg());
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, pal::border());
    v.widgets.inactive.bg_fill = pal::card2();
    v.widgets.inactive.weak_bg_fill = pal::card2();
    v.hyperlink_color = pal::cyan();
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(10.0, 5.0);
    });
}

/// Tema: config (0 = segue o Windows, 1 = claro, 2 = escuro).
fn want_light(ctx: &egui::Context) -> bool {
    match crate::config::get().theme {
        1 => true,
        2 => false,
        _ => ctx.system_theme().map(|t| t == egui::Theme::Light).unwrap_or(false),
    }
}

// ---------------------------------------------------------------------------
// Peças visuais

fn card<R>(ui: &mut Ui, title: &str, accent: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(pal::card())
        .stroke(Stroke::new(1.0, pal::border()))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(4.0, 16.0), Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::same(2), accent);
                ui.label(egui::RichText::new(title).size(15.0).strong().color(pal::fg()));
            });
            ui.add_space(6.0);
            add(ui)
        })
        .inner
}

fn kv(ui: &mut Ui, k: &str, v: impl Into<egui::WidgetText>) {
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(k).color(pal::dim()));
        ui.label(v);
    });
}

fn pill(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(text).color(color).size(12.0)).extend());
        });
}

fn big(ui: &mut Ui, value: String, unit: &str, label: &str, color: Color32) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label).color(pal::dim()).size(11.5));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.label(egui::RichText::new(value).size(22.0).strong().color(color));
            ui.label(egui::RichText::new(unit).size(12.0).color(pal::dim()));
        });
    });
}

/// Gráfico de linha simples (sem dependência extra).
fn chart(ui: &mut Ui, label: &str, series: &[(Color32, &str, Vec<f32>)], max: f32, unit: &str, h: f32) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(6), pal::well());
    let plot = rect.shrink2(vec2(6.0, 16.0)).translate(vec2(0.0, 6.0));
    for i in 0..=3 {
        let y = plot.bottom() - plot.height() * i as f32 / 3.0;
        p.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], Stroke::new(1.0, pal::grid()));
    }
    let maxv = series
        .iter()
        .flat_map(|s| s.2.iter().cloned())
        .fold(max, f32::max)
        .max(1e-3);
    p.text(rect.left_top() + vec2(8.0, 4.0), egui::Align2::LEFT_TOP, label, FontId::proportional(11.5), pal::dim());
    p.text(rect.right_top() + vec2(-8.0, 4.0), egui::Align2::RIGHT_TOP, format!("máx {maxv:.0} {unit}"), FontId::proportional(10.5), pal::faint());
    let n = collect::HISTORY as f32;
    let mut legend_x = rect.left() + 8.0;
    for (color, name, data) in series {
        if data.len() >= 2 {
            let off = n - data.len() as f32;
            let pts: Vec<Pos2> = data
                .iter()
                .enumerate()
                .map(|(i, v)| pos2(plot.left() + plot.width() * (off + i as f32) / (n - 1.0), plot.bottom() - plot.height() * (v / maxv).clamp(0.0, 1.0)))
                .collect();
            // área
            let mut mesh = egui::Mesh::default();
            for (i, pt) in pts.iter().enumerate() {
                mesh.colored_vertex(*pt, color.gamma_multiply(0.28));
                mesh.colored_vertex(pos2(pt.x, plot.bottom()), color.gamma_multiply(0.02));
                if i > 0 {
                    let b = (i as u32 - 1) * 2;
                    mesh.add_triangle(b, b + 1, b + 2);
                    mesh.add_triangle(b + 1, b + 3, b + 2);
                }
            }
            p.add(mesh);
            p.add(egui::Shape::line(pts.clone(), Stroke::new(1.6, *color)));
            let last = data.last().copied().unwrap_or(0.0);
            let txt = format!("{name} {last:.0}{unit}");
            let g = p.layout_no_wrap(txt, FontId::proportional(11.0), *color);
            let lw = g.size().x;
            p.galley(pos2(legend_x, rect.bottom() - 15.0), g, *color);
            legend_x += lw + 14.0;
        }
    }
    if data_empty(series) {
        p.text(rect.center(), egui::Align2::CENTER_CENTER, "aguardando amostras…", FontId::proportional(12.0), pal::faint());
    }
    let _ = resp;
}

fn data_empty(s: &[(Color32, &str, Vec<f32>)]) -> bool {
    s.iter().all(|x| x.2.len() < 2)
}

// ---------------------------------------------------------------------------
// Topologia animada

struct Flow {
    copy: f32,
    d3: f32,
    pd_on: bool,
    gpu: Option<crate::collect::GpuM>,
}

fn node_box(p: &egui::Painter, rect: Rect, title: &str, lines: &[String], accent: Color32, state: Color32, selected: bool, focus: bool) {
    let p = &p.with_clip_rect(rect.intersect(p.clip_rect()));
    let fill = if focus { pal::card3() } else { pal::card2() };
    p.rect_filled(rect, CornerRadius::same(10), fill);
    p.rect_stroke(rect, CornerRadius::same(10), Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { pal::cyan() } else { pal::border() }), StrokeKind::Inside);
    let bar = Rect::from_min_size(rect.min + vec2(1.0, 8.0), vec2(3.0, rect.height() - 16.0));
    p.rect_filled(bar, CornerRadius::same(2), accent);
    p.circle_filled(rect.right_top() + vec2(-12.0, 12.0), 4.5, state);
    p.text(rect.left_top() + vec2(12.0, 8.0), egui::Align2::LEFT_TOP, title, FontId::proportional(if focus { 15.0 } else { 13.5 }), pal::fg());
    let mut y = rect.top() + if focus { 29.0 } else { 27.0 };
    for l in lines {
        if y > rect.bottom() - 12.0 {
            break;
        }
        p.text(pos2(rect.left() + 12.0, y), egui::Align2::LEFT_TOP, l, FontId::proportional(11.0), pal::dim());
        y += 15.0;
    }
}

/// Pontinhos andando ao longo de uma polilinha (a "pulsação" do tráfego).
fn pulses(p: &egui::Painter, pts: &[Pos2], t: f64, intensity: f32, color: Color32, reverse: bool) {
    let mut lens = vec![0.0f32];
    for w in pts.windows(2) {
        lens.push(lens.last().unwrap() + w[0].distance(w[1]));
    }
    let total = *lens.last().unwrap();
    if total < 1.0 {
        return;
    }
    let spacing = 46.0 - 30.0 * intensity.clamp(0.0, 1.0);
    let speed = 30.0 + 220.0 * intensity.clamp(0.0, 1.0);
    let n = (total / spacing) as usize + 1;
    let base = (t as f32 * speed) % spacing;
    for i in 0..n {
        let mut d = base + i as f32 * spacing;
        if d > total {
            continue;
        }
        if reverse {
            d = total - d;
        }
        let k = lens.windows(2).position(|w| d >= w[0] && d <= w[1]).unwrap_or(0);
        let seg = (lens[k + 1] - lens[k]).max(1e-3);
        let pos = pts[k] + (pts[k + 1] - pts[k]) * ((d - lens[k]) / seg);
        let a = 0.35 + 0.65 * intensity.clamp(0.0, 1.0);
        p.circle_filled(pos, 2.2 + 1.6 * intensity, color.gamma_multiply(a));
    }
}

fn link(p: &egui::Painter, pts: &[Pos2], alive: bool, width: f32, color: Color32) {
    let c = if alive { color.gamma_multiply(0.45) } else { pal::red().gamma_multiply(0.5) };
    if alive {
        p.add(egui::Shape::line(pts.to_vec(), Stroke::new(width, c)));
    } else {
        for w in pts.windows(2) {
            p.extend(egui::Shape::dashed_line(&[w[0], w[1]], Stroke::new(width.min(2.0), c), 6.0, 5.0));
        }
    }
}

fn dname(n: &Node) -> String {
    n.dstate.map(pnp::dstate_name).unwrap_or("?").to_string()
}

fn topo_view(ui: &mut Ui, topo: &Topology, flow: &Flow, gpu_static: Option<&crate::collect::GpuStatic>, sel: &mut Option<Sel>, bench: Option<&BenchResult>) {
    let w = ui.available_width();
    let h = 236.0;
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let p = ui.painter_at(rect);
    let t = ui.input(|i| i.time);

    let dock_ok = topo.dock_router.is_some();
    let switch_ok = topo.dock_switch.is_some();
    let gpu_ok = topo.gpu.is_some();

    // geometria
    let host_w = (w * 0.17).clamp(150.0, 210.0);
    let bw = (w * 0.22).clamp(170.0, 240.0);
    let host = Rect::from_min_size(rect.left_top() + vec2(4.0, 82.0), vec2(host_w, 72.0));
    let bx = rect.right() - bw - 4.0;
    // o dock fica centrado no vão entre o note e os ramos, deixando espaço para os cabos
    let gap = bx - host.right();
    let dock_w = (gap * 0.52).clamp(190.0, 320.0);
    let dock = Rect::from_center_size(pos2(host.right() + gap * 0.5, rect.center().y), vec2(dock_w, 124.0));
    let gpu_r = Rect::from_min_size(pos2(bx, rect.top() + 2.0), vec2(bw, 70.0));
    let usb_r = Rect::from_min_size(pos2(bx, rect.top() + 83.0), vec2(bw, 70.0));
    let tb_r = Rect::from_min_size(pos2(bx, rect.top() + 164.0), vec2(bw, 70.0));

    // --- cabo TB (2 faixas) + PD voltando para o note
    let lanes = topo.dock_usb4.iter().find(|p| p.pid == 21).and_then(|p| p.val.as_u32()).unwrap_or(2).max(1);
    let a = pos2(host.right(), host.center().y);
    let b = pos2(dock.left(), dock.center().y);
    let intensity = (flow.copy.max(flow.d3 * 0.35) / 100.0).clamp(0.0, 1.0);
    for l in 0..lanes {
        let off = (l as f32 - (lanes as f32 - 1.0) / 2.0) * 9.0 - 6.0;
        let pts = [a + vec2(0.0, off), b + vec2(0.0, off)];
        link(&p, &pts, dock_ok, 3.0, pal::bolt());
        if dock_ok {
            pulses(&p, &pts, t + l as f64 * 0.37, intensity, pal::cyan(), false);
        }
    }
    // PD: energia do dock para o note (sentido inverso)
    let pd_pts = [a + vec2(0.0, 22.0), b + vec2(0.0, 22.0)];
    link(&p, &pd_pts, dock_ok && flow.pd_on, 1.5, pal::bolt());
    if dock_ok && flow.pd_on {
        pulses(&p, &pd_pts, t, 0.15, pal::bolt(), true);
    }
    let mid = pos2((a.x + b.x) / 2.0, a.y);
    let speed = topo.dock_usb4.iter().find(|p| p.pid == 20).and_then(|p| p.val.as_u32());
    let cable_txt = match speed {
        Some(0x8) => format!("cabo TB3 · {lanes}×10 Gb/s?"),
        Some(0x4) => format!("cabo TB3 · {lanes}×20 Gb/s"),
        Some(x) => format!("cabo TB3 · código {x}"),
        None if dock_ok => "cabo TB3".into(),
        None => "sem enlace".into(),
    };
    let narrow = b.x - a.x < 190.0;
    let cable_txt = if narrow { cable_txt.replace("cabo TB3 · ", "") } else { cable_txt };
    p.text(mid + vec2(0.0, -30.0), egui::Align2::CENTER_BOTTOM, cable_txt, FontId::proportional(12.0), if dock_ok { pal::bolt() } else { pal::red() });
    p.text(mid + vec2(0.0, 34.0), egui::Align2::CENTER_TOP, if flow.pd_on { if narrow { "PD 85 W" } else { "PD → note (85 W anunciado)" } } else { "PD: sem energia" }, FontId::proportional(11.0), if flow.pd_on { pal::bolt().gamma_multiply(0.8) } else { pal::amber() });
    let cable_hit = Rect::from_center_size(mid, vec2((b.x - a.x).max(10.0), 70.0));
    if ui.interact(cable_hit, ui.id().with("cabo"), Sense::click()).clicked() {
        *sel = Some(Sel::Cable);
    }

    // --- ramos do switch PCIe
    let ports: Vec<(&DockPort, Rect)> = topo
        .ports
        .iter()
        .map(|dp| {
            let r = match dp.role {
                PortRole::Gpu => gpu_r,
                PortRole::UsbA => usb_r,
                PortRole::TbExtra => tb_r,
                PortRole::Other => tb_r,
            };
            (dp, r)
        })
        .collect();
    let branch = |target: Rect, i: f32| -> Vec<Pos2> {
        let s = pos2(dock.right(), dock.center().y + (i - 1.0) * 26.0);
        let e = pos2(target.left(), target.center().y);
        let mx = (s.x + e.x) / 2.0;
        vec![s, pos2(mx, s.y), pos2(mx, e.y), e]
    };
    let gpu_pts = branch(gpu_r, 0.0);
    link(&p, &gpu_pts, gpu_ok && switch_ok, 2.5, pal::pcie());
    if gpu_ok && switch_ok {
        pulses(&p, &gpu_pts, t, intensity, pal::cyan(), false);
        if flow.d3 > 1.0 {
            pulses(&p, &gpu_pts, t + 0.5, (flow.d3 / 100.0) * 0.4, pal::amd(), true);
        }
    }
    let usb_alive = ports.iter().any(|(d, _)| d.role == PortRole::UsbA);
    let usb_pts = branch(usb_r, 1.0);
    link(&p, &usb_pts, usb_alive, 2.0, pal::pcie());
    if usb_alive && !topo.usb_a.is_empty() {
        pulses(&p, &usb_pts, t, 0.08, pal::green(), false);
    }
    let tb_pts = branch(tb_r, 2.0);
    p.extend(egui::Shape::dashed_line(&tb_pts, Stroke::new(1.5, pal::faint()), 5.0, 5.0));

    // --- nós
    let hostn = topo.host_router.as_ref();
    let host_lines = vec![format!("USB4 A73E · fw {}", hostn.and_then(|n| n.firmware.clone()).unwrap_or("?".into()))];
    node_box(&p, host, "💻 Notebook", &host_lines, pal::intel(), if hostn.is_some() { pal::green() } else { pal::amber() }, *sel == Some(Sel::Host), false);
    if ui.interact(host, ui.id().with("host"), Sense::click()).clicked() {
        *sel = Some(Sel::Host);
    }

    let dr = topo.dock_router.as_ref();
    let fw = dr.and_then(|n| n.firmware.clone()).unwrap_or("—".into());
    let model = topo.dock_usb4.iter().find(|p| p.pid == 10).map(|p| p.val.show()).unwrap_or("—".into());
    let mut dock_lines = vec![format!("JHL7440 · fw {fw}")];
    let _ = &model;
    match dr.and_then(|n| n.last_arrival) {
        Some(ts) => dock_lines.push(format!("🕒 {}", fmt_dur(now_ms() - ts))),
        None => dock_lines.push("⛔ não detectado".into()),
    }
    if let Some(br) = bench {
        dock_lines.push(format!("⇅ {:.2} / {:.2} GB/s", br.h2d_gbs, br.d2h_gbs));
    }
    node_box(&p, dock, "🔗 Dock WKG-L19", &dock_lines, pal::cyan(), if dock_ok && switch_ok { pal::green() } else { pal::red() }, *sel == Some(Sel::Dock), true);
    // bolinhas de estado das portas downstream (verde = D0 ativo, cinza = D3 dormindo)
    for (i, dp) in topo.ports.iter().enumerate() {
        let c = pos2(dock.left() + 22.0 + i as f32 * 34.0, dock.bottom() - 18.0);
        let on = dp.node.dstate == Some(1);
        let col = if dp.node.problem != 0 { pal::red() } else if on { pal::green() } else { pal::faint() };
        if on {
            let k = (t * 2.0 + i as f64).sin() as f32 * 0.5 + 0.5;
            p.circle_filled(c, 7.0 + 2.0 * k, col.gamma_multiply(0.18));
        }
        p.circle_filled(c, 5.0, col);
        let icon = match dp.role {
            PortRole::Gpu => "🎮",
            PortRole::UsbA => "🖴",
            _ => "⚡",
        };
        p.text(c + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, icon, FontId::proportional(10.0), pal::dim());
        let hit = Rect::from_center_size(c + vec2(6.0, 0.0), vec2(26.0, 18.0));
        ui.interact(hit, ui.id().with(("dport", i)), Sense::hover()).on_hover_text(format!("porta downstream {} · {}", i + 1, dname(&dp.node)));
    }
    if ui.interact(dock, ui.id().with("dock"), Sense::click()).clicked() {
        *sel = Some(Sel::Dock);
    }

    // GPU
    let gl = if let Some(g) = &topo.gpu {
        let gm = flow.gpu.unwrap_or_default();
        let _ = g;
        vec![
            format!("🌡 {:.0} °C   ⚡ {:.0} W", gm.temp.unwrap_or(0.0), gm.tbp.unwrap_or(0.0)),
            match gpu_static {
                Some(s) => format!("{} x{}", s.bus, s.lanes),
                None => "…".into(),
            },
        ]
    } else {
        vec!["não detectada".into()]
    };
    node_box(&p, gpu_r, "🎮 Radeon RX 7600 XT", &gl, pal::amd(), if gpu_ok { pal::green() } else { pal::red() }, *sel == Some(Sel::Gpu), false);
    if ui.interact(gpu_r, ui.id().with("gpu"), Sense::click()).clicked() {
        *sel = Some(Sel::Gpu);
    }
    for (i, (dp, r)) in ports.iter().enumerate() {
        if dp.role == PortRole::Gpu {
            continue;
        }
        let (title, lines, accent) = match dp.role {
            PortRole::UsbA => {
                let x = dp.child.as_ref();
                let _ = x;
                let mut l = vec![];
                if topo.usb_a.is_empty() {
                    l.push("vazia".into());
                } else {
                    for d in topo.usb_a.iter().take(2) {
                        l.push(format!("🖴 {}", d.name));
                    }
                }
                ("🖴 USB-A 10 Gb/s", l, pal::green())
            }
            _ => (
                "⚡ TB extra 15 W",
                vec![if dp.child.is_some() { "em uso".into() } else { "vazia".into() }],
                pal::faint(),
            ),
        };
        node_box(&p, *r, title, &lines, accent, if dp.node.problem == 0 { pal::green() } else { pal::red() }, *sel == Some(Sel::Port(i)), false);
        if ui.interact(*r, ui.id().with(("port", i)), Sense::click()).clicked() {
            *sel = Some(Sel::Port(i));
        }
    }
    if ports.iter().all(|(d, _)| d.role != PortRole::UsbA) {
        node_box(&p, usb_r, "Porta USB-A (10 Gb/s)", &["controlador não detectado".into()], pal::green(), pal::red(), false, false);
    }
    if ports.iter().all(|(d, _)| d.role != PortRole::TbExtra) {
        node_box(&p, tb_r, "Porta TB extra (15 W)", &["não detectada".into()], pal::faint(), pal::amber(), false, false);
    }
}

// ---------------------------------------------------------------------------

impl eframe::App for App {
    fn ui(&mut self, root: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        let wl = want_light(&ctx);
        if wl != pal::LIGHT.load(std::sync::atomic::Ordering::Relaxed) {
            apply_theme(&ctx, wl);
        }
        self.refresh_history();
        let (topo, samples, gpu_static, notes) = collect::with_live(|l| (l.topo.clone(), l.samples.iter().copied().collect::<Vec<Sample>>(), l.gpu_static.clone(), l.notes.clone()))
            .unwrap_or_default();
        let topo = topo.unwrap_or_default();
        let last = samples.last().copied().unwrap_or_default();
        let flow = Flow { copy: last.copy, d3: last.d3, pd_on: last.power.map(|p| p.online).unwrap_or(true), gpu: last.gpu };
        let bench = self.bench.lock().unwrap().clone();
        let bench_done = if let BenchState::Done(r) = &bench { Some(r.clone()) } else { None };

        // ---------------- cabeçalho
        egui::Panel::top("top").frame(egui::Frame::new().fill(pal::bg()).inner_margin(egui::Margin::symmetric(16, 10))).show_inside(root, |ui| {
            ui.horizontal(|ui| {
                logo(ui, topo.dock_router.is_some());
                ui.label(egui::RichText::new("eGPU Painel").size(20.0).strong().color(pal::fg()));
                ui.label(egui::RichText::new("WKG-L19 · JHL7440").color(pal::faint()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.settings_menu(ui);
                    let mut cfg = crate::config::get();
                    let (icon, tip) = match cfg.theme {
                        1 => ("☀", "tema claro"),
                        2 => ("🌙", "tema escuro"),
                        _ => ("◐", "tema do Windows"),
                    };
                    if ui.add(egui::Button::new(egui::RichText::new(icon).size(16.0)).frame(false)).on_hover_text(format!("{tip} — clique para trocar")).clicked() {
                        cfg.theme = (cfg.theme + 1) % 3;
                        crate::config::set(cfg);
                    }
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(format!("RAM {:.0} MB · CPU {:.1}%", last.ws_mb, last.cpu_pct)).color(pal::faint()).size(11.5))
                        .on_hover_text("Consumo do próprio painel (working set e CPU). Fechando a janela ele volta para a bandeja e para de coletar.");
                    ui.add_space(10.0);
                    let t = ui.input(|i| i.time);
                    match &topo.dock_router {
                        Some(n) => {
                            if status_pill(ui, &format!("dock conectado · {}", n.last_arrival.map(|a| fmt_ago(a).replace("há ", "")).unwrap_or_default()), pal::green(), t, false).clicked() {
                                self.sel = Some(Sel::Dock);
                            }
                        }
                        None if topo.read_ms == 0 => {
                            status_pill(ui, "lendo…", pal::dim(), t, false);
                        }
                        None => {
                            status_pill(ui, "dock DESCONECTADO", pal::red(), t, true);
                        }
                    }
                });
            });
        });

        // ---------------- painel lateral (detalhes / explicações / opções)
        if self.sel.is_some() {
            egui::Panel::right("detalhes")
                .resizable(true)
                .default_size(430.0)
                .max_size(560.0)
                .frame(egui::Frame::new().fill(pal::card()).inner_margin(egui::Margin::same(14)).stroke(Stroke::new(1.0, pal::border())))
                .show_inside(root, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                    egui::ScrollArea::vertical().show(ui, |ui| self.details(ui, &topo, gpu_static.as_ref(), &samples, bench_done.as_ref(), &ctx));
                });
        }

        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal::bg()).inner_margin(egui::Margin::symmetric(16, 4))).show_inside(root, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                card(ui, "🔗  Topologia", pal::cyan(), |ui| {
                    let mut sel = self.sel;
                    topo_view(ui, &topo, &flow, gpu_static.as_ref(), &mut sel, bench_done.as_ref());
                    if sel != self.sel {
                        self.sel = sel;
                    }
                    hidden_strip(ui, &mut self.sel);
                });
                ui.add_space(10.0);
                let cols = if ui.available_width() > 1150.0 { 3 } else { 2 };
                ui.columns(cols, |c| {
                    self.card_link(&mut c[0], &topo, bench_done.as_ref());
                    let (pi, si) = if cols == 3 { (1, 2) } else { (1, 1) };
                    self.card_power(&mut c[pi], &samples);
                    c[si].add_space(if cols == 3 { 0.0 } else { 10.0 });
                    self.card_stability(&mut c[si], &topo);
                });
                ui.add_space(10.0);
                ui.columns(2, |c| {
                    self.card_flow(&mut c[0], &samples, &bench, &ctx);
                    self.card_gpu(&mut c[1], &samples, gpu_static.as_ref());
                });
                ui.add_space(10.0);
                self.card_timeline(ui);
                for n in &notes {
                    ui.label(egui::RichText::new(format!("⚠ {n}")).color(pal::amber()).size(11.5));
                }
                ui.add_space(14.0);
            });
        });

        let busy = flow.copy > 2.0 || flow.d3 > 5.0 || matches!(bench, BenchState::Running(_));
        let eco = crate::config::get().eco;
        let ms = match (eco, busy) {
            (true, _) => 200,
            (false, true) => 33,
            (false, false) => 66,
        };
        ctx.request_repaint_after(Duration::from_millis(ms));
    }
}

// ---------------------------------------------------------------------------
// Peças visuais extras

fn logo(ui: &mut Ui, ok: bool) {
    let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
    let p = ui.painter_at(r);
    p.rect_filled(r, CornerRadius::same(7), pal::card2());
    let a = r.left_bottom() + vec2(8.0, -9.0);
    let b = r.right_top() + vec2(-8.0, 9.0);
    p.line_segment([a, b], Stroke::new(2.0, pal::dim()));
    p.circle_filled(a, 4.5, pal::cyan());
    p.circle_filled(b, 5.0, if ok { pal::green() } else { pal::red() });
}

fn status_pill(ui: &mut Ui, text: &str, color: Color32, t: f64, pulse: bool) -> egui::Response {
    let r = egui::Frame::new()
        .fill(color.gamma_multiply(0.15))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (d, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                let k = if pulse { (t * 3.0).sin() as f32 * 0.5 + 0.5 } else { (t * 1.2).sin() as f32 * 0.25 + 0.75 };
                ui.painter().circle_filled(d.center(), 6.0 * k, color.gamma_multiply(0.25));
                ui.painter().circle_filled(d.center(), 3.5, color);
                ui.label(egui::RichText::new(text).color(color).size(12.5));
            });
        })
        .response;
    ui.interact(r.rect, ui.id().with(("pill", text)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Bloco número-grande clicável.
fn tile(ui: &mut Ui, icon: &str, value: String, unit: &str, label: &str, color: Color32) -> egui::Response {
    let resp = egui::Frame::new()
        .fill(pal::card2())
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(format!("{icon} {label}")).color(pal::dim()).size(11.0));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    ui.label(egui::RichText::new(value).size(21.0).strong().color(color));
                    ui.label(egui::RichText::new(unit).size(11.5).color(pal::dim()));
                });
            });
        })
        .response;
    let r = ui.interact(resp.rect, ui.id().with(("tile", label)), Sense::click());
    if r.hovered() {
        ui.painter().rect_stroke(resp.rect, CornerRadius::same(8), Stroke::new(1.0, color.gamma_multiply(0.7)), StrokeKind::Inside);
    }
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn info_btn(ui: &mut Ui, sel: &mut Option<Sel>, topic: Topic, tip: &str) {
    if ui.add(egui::Button::new(egui::RichText::new("ⓘ").color(pal::dim())).frame(false)).on_hover_text(tip).clicked() {
        *sel = Some(Sel::Info(topic));
    }
}

/// Cartão com ícone de informação no título.
fn card_i<R>(ui: &mut Ui, title: &str, accent: Color32, sel: &mut Option<Sel>, topic: Topic, tip: &str, add: impl FnOnce(&mut Ui, &mut Option<Sel>) -> R) -> R {
    egui::Frame::new()
        .fill(pal::card())
        .stroke(Stroke::new(1.0, pal::border()))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(4.0, 16.0), Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::same(2), accent);
                ui.label(egui::RichText::new(title).size(15.0).strong().color(pal::fg()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| info_btn(ui, sel, topic, tip));
            });
            ui.add_space(6.0);
            add(ui, sel)
        })
        .inner
}

const HIDDEN: [(&str, &str, &str); 6] = [
    ("🔌", "Fonte ATX", "Tensões, temperatura e potência: a fonte não tem barramento de dados (PMBus) ligado ao dock."),
    ("🌀", "Ventoinha", "Sem sensor de RPM ligado a um controlador visível pelo host."),
    ("🌡", "Temp. JHL7440", "O controlador Titan Ridge não publica sensor térmico ao Windows."),
    ("⚡", "Contrato PD", "Tensão/corrente negociadas ficam no controlador PD do dock; o Windows só vê energia externa sim/não e a taxa de carga."),
    ("📶", "Tráfego real", "Nenhum contador de bytes por adaptador USB4/TB. O painel usa o motor Copy da GPU como proxy."),
    ("📉", "Erros do enlace", "Contadores de erro/retreino das faixas não são publicados. Só aparecem quedas (PnP) e erros PCIe (WHEA/AER)."),
];

fn hidden_strip(ui: &mut Ui, sel: &mut Option<Sel>) {
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("🚫").color(pal::faint()).size(12.0)).on_hover_text("Invisível ao Windows — passe o mouse em cada ícone");
        for (icon, name, why) in HIDDEN {
            let r = egui::Frame::new()
                .fill(pal::well())
                .stroke(Stroke::new(1.0, pal::border()))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::symmetric(8, 2))
                .show(ui, |ui| {
                    let g = ui.add(egui::Label::new(egui::RichText::new(icon.to_string()).color(pal::faint()).size(14.0)).extend());
                    // risco diagonal: "não disponível"
                    let rr = g.rect;
                    ui.painter().line_segment([rr.left_bottom(), rr.right_top()], Stroke::new(1.0, pal::red().gamma_multiply(0.45)));
                })
                .response;
            if ui.interact(r.rect, ui.id().with(("hid", name)), Sense::click()).on_hover_text(format!("{name}: {why}")).clicked() {
                *sel = Some(Sel::Info(Topic::Hidden));
            }
        }
    });
}

/// Faixas do enlace desenhadas: cada faixa é uma pista de 0–20 Gb/s (TB3).
fn lanes_view(ui: &mut Ui, lanes: u32, per_lane: Option<f32>, t: f64) -> egui::Response {
    let w = ui.available_width();
    let h = 18.0 * lanes.max(1) as f32 + 22.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let p = ui.painter_at(rect);
    let x0 = rect.left() + 58.0;
    let x1 = rect.right() - 70.0;
    for l in 0..lanes.max(1) {
        let y = rect.top() + 4.0 + l as f32 * 18.0;
        let track = Rect::from_min_max(pos2(x0, y), pos2(x1, y + 11.0));
        p.rect_filled(track, CornerRadius::same(5), pal::well());
        p.text(pos2(rect.left(), y + 5.5), egui::Align2::LEFT_CENTER, format!("faixa {}", l + 1), FontId::proportional(11.0), pal::dim());
        if let Some(v) = per_lane {
            let frac = (v / 20.0).clamp(0.0, 1.0);
            let fill = Rect::from_min_max(track.min, pos2(x0 + (x1 - x0) * frac, track.bottom()));
            let col = if frac < 0.99 { pal::amber() } else { pal::green() };
            p.rect_filled(fill, CornerRadius::same(5), col.gamma_multiply(0.85));
            // brilho correndo dentro da faixa
            let sx = fill.left() + ((t as f32 * 60.0 + l as f32 * 37.0) % fill.width().max(1.0));
            p.rect_filled(Rect::from_center_size(pos2(sx, fill.center().y), vec2(14.0, 9.0)), CornerRadius::same(4), Color32::WHITE.gamma_multiply(0.18));
            p.text(pos2(x1 + 8.0, y + 5.5), egui::Align2::LEFT_CENTER, format!("{v:.0}/20 Gb/s"), FontId::proportional(11.5), col);
        }
    }
    let total = per_lane.map(|v| v * lanes as f32);
    if let Some(t) = total {
        p.text(pos2(x1 + 8.0, rect.bottom() - 2.0), egui::Align2::LEFT_BOTTOM, format!("Σ {t:.0}/40"), FontId::proportional(11.0), pal::dim());
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn battery_icon(ui: &mut Ui, pct: f32, online: bool, charging: bool, t: f64) {
    let (r, _) = ui.allocate_exact_size(vec2(74.0, 38.0), Sense::hover());
    let p = ui.painter_at(r);
    let body = Rect::from_min_size(r.min + vec2(2.0, 4.0), vec2(58.0, 30.0));
    p.rect_stroke(body, CornerRadius::same(5), Stroke::new(2.0, pal::dim()), StrokeKind::Inside);
    p.rect_filled(Rect::from_min_size(pos2(body.right(), body.center().y - 6.0), vec2(4.0, 12.0)), CornerRadius::same(1), pal::dim());
    let inner = body.shrink(4.0);
    let col = if pct < 20.0 { pal::red() } else if online { pal::green() } else { pal::amber() };
    let fill = Rect::from_min_size(inner.min, vec2(inner.width() * (pct / 100.0).clamp(0.0, 1.0), inner.height()));
    p.rect_filled(fill, CornerRadius::same(3), col.gamma_multiply(0.8));
    if charging {
        let sx = inner.left() + ((t as f32 * 30.0) % inner.width());
        p.rect_filled(Rect::from_center_size(pos2(sx, inner.center().y), vec2(8.0, inner.height())), CornerRadius::same(2), Color32::WHITE.gamma_multiply(0.2));
    }
    if online {
        p.text(body.center(), egui::Align2::CENTER_CENTER, "⚡", FontId::proportional(16.0), pal::bolt());
    }
}

/// Quedas por dia (14 dias): vermelho = só enlace, âmbar = PD caiu junto.
fn drops_chart(ui: &mut Ui, incidents: &[&Incident]) -> egui::Response {
    use chrono::{Local, TimeZone};
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 64.0), Sense::click());
    let p = ui.painter_at(rect);
    let days = 14i64;
    let today = Local::now().date_naive();
    let mut buckets = vec![(0u32, 0u32); days as usize];
    for i in incidents {
        if let Some(d) = Local.timestamp_millis_opt(i.start).single() {
            let age = (today - d.date_naive()).num_days();
            if (0..days).contains(&age) {
                let b = &mut buckets[(days - 1 - age) as usize];
                if i.pd_lost { b.1 += 1 } else { b.0 += 1 }
            }
        }
    }
    let max = buckets.iter().map(|b| b.0 + b.1).max().unwrap_or(0).max(1) as f32;
    let bw = rect.width() / days as f32;
    for (k, (link, pd)) in buckets.iter().enumerate() {
        let x = rect.left() + k as f32 * bw;
        let base = rect.bottom() - 12.0;
        let hmax = rect.height() - 16.0;
        let h1 = hmax * (*pd as f32 / max);
        let h2 = hmax * (*link as f32 / max);
        p.rect_filled(Rect::from_min_max(pos2(x + 2.0, base - 1.0), pos2(x + bw - 2.0, base)), CornerRadius::ZERO, pal::grid());
        if *pd > 0 {
            p.rect_filled(Rect::from_min_max(pos2(x + 2.0, base - h1), pos2(x + bw - 2.0, base)), CornerRadius::same(2), pal::amber());
        }
        if *link > 0 {
            p.rect_filled(Rect::from_min_max(pos2(x + 2.0, base - h1 - h2), pos2(x + bw - 2.0, base - h1)), CornerRadius::same(2), pal::red());
        }
        if link + pd > 0 {
            p.text(pos2(x + bw / 2.0, base - h1 - h2 - 1.0), egui::Align2::CENTER_BOTTOM, format!("{}", link + pd), FontId::proportional(10.0), pal::dim());
        }
    }
    p.text(rect.left_bottom(), egui::Align2::LEFT_BOTTOM, "14 dias atrás", FontId::proportional(9.5), pal::faint());
    p.text(rect.right_bottom(), egui::Align2::RIGHT_BOTTOM, "hoje", FontId::proportional(9.5), pal::faint());
    resp.on_hover_text("Quedas por dia. Âmbar: o PD caiu junto (o dock inteiro apagou). Vermelho: só o enlace caiu.")
}

/// Medidor circular (arco de 270 graus) com valor animado.
fn gauge(ui: &mut Ui, icon: &str, value: f32, max: f32, unit: &str, label: &str, color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(96.0, 92.0), Sense::click());
    let v = ui.ctx().animate_value_with_time(ui.id().with(("g", label)), value, 0.45);
    let p = ui.painter_at(rect);
    let c = pos2(rect.center().x, rect.top() + 44.0);
    let r = 34.0;
    let a0 = 135f32.to_radians();
    let sweep = 270f32.to_radians();
    let arc = |frac: f32| -> Vec<Pos2> {
        let n = ((56.0 * frac) as usize).max(2);
        (0..=n)
            .map(|i| {
                let a = a0 + sweep * frac * i as f32 / n as f32;
                c + vec2(a.cos(), a.sin()) * r
            })
            .collect()
    };
    p.add(egui::Shape::line(arc(1.0), Stroke::new(7.0, pal::grid())));
    let frac = (v / max).clamp(0.0, 1.0);
    if frac > 0.004 {
        p.add(egui::Shape::line(arc(frac), Stroke::new(7.0, color)));
        let a = a0 + sweep * frac;
        p.circle_filled(c + vec2(a.cos(), a.sin()) * r, 4.5, color);
    }
    p.text(c + vec2(0.0, -3.0), egui::Align2::CENTER_CENTER, format!("{v:.0}"), FontId::proportional(19.0), pal::fg());
    p.text(c + vec2(0.0, 14.0), egui::Align2::CENTER_CENTER, unit, FontId::proportional(10.5), pal::dim());
    p.text(pos2(c.x, rect.bottom() - 1.0), egui::Align2::CENTER_BOTTOM, format!("{icon} {label}"), FontId::proportional(11.0), pal::dim());
    if resp.hovered() {
        p.circle_stroke(c, r + 7.0, Stroke::new(1.0, color.gamma_multiply(0.4)));
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn kind_icon(k: Kind) -> &'static str {
    use Kind::*;
    match k {
        DockOn => "🔗",
        DockOff => "⏏",
        DockFail => "💥",
        GpuOn => "🎮",
        GpuOff => "🎮",
        UsbAOn | UsbAOff | UsbADev => "🖴",
        PcieErr | Whea => "⚠",
        PcieInfo => "↯",
        Tdr | TdrGame => "❄",
        GpuDriver => "🛠",
        AcOn => "⚡",
        AcOff => "🔋",
        Sleep | StandbyIn => "🌙",
        Wake | StandbyOut => "☀",
        Boot => "⏻",
        Shutdown => "⏼",
        Crash => "⚠",
        Bench => "📏",
        Note => "ℹ",
    }
}

fn chip(ui: &mut Ui, on: bool, text: &str, color: Color32) -> bool {
    let fill = if on { color.gamma_multiply(0.22) } else { Color32::TRANSPARENT };
    let stroke = if on { color.gamma_multiply(0.8) } else { pal::border() };
    let tc = if on { color } else { pal::faint() };
    let r = egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| ui.add(egui::Label::new(egui::RichText::new(text).color(tc).size(12.0)).extend()))
        .response;
    ui.interact(r.rect, ui.id().with(("chip", text)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

// ---------------------------------------------------------------------------
// Cards

impl App {
    fn settings_menu(&mut self, ui: &mut Ui) {
        ui.menu_button(egui::RichText::new("⚙").size(16.0), |ui| {
            ui.set_min_width(260.0);
            ui.label(egui::RichText::new("Opções").strong());
            let mut cfg = crate::config::get();
            let mut auto = crate::config::autostart_enabled();
            if ui.checkbox(&mut auto, "🚀 Iniciar com o Windows (na bandeja)").changed() {
                if let Err(e) = crate::config::set_autostart(auto) {
                    self.flash = Some(format!("Não consegui alterar a inicialização: {e}"));
                }
            }
            let before = cfg;
            ui.checkbox(&mut cfg.notify, "🔔 Avisar na bandeja quando o dock cair");
            ui.checkbox(&mut cfg.eco, "🍃 Animação econômica (5 qps)");
            if cfg != before {
                crate::config::set(cfg);
            }
            ui.separator();
            if ui.button("📄 Exportar linha do tempo (CSV)").clicked() {
                match crate::config::export_csv() {
                    Ok(p) => {
                        crate::config::open_folder(p.parent().unwrap_or(&p));
                        self.flash = Some(format!("Exportado: {}", p.display()));
                    }
                    Err(e) => self.flash = Some(format!("Falha ao exportar: {e}")),
                }
                ui.close();
            }
            if ui.button("📂 Abrir pasta de dados").clicked() {
                crate::config::open_folder(&crate::util::data_dir());
                ui.close();
            }
            if ui.button("📏 Rodar teste de banda").clicked() {
                self.start_bench(ui.ctx());
                ui.close();
            }
        });
    }

    fn card_link(&mut self, ui: &mut Ui, topo: &Topology, bench: Option<&BenchResult>) {
        let mut sel = self.sel;
        card_i(ui, "⚡  Enlace Thunderbolt", pal::bolt(), &mut sel, Topic::Link, "Como a velocidade foi decodificada", |ui, sel| {
            let Some(dr) = &topo.dock_router else {
                ui.label(egui::RichText::new("⛓ enlace desconectado").color(pal::red()).size(16.0));
                return;
            };
            let speed = topo.dock_usb4.iter().find(|p| p.pid == 20).and_then(|p| p.val.as_u32());
            let lanes = topo.dock_usb4.iter().find(|p| p.pid == 21).and_then(|p| p.val.as_u32()).unwrap_or(2);
            let per_lane = match speed {
                Some(8) => Some(10.0),
                Some(4) => Some(20.0),
                Some(2) => Some(40.0),
                _ => None,
            };
            let t = ui.input(|i| i.time);
            if lanes_view(ui, lanes, per_lane, t).on_hover_text("Chaves USB4 20 (velocidade) e 21 (faixas). Clique para ver a decodificação.").clicked() {
                *sel = Some(Sel::Info(Topic::Link));
            }
            if speed == Some(8) {
                let low = bench.map(|b| b.h2d_gbs < 2.2).unwrap_or(false);
                let txt = if low { "⚠ metade da velocidade" } else { "⚠ metade da velocidade?" };
                if chip(ui, true, txt, pal::amber()) {
                    *sel = Some(Sel::Info(Topic::Link));
                }
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if tile(ui, "💾", dr.firmware.clone().unwrap_or("?".into()), "", "firmware", pal::fg()).on_hover_text("Firmware do roteador Titan Ridge do dock").clicked() {
                    *sel = Some(Sel::Dock);
                }
                if let Some(ts) = dr.last_arrival {
                    if tile(ui, "🕒", fmt_dur(now_ms() - ts), "", "conectado há", pal::green()).on_hover_text(format!("desde {}", fmt_local(ts, false))).clicked() {
                        *sel = Some(Sel::Info(Topic::Stability));
                    }
                }
                if tile(ui, "🔀", dname(dr).split(' ').next().unwrap_or("?").to_string(), "", "estado", pal::cyan())
                    .on_hover_text("Estado de energia do roteador do dock (D0 ativo, D3 economia — o USB4 dorme quando ocioso)")
                    .clicked()
                {
                    *sel = Some(Sel::Dock);
                }
            });
        });
        self.sel = sel;
    }

    fn card_power(&mut self, ui: &mut Ui, samples: &[Sample]) {
        let mut sel = self.sel;
        card_i(ui, "🔌  Energia PD", pal::bolt(), &mut sel, Topic::Power, "O que dá e o que não dá para saber do PD", |ui, sel| {
            let Some(pw) = samples.last().and_then(|s| s.power) else {
                ui.spinner();
                return;
            };
            let pct = if pw.full_mwh > 0 { pw.rem_mwh as f32 / pw.full_mwh as f32 * 100.0 } else { 0.0 };
            let t = ui.input(|i| i.time);
            ui.horizontal(|ui| {
                battery_icon(ui, pct, pw.online, pw.charging, t);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(format!("{pct:.0}%")).size(22.0).strong().color(pal::fg()));
                    ui.label(egui::RichText::new(if pw.online { "🔌 PD" } else { "🔋 bateria" }).color(if pw.online { pal::green() } else { pal::amber() }).size(13.0));
                });
            });
            ui.horizontal_wrapped(|ui| {
                let w = if pw.charge_mw > 0 { pw.charge_mw as f64 / 1000.0 } else { -(pw.discharge_mw as f64) / 1000.0 };
                let wtxt = if w.abs() < 0.05 { "0".into() } else { format!("{w:+.1}") };
                if tile(ui, "🔋", wtxt, "W", "carga", if w >= 0.0 { pal::bolt() } else { pal::amber() }).on_hover_text("Taxa de carga (+) ou descarga (−) da bateria — WMI root\\wmi BatteryStatus").clicked() {
                    *sel = Some(Sel::Info(Topic::Power));
                }
                if tile(ui, "〰", format!("{:.2}", pw.mv as f64 / 1000.0), "V", "tensão", pal::fg()).on_hover_text("Tensão da bateria (não é a tensão do PD)").clicked() {
                    *sel = Some(Sel::Info(Topic::Power));
                }
                if tile(ui, "⚡", "85".into(), "W", "PD anunciado", pal::faint()).on_hover_text("Valor do DROM do dock (\"Module 2 PD 85W\"). O contrato negociado não é exposto.").clicked() {
                    *sel = Some(Sel::Info(Topic::Power));
                }
            });
            let series: Vec<f32> = samples.iter().filter_map(|s| s.power).map(|p| (p.charge_mw - p.discharge_mw) as f32 / 1000.0).collect();
            chart(ui, "carga/descarga (W)", &[(pal::bolt(), "W", series)], 10.0, " W", 58.0);
        });
        self.sel = sel;
    }

    fn card_stability(&mut self, ui: &mut Ui, topo: &Topology) {
        let mut sel = self.sel;
        let incidents = &self.incidents;
        card_i(ui, "🛡  Estabilidade do dock", pal::red(), &mut sel, Topic::Stability, "Como as quedas são classificadas", |ui, sel| {
            let now = now_ms();
            let drops: Vec<&Incident> = incidents.iter().filter(|i| i.is_drop && i.events.iter().any(|e| e.kind.cat() == Cat::Dock || e.kind == Kind::GpuOff)).collect();
            let count = |ms: i64| drops.iter().filter(|i| now - i.start <= ms).count();
            ui.horizontal_wrapped(|ui| {
                let c24 = count(86_400_000);
                for (icon, v, l, c) in [("💥", c24, "24 h", if c24 > 0 { pal::red() } else { pal::green() }), ("📅", count(7 * 86_400_000), "7 dias", pal::fg()), ("🗓", count(30 * 86_400_000), "30 dias", pal::fg())] {
                    if tile(ui, icon, v.to_string(), "", &format!("quedas {l}"), c).clicked() {
                        self_open_timeline(sel);
                    }
                }
            });
            if drops_chart(ui, &drops).clicked() {
                *sel = Some(Sel::Info(Topic::Stability));
            }
            let d30: Vec<&&Incident> = drops.iter().filter(|i| now - i.start <= 30 * 86_400_000).collect();
            if !d30.is_empty() {
                let pd = d30.iter().filter(|i| i.pd_lost).count();
                ui.horizontal_wrapped(|ui| {
                    if chip(ui, true, &format!("🔋 {pd}/{} com PD caindo junto", d30.len()), pal::amber()) {
                        *sel = Some(Sel::Info(Topic::Stability));
                    }
                    if let Some(ts) = topo.dock_router.as_ref().and_then(|n| n.last_arrival) {
                        let _ = chip(ui, true, &format!("🕒 sem cair há {}", fmt_dur(now - ts)), pal::green());
                    }
                });
            }
        });
        self.sel = sel;
    }

    fn card_flow(&mut self, ui: &mut Ui, samples: &[Sample], bench: &BenchState, ctx: &egui::Context) {
        let mut sel = self.sel;
        let mut run = false;
        card_i(ui, "📶  Fluxo de dados no túnel", pal::cyan(), &mut sel, Topic::Flow, "Por que o motor Copy é o proxy do tráfego", |ui, sel| {
            let last = samples.last().copied().unwrap_or_default();
            ui.horizontal(|ui| {
                if gauge(ui, "⇄", last.copy, 100.0, "%", "Copy", pal::cyan()).on_hover_text("Motor Copy: cópias DMA host↔GPU — o que mais passa pelo túnel").clicked() {
                    *sel = Some(Sel::Info(Topic::Flow));
                }
                if gauge(ui, "△", last.d3, 100.0, "%", "3D", pal::amd()).clicked() {
                    *sel = Some(Sel::Info(Topic::Flow));
                }
                ui.vertical(|ui| {
                    ui.add_space(14.0);
                    ui.add(egui::ProgressBar::new((last.ded_mb as f32 / 16368.0).clamp(0.0, 1.0)).desired_width(ui.available_width()).text(format!("🗃 VRAM {:.0} MB", last.ded_mb)).fill(pal::amd().gamma_multiply(0.7)))
                        .on_hover_text("Memória dedicada da GPU em uso (PDH GPU Adapter Memory)");
                    ui.add_space(6.0);
                    ui.add(egui::ProgressBar::new((last.shared_mb as f32 / 8192.0).clamp(0.0, 1.0)).desired_width(ui.available_width()).text(format!("🔁 compartilhada {:.0} MB", last.shared_mb)).fill(pal::cyan().gamma_multiply(0.7)))
                        .on_hover_text("RAM do note mapeada pela GPU — cada acesso atravessa o túnel TB");
                });
            });
            let copy: Vec<f32> = samples.iter().map(|s| s.copy).collect();
            let d3: Vec<f32> = samples.iter().map(|s| s.d3).collect();
            chart(ui, "motores (5 min)", &[(pal::cyan(), "Copy", copy), (pal::amd(), "3D", d3)], 100.0, "%", 84.0);
            ui.horizontal(|ui| {
                let running = matches!(bench, BenchState::Running(_));
                if ui.add_enabled(!running, egui::Button::new("📏 Teste de banda")).on_hover_text("Cópias D3D12 de 1 GiB na fila COPY, host↔VRAM (~5 s).").clicked() {
                    run = true;
                }
                match bench {
                    BenchState::Running(s) => {
                        ui.spinner();
                        ui.label(egui::RichText::new(s).color(pal::dim()).size(12.0));
                    }
                    BenchState::Failed(e) => {
                        ui.label(egui::RichText::new(e).color(pal::red()).size(12.0));
                    }
                    _ => {}
                }
            });
            if let BenchState::Done(r) = bench {
                bars(ui, r);
            }
        });
        if run {
            self.start_bench(ctx);
        }
        self.sel = sel;
    }

    fn card_gpu(&mut self, ui: &mut Ui, samples: &[Sample], st: Option<&crate::collect::GpuStatic>) {
        let mut sel = self.sel;
        card_i(ui, "🎮  GPU no dock", pal::amd(), &mut sel, Topic::Gpu, "Dados via AMD ADLX", |ui, sel| {
            let g = samples.last().and_then(|s| s.gpu).unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                let temp = g.temp.unwrap_or(0.0) as f32;
                let hot = g.hot.unwrap_or(0.0) as f32;
                let heat = |t: f32| if t > 85.0 { pal::red() } else if t > 70.0 { pal::amber() } else { pal::green() };
                for (icon, v, max, u, l, c) in [
                    ("🌡", temp, 100.0, "°C", "GPU", heat(temp)),
                    ("🔥", hot, 110.0, "°C", "hotspot", heat(hot - 10.0)),
                    ("⚡", g.tbp.unwrap_or(0.0) as f32, 190.0, "W", "placa", pal::bolt()),
                    ("⏱", g.clk.unwrap_or(0) as f32, 2900.0, "MHz", "clock", pal::cyan()),
                    ("🌀", g.fan.unwrap_or(0) as f32, 3300.0, "RPM", "fan", pal::violet()),
                ] {
                    if gauge(ui, icon, v, max, u, l, c).clicked() {
                        *sel = Some(Sel::Gpu);
                    }
                }
            });
            let temp: Vec<f32> = samples.iter().filter_map(|s| s.gpu.and_then(|g| g.temp)).map(|v| v as f32).collect();
            let hot: Vec<f32> = samples.iter().filter_map(|s| s.gpu.and_then(|g| g.hot)).map(|v| v as f32).collect();
            let tbp: Vec<f32> = samples.iter().filter_map(|s| s.gpu.and_then(|g| g.tbp)).map(|v| v as f32).collect();
            chart(ui, "🌡 / ⚡", &[(pal::green(), "GPU", temp), (pal::amber(), "hotspot", hot), (pal::bolt(), "W", tbp)], 90.0, "", 70.0);
            if let Some(s) = st {
                let vram = g.vram.unwrap_or(0) as f32;
                let frac = if s.vram_total_mb > 0 { vram / s.vram_total_mb as f32 } else { 0.0 };
                ui.add(egui::ProgressBar::new(frac).text(format!("VRAM {} / {} MB", g.vram.unwrap_or(0), s.vram_total_mb)).fill(pal::amd().gamma_multiply(0.6)));
            }
        });
        self.sel = sel;
    }

    fn card_timeline(&mut self, ui: &mut Ui) {
        let now = now_ms();
        let mut sel = self.sel;
        card_i(ui, "🕓  Linha do tempo", pal::red(), &mut sel, Topic::Stability, "Fontes: Event Log, PnP ao vivo, Overwatch", |ui, _sel| {
            ui.horizontal_wrapped(|ui| {
                for r in [Range::H24, Range::D7, Range::D30, Range::All] {
                    if chip(ui, self.range == r, r.label(), pal::cyan()) {
                        self.range = r;
                    }
                }
                ui.add_space(8.0);
                for (c, on) in self.cats.iter_mut() {
                    let name = match c {
                        Cat::Dock => "🔗 dock",
                        Cat::Energia => "⚡ energia",
                        Cat::Pcie => "⚠ PCIe/WHEA",
                        Cat::Tdr => "❄ TDR",
                        Cat::Gpu => "🎮 GPU",
                        Cat::UsbA => "🖴 USB-A",
                        Cat::Sistema => "🌙 sistema",
                        Cat::App => "ℹ painel",
                    };
                    if chip(ui, *on, name, cat_color(*c)) {
                        *on = !*on;
                    }
                }
                ui.add_space(8.0);
                if chip(ui, self.only_problems, "❗ só problemas", pal::red()) {
                    self.only_problems = !self.only_problems;
                }
            });
            ui.add_space(4.0);
            let enabled: Vec<Cat> = self.cats.iter().filter(|c| c.1).map(|c| c.0).collect();
            let range = self.range.ms();
            let w = ui.available_width();
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 46.0), Sense::hover());
            let p = ui.painter_at(rect);
            p.rect_filled(rect, CornerRadius::same(6), pal::well());
            let t0 = if self.range == Range::All { self.events.first().map(|e| e.ts).unwrap_or(now - 86_400_000) } else { now - range };
            let x_of = |ts: i64| rect.left() + 8.0 + (rect.width() - 16.0) * ((ts - t0) as f32 / (now - t0).max(1) as f32);
            for e in self.events.iter().filter(|e| e.ts >= t0 && enabled.contains(&e.kind.cat())) {
                let x = x_of(e.ts);
                let (y0, y1) = match e.kind.severity() {
                    2 => (rect.top() + 6.0, rect.bottom() - 14.0),
                    1 => (rect.top() + 12.0, rect.bottom() - 16.0),
                    _ => (rect.top() + 18.0, rect.bottom() - 18.0),
                };
                p.line_segment([pos2(x, y0), pos2(x, y1)], Stroke::new(if e.kind.severity() == 2 { 2.0 } else { 1.0 }, cat_color(e.kind.cat())));
            }
            p.text(rect.left_bottom() + vec2(8.0, -2.0), egui::Align2::LEFT_BOTTOM, fmt_local(t0, false), FontId::proportional(10.0), pal::faint());
            p.text(rect.right_bottom() + vec2(-8.0, -2.0), egui::Align2::RIGHT_BOTTOM, "agora", FontId::proportional(10.0), pal::faint());
            if let Some(pos) = resp.hover_pos() {
                let ts = t0 + (((pos.x - rect.left() - 8.0) / (rect.width() - 16.0)) as f64 * (now - t0) as f64) as i64;
                p.line_segment([pos2(pos.x, rect.top()), pos2(pos.x, rect.bottom())], Stroke::new(1.0, pal::dim().gamma_multiply(0.5)));
                p.text(pos2(pos.x + 4.0, rect.top() + 2.0), egui::Align2::LEFT_TOP, fmt_local(ts, false), FontId::proportional(10.0), pal::dim());
            }
            ui.add_space(6.0);

            let mut open = self.open_inc;
            let incs: Vec<&Incident> = self
                .incidents
                .iter()
                .rev()
                .filter(|i| i.start >= t0)
                .filter(|i| i.events.iter().any(|e| enabled.contains(&e.kind.cat())))
                .filter(|i| !self.only_problems || i.sev >= 1)
                .take(150)
                .collect();
            if incs.is_empty() {
                ui.label(egui::RichText::new("✔ nada neste período").color(pal::green()));
            }
            for inc in incs {
                let head = inc.events.iter().max_by_key(|e| (e.kind.severity(), e.kind.is_drop() as u8)).unwrap();
                let color = if inc.is_drop { pal::red() } else { sev_color(inc.sev) };
                let is_open = open == Some(inc.start);
                egui::Frame::new()
                    .fill(if is_open { pal::card2() } else { Color32::TRANSPARENT })
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(8, 3))
                    .show(ui, |ui| {
                        let r = ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(kind_icon(head.kind)).color(color).size(15.0));
                            ui.label(egui::RichText::new(fmt_local(inc.start, true)).monospace().color(pal::dim()).size(12.0));
                            ui.label(egui::RichText::new(head.kind.label()).color(color).strong());
                            // ícones dos outros eventos do incidente
                            let mut seen = vec![head.kind];
                            for e in &inc.events {
                                if !seen.contains(&e.kind) {
                                    seen.push(e.kind);
                                    ui.label(egui::RichText::new(kind_icon(e.kind)).color(cat_color(e.kind.cat())).size(13.0)).on_hover_text(e.kind.label());
                                }
                            }
                            if inc.is_drop && inc.events.iter().any(|e| e.kind.cat() == Cat::Dock) {
                                if inc.pd_lost {
                                    ui.label(egui::RichText::new("🔋").color(pal::amber()).size(14.0)).on_hover_text("PD caiu junto: o dock inteiro apagou");
                                } else {
                                    ui.label(egui::RichText::new("⛓").color(pal::violet()).size(14.0)).on_hover_text("Só o enlace caiu; o PD continuou");
                                }
                            }
                            if inc.events.len() > 1 {
                                ui.label(egui::RichText::new(format!("×{}", inc.events.len())).color(pal::faint()).size(11.0));
                            }
                        });
                        let click = ui.interact(r.response.rect, ui.id().with(("inc", inc.start)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                        if click.clicked() {
                            open = if is_open { None } else { Some(inc.start) };
                        }
                        if is_open {
                            for e in &inc.events {
                                ui.horizontal_wrapped(|ui| {
                                    ui.add_space(22.0);
                                    ui.label(egui::RichText::new(kind_icon(e.kind)).color(cat_color(e.kind.cat())));
                                    ui.label(egui::RichText::new(fmt_local(e.ts, true)).monospace().size(11.5).color(pal::faint()));
                                    ui.label(egui::RichText::new(&e.msg).size(12.0)).on_hover_text(format!("fonte: {}\n{}", e.src, e.dev.clone().unwrap_or_default()));
                                });
                                if let Some(s) = &e.snap {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.add_space(40.0);
                                        ui.label(egui::RichText::new(format!("📸 {s}")).size(11.5).color(pal::cyan()));
                                    });
                                }
                            }
                            if inc.is_drop {
                                let before = history::get().before(inc.start, 120_000);
                                ui.horizontal(|ui| {
                                    ui.add_space(22.0);
                                    ui.label(egui::RichText::new(format!("⏪ 2 min antes: {}", if before.is_empty() { "nada no Event Log (queda sem aviso)".to_string() } else { format!("{} evento(s)", before.len()) })).strong().color(pal::amber()).size(12.0));
                                });
                                for e in before.iter().rev().take(20) {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.add_space(40.0);
                                        ui.label(egui::RichText::new(format!("−{:.1} s", (inc.start - e.ts) as f64 / 1000.0)).monospace().size(11.5).color(pal::faint()));
                                        ui.label(egui::RichText::new(kind_icon(e.kind)).color(cat_color(e.kind.cat())));
                                        ui.label(egui::RichText::new(&e.msg).size(11.5).color(pal::dim()));
                                    });
                                }
                            }
                            ui.add_space(3.0);
                        }
                    });
            }
            self.open_inc = open;
            ui.label(egui::RichText::new(format!("{} eventos no histórico", self.events.len())).color(pal::faint()).size(11.0));
        });
        self.sel = sel;
        if let Some(f) = self.flash.clone() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&f).color(pal::cyan()).size(12.0));
                if ui.small_button("✕").clicked() {
                    self.flash = None;
                }
            });
        }
    }

    // ---------------- painel lateral

    fn details(&mut self, ui: &mut Ui, topo: &Topology, gst: Option<&crate::collect::GpuStatic>, samples: &[Sample], bench: Option<&BenchResult>, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            let title = match self.sel {
                Some(Sel::Info(t)) => t.title(),
                Some(Sel::Host) => "💻 Notebook",
                Some(Sel::Cable) | Some(Sel::Dock) => "🔗 Dock",
                Some(Sel::Port(_)) => "🔌 Porta do dock",
                Some(Sel::Gpu) => "🎮 GPU",
                None => "",
            };
            ui.label(egui::RichText::new(title).size(16.0).strong().color(pal::fg()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("✕").clicked() {
                    self.sel = None;
                }
            });
        });
        ui.separator();
        match self.sel {
            Some(Sel::Info(t)) => self.info(ui, t, topo, samples, bench, ctx),
            Some(Sel::Host) => {
                node_details(ui, "Controlador USB4 do note", topo.host_ctrl.as_ref());
                node_details(ui, "Roteador raiz USB4", topo.host_router.as_ref());
                usb4_table(ui, "Chaves USB4 do host", &topo.host_usb4);
                node_details(ui, "Porta raiz PCIe (túnel)", topo.root_port.as_ref());
            }
            Some(Sel::Cable) | Some(Sel::Dock) => {
                node_details(ui, "Roteador Thunderbolt do dock", topo.dock_router.as_ref());
                usb4_table(ui, "Chaves USB4 do dock {5DF7E321-…}", &topo.dock_usb4);
                node_details(ui, "Switch PCIe do dock (upstream)", topo.dock_switch.as_ref());
                for (i, p) in topo.ports.iter().enumerate() {
                    node_details(ui, &format!("Porta downstream {} — {:?}", i + 1, p.role), Some(&p.node));
                }
            }
            Some(Sel::Port(i)) => {
                if let Some(p) = topo.ports.get(i) {
                    node_details(ui, "Porta downstream do switch", Some(&p.node));
                    node_details(ui, "Dispositivo na porta", p.child.as_ref());
                    if p.role == PortRole::UsbA {
                        for d in &topo.usb_a {
                            node_details(ui, "Dispositivo USB", Some(d));
                        }
                    }
                }
            }
            Some(Sel::Gpu) => {
                if let Some(s) = gst {
                    kv(ui, "ADLX:", format!("{} · {} MB · {} x{}", s.name, s.vram_total_mb, s.bus, s.lanes));
                    kv(ui, "LUID (PDH):", s.luid.clone().unwrap_or_default());
                }
                node_details(ui, "GPU", topo.gpu.as_ref());
                for n in &topo.gpu_chain {
                    node_details(ui, "Switch interno da placa", Some(n));
                }
            }
            None => {}
        }
    }

    fn info(&mut self, ui: &mut Ui, t: Topic, topo: &Topology, samples: &[Sample], bench: Option<&BenchResult>, ctx: &egui::Context) {
        let para = |ui: &mut Ui, s: &str| {
            ui.label(egui::RichText::new(s).size(12.5).color(pal::fg()));
            ui.add_space(4.0);
        };
        match t {
            Topic::Link => {
                para(ui, "O Windows publica no roteador do dock propriedades USB4 sem documentação ({5DF7E321-…}). Comparando dock e host com os IDs de hardware, deu para decodificar várias delas (tabela completa ao clicar no dock).");
                para(ui, "A chave 20 vale 0x8. No spec USB4 (Lane Adapter CS1) esse campo vale 0x8 = Gen2 (10 Gb/s por faixa) e 0x4 = Gen3 (20 Gb/s). A chave 21 vale 2: duas faixas. Juntando, o enlace estaria a 20 Gb/s, metade dos 40 Gb/s do TB3.");
                match bench {
                    Some(b) => para(ui, &format!("O teste de banda mediu ↑ {:.2} GB/s e ↓ {:.2} GB/s. Em TB3 a 40 Gb/s o esperado é 2,5–2,8 GB/s; perto de 1,6–1,9 GB/s é o típico de um enlace de 20 Gb/s.", b.h2d_gbs, b.d2h_gbs)),
                    None => para(ui, "Rode o teste de banda: abaixo de ~2,2 GB/s confirma o enlace reduzido."),
                }
                ui.label(egui::RichText::new("▶ Como provar").strong().color(pal::bolt()));
                para(ui, "Troque por um cabo Thunderbolt 3/4 de 40 Gb/s certificado (passivo, até 0,8 m). Se a chave 20 virar 0x4 e a banda subir para ~2,5 GB/s, a decodificação fica provada e o problema era o cabo.");
                if ui.button("📏 Rodar teste de banda agora").clicked() {
                    self.start_bench(ctx);
                }
                usb4_table(ui, "Chaves USB4 do dock", &topo.dock_usb4);
            }
            Topic::Power => {
                para(ui, "Fonte: WMI root\\wmi BatteryStatus (PowerOnline, ChargeRate, DischargeRate, Voltage) — é o que o driver de bateria ACPI publica.");
                para(ui, "Com a bateria cheia, a taxa de carga é zero e o PD alimenta só o consumo do sistema. Nesse caso o Windows não informa quantos watts o dock entrega.");
                para(ui, "A tensão/corrente negociadas no contrato PD (por ex. 20 V × 4,25 A = 85 W) ficam no controlador PD do dock e não chegam ao Windows.");
                para(ui, "O sinal mais útil para diagnóstico é a queda do PowerOnline: se a energia externa some no mesmo segundo em que o dock cai, o dock inteiro apagou.");
                let series: Vec<f32> = samples.iter().filter_map(|s| s.power).map(|p| p.mv as f32 / 1000.0).collect();
                chart(ui, "tensão da bateria (V)", &[(pal::bolt(), "V", series)], 13.0, " V", 70.0);
            }
            Topic::Stability => {
                para(ui, "Uma queda é agrupada com tudo o que aconteceu até 20 s depois (switch PCIe, roteador TB, GPU, energia).");
                para(ui, "🔋 PD caiu junto: o note perdeu a energia externa no mesmo instante. Isso indica que o dock inteiro apagou ou resetou (fonte ATX, controlador do dock ou PD), não só o túnel.");
                para(ui, "⛓ Só o enlace: a energia continuou chegando e só o túnel/PCIe caiu. Isso aponta para cabo, conector ou o próprio controlador TB.");
                para(ui, "💥 \"Falha\" = Kernel-PnP 1011: o Windows relatou o dispositivo como falhando. ⏏ \"Removido\" = 1010: sumiu do barramento (desconexão física ou apagão).");
                para(ui, "Clique num incidente da linha do tempo para ver os eventos e o que veio nos 2 minutos anteriores.");
                if ui.button("📄 Exportar linha do tempo (CSV)").clicked() {
                    match crate::config::export_csv() {
                        Ok(p) => {
                            crate::config::open_folder(p.parent().unwrap_or(&p));
                            self.flash = Some(format!("Exportado: {}", p.display()));
                        }
                        Err(e) => self.flash = Some(format!("Falha: {e}")),
                    }
                }
            }
            Topic::Flow => {
                para(ui, "Nenhum contador do Windows mede bytes no túnel Thunderbolt. O motor Copy da GPU executa as cópias DMA entre a memória do note e a VRAM; quando ele está ocupado, o túnel está ocupado.");
                para(ui, "Os contadores vêm do PDH: GPU Engine (Utilization Percentage, filtrado pelo LUID da Radeon) e GPU Adapter Memory.");
                para(ui, "Os pulsos da topologia andam mais rápido e ficam mais densos conforme o uso do Copy.");
            }
            Topic::Gpu => {
                para(ui, "Dados do AMD ADLX (amdadlx64.dll), carregado só com a janela aberta e liberado ao fechar.");
            }
            Topic::Hidden => {
                for (icon, name, why) in HIDDEN {
                    ui.label(egui::RichText::new(format!("{icon} {name}")).strong());
                    para(ui, why);
                }
            }
        }
    }
}

fn self_open_timeline(sel: &mut Option<Sel>) {
    *sel = Some(Sel::Info(Topic::Stability));
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Topic {
    Link,
    Power,
    Stability,
    Flow,
    Gpu,
    Hidden,
}

impl Topic {
    fn title(self) -> &'static str {
        match self {
            Topic::Link => "⚡ Enlace Thunderbolt",
            Topic::Power => "🔌 Energia PD",
            Topic::Stability => "🛡 Quedas do dock",
            Topic::Flow => "📶 Fluxo de dados",
            Topic::Gpu => "🎮 GPU",
            Topic::Hidden => "🚫 Invisível ao Windows",
        }
    }
}

fn node_details(ui: &mut Ui, title: &str, n: Option<&Node>) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title).strong().color(pal::cyan()));
    let Some(n) = n else {
        ui.label(egui::RichText::new("ausente").color(pal::red()));
        return;
    };
    egui::Grid::new(("nd", title, &n.id)).num_columns(2).spacing(vec2(10.0, 2.0)).striped(true).max_col_width(ui.available_width() * 0.62).show(ui, |ui| {
        let mut row = |k: &str, v: String| {
            ui.label(egui::RichText::new(k).color(pal::dim()).size(11.5));
            ui.label(egui::RichText::new(v).size(11.5));
            ui.end_row();
        };
        row("nome", n.name.clone());
        row("instance id", n.id.clone());
        if let Some(f) = &n.firmware {
            row("firmware", f.clone());
        }
        row("estado", format!("{} · problema {}", dname(n), n.problem));
        if let Some(d) = &n.driver {
            row("driver", format!("{d} ({})", n.driver_provider.clone().unwrap_or_default()));
        }
        if let Some(l) = &n.location {
            row("local", l.clone());
        }
        if let Some((cs, cw, ms, mw)) = n.pcie {
            row("enlace PCIe", format!("{} x{cw} (máx {} x{mw})", pnp::pcie_speed_name(cs), pnp::pcie_speed_name(ms)));
        }
        if let Some(t) = n.last_arrival {
            row("última chegada", fmt_local(t, false));
        }
        if let Some(t) = n.last_removal {
            row("última remoção", fmt_local(t, false));
        }
        if let Some(r) = n.removal_policy {
            row("política de remoção", match r { 1 => "esperada (hot-plug)".into(), 2 => "remoção ordenada".into(), 3 => "remoção surpresa".into(), x => x.to_string() });
        }
        for (k, v) in &n.pci_err {
            let v = if *k == "Tipo de dispositivo PCIe" { v.split(' ').next().and_then(|x| x.parse().ok()).map(pnp::pci_type_name).unwrap_or("?").to_string() } else { v.clone() };
            row(k, v);
        }
    });
}

fn usb4_table(ui: &mut Ui, title: &str, props: &[crate::pnp::Usb4Prop]) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title).strong().color(pal::bolt()));
    if props.is_empty() {
        ui.label(egui::RichText::new("nenhuma").color(pal::dim()));
        return;
    }
    egui::Grid::new(("u4", title)).num_columns(4).spacing(vec2(8.0, 2.0)).striped(true).max_col_width(ui.available_width() * 0.42).show(ui, |ui| {
        for h in ["pid", "valor bruto", "significado", "confiança"] {
            ui.label(egui::RichText::new(h).color(pal::faint()).size(11.0));
        }
        ui.end_row();
        for p in props {
            ui.label(egui::RichText::new(p.pid.to_string()).monospace().size(11.5));
            ui.label(egui::RichText::new(format!("{} {}", p.val.type_name(), p.val.show())).monospace().size(11.0).color(pal::dim()));
            ui.label(egui::RichText::new(format!("{}\n{}", p.meaning, p.decoded)).size(11.5));
            let c = match p.confidence {
                "certo" => pal::green(),
                "forte" => pal::cyan(),
                "hipótese" => pal::amber(),
                _ => pal::faint(),
            };
            ui.label(egui::RichText::new(p.confidence).color(c).size(11.5));
            ui.end_row();
        }
    });
}

fn bars(ui: &mut Ui, r: &BenchResult) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 74.0), Sense::hover());
    let p = ui.painter_at(rect);
    let max = 3.2f32;
    let x_of = |v: f32| rect.left() + 110.0 + (rect.width() - 120.0) * (v / max).clamp(0.0, 1.0);
    // faixa esperada TB3 40G
    let band = Rect::from_x_y_ranges(x_of(2.5)..=x_of(2.8), rect.top()..=rect.bottom() - 14.0);
    p.rect_filled(band, CornerRadius::same(3), pal::green().gamma_multiply(0.12));
    p.text(pos2(band.center().x, rect.bottom()), egui::Align2::CENTER_BOTTOM, "esperado TB3 40 Gb/s", FontId::proportional(10.0), pal::green().gamma_multiply(0.8));
    for (i, (label, v, c)) in [("host → GPU", r.h2d_gbs, pal::cyan()), ("GPU → host", r.d2h_gbs, pal::violet())].iter().enumerate() {
        let y = rect.top() + 8.0 + i as f32 * 26.0;
        p.text(pos2(rect.left(), y + 8.0), egui::Align2::LEFT_CENTER, *label, FontId::proportional(12.0), pal::dim());
        let bar = Rect::from_min_max(pos2(x_of(0.0), y), pos2(x_of(*v as f32), y + 16.0));
        p.rect_filled(bar, CornerRadius::same(3), *c);
        p.text(pos2(bar.right() + 6.0, y + 8.0), egui::Align2::LEFT_CENTER, format!("{v:.2} GB/s"), FontId::proportional(12.5), pal::fg());
    }
}

fn fmt_dur(ms: i64) -> String {
    let s = ms / 1000;
    if s < 3600 {
        format!("{} min", s / 60)
    } else if s < 86400 {
        format!("{} h {} min", s / 3600, (s % 3600) / 60)
    } else {
        format!("{} d {} h", s / 86400, (s % 86400) / 3600)
    }
}
