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


/// Cinco paletas: claro e escuro (manuais) e AMD / NVIDIA / Intel (automáticas, pela placa
/// detectada no dock). `cyan()` é a cor de destaque do tema (dados, seleção, pulsos).
mod pal {
    use eframe::egui::{Color32, Shadow};
    use std::sync::atomic::{AtomicU8, Ordering};

    #[derive(Clone, Copy, PartialEq, Debug)]
    pub enum Th {
        Light = 0,
        Dark = 1,
        Amd = 2,
        Nvidia = 3,
        Intel = 4,
    }

    impl Th {
        pub fn is_light(self) -> bool {
            self == Th::Light
        }
        /// NVIDIA e Intel usam menu lateral; AMD (como o Adrenalin), claro e escuro usam abas no topo.
        pub fn nav_left(self) -> bool {
            matches!(self, Th::Nvidia | Th::Intel)
        }
        pub fn name(self) -> &'static str {
            match self {
                Th::Light => "Claro",
                Th::Dark => "Escuro",
                Th::Amd => "AMD",
                Th::Nvidia => "NVIDIA",
                Th::Intel => "Intel",
            }
        }
    }

    static CUR: AtomicU8 = AtomicU8::new(1);

    pub fn current() -> Th {
        match CUR.load(Ordering::Relaxed) {
            0 => Th::Light,
            2 => Th::Amd,
            3 => Th::Nvidia,
            4 => Th::Intel,
            _ => Th::Dark,
        }
    }
    pub fn set(t: Th) {
        CUR.store(t as u8, Ordering::Relaxed);
    }

    const fn c(r: u8, g: u8, b: u8) -> Color32 {
        Color32::from_rgb(r, g, b)
    }

    // ordem: bg card card2 card3 well grid border fg dim faint accent green amber red violet amd intel bolt pcie data
    const P: [[Color32; 20]; 5] = [
        // claro
        [c(242, 244, 247), c(255, 255, 255), c(244, 246, 249), c(235, 242, 252), c(246, 248, 251), c(226, 230, 236), c(214, 220, 227), c(28, 33, 40), c(90, 99, 110), c(150, 158, 168), c(2, 132, 199), c(26, 127, 55), c(176, 112, 0), c(207, 34, 46), c(130, 80, 223), c(200, 30, 40), c(0, 104, 181), c(181, 136, 0), c(100, 125, 160), c(2, 120, 190)],
        // escuro
        [c(13, 17, 23), c(22, 27, 34), c(30, 36, 45), c(26, 33, 44), c(16, 20, 26), c(33, 39, 47), c(48, 54, 61), c(230, 237, 243), c(150, 160, 170), c(118, 126, 136), c(56, 189, 248), c(63, 185, 80), c(210, 153, 34), c(248, 81, 73), c(163, 113, 247), c(237, 70, 70), c(64, 140, 230), c(245, 196, 0), c(120, 150, 190), c(56, 189, 248)],
        // AMD (Adrenalin): fundo #111, cartões #262626 com faixa de título preta, vermelho #E4002B
        [c(17, 17, 18), c(38, 38, 40), c(48, 48, 50), c(54, 54, 58), c(12, 12, 13), c(52, 52, 55), c(38, 38, 40), c(242, 242, 244), c(190, 190, 196), c(140, 140, 148), c(228, 0, 43), c(46, 204, 113), c(255, 170, 30), c(255, 99, 120), c(175, 125, 255), c(237, 28, 36), c(80, 150, 230), c(255, 196, 0), c(150, 150, 162), c(110, 185, 235)],
        // NVIDIA: grafite + verde
        [c(24, 24, 24), c(34, 34, 34), c(44, 44, 44), c(36, 48, 18), c(28, 28, 28), c(52, 52, 52), c(58, 58, 58), c(238, 238, 238), c(180, 180, 180), c(132, 132, 132), c(118, 185, 0), c(118, 185, 0), c(240, 170, 0), c(255, 88, 88), c(160, 125, 240), c(237, 70, 70), c(80, 150, 230), c(240, 200, 0), c(135, 155, 120), c(140, 200, 60)],
        // Intel: azul-marinho + ciano
        [c(9, 17, 31), c(16, 27, 45), c(23, 37, 59), c(17, 43, 69), c(11, 21, 37), c(31, 47, 71), c(35, 53, 79), c(232, 240, 250), c(156, 176, 200), c(110, 130, 158), c(0, 199, 253), c(70, 200, 120), c(255, 180, 0), c(255, 92, 92), c(150, 130, 255), c(237, 70, 70), c(0, 104, 181), c(250, 200, 0), c(110, 150, 200), c(0, 199, 253)],
    ];

    /// Raio dos cantos: o Adrenalin é todo reto; NVIDIA quase reto.
    pub fn radius() -> u8 {
        match current() {
            Th::Amd => 0,
            Th::Nvidia => 2,
            Th::Intel => 6,
            _ => 10,
        }
    }
    /// Espaço entre cartões.
    pub fn gap() -> f32 {
        if current() == Th::Amd { 20.0 } else { 12.0 }
    }

    fn at(i: usize) -> Color32 {
        P[current() as usize][i]
    }
    pub fn bg() -> Color32 { at(0) }
    pub fn card() -> Color32 { at(1) }
    pub fn card2() -> Color32 { at(2) }
    pub fn card3() -> Color32 { at(3) }
    pub fn well() -> Color32 { at(4) }
    pub fn grid() -> Color32 { at(5) }
    pub fn border() -> Color32 { at(6) }
    pub fn fg() -> Color32 { at(7) }
    pub fn dim() -> Color32 { at(8) }
    pub fn faint() -> Color32 { at(9) }
    pub fn cyan() -> Color32 { at(10) }
    pub fn green() -> Color32 { at(11) }
    pub fn amber() -> Color32 { at(12) }
    pub fn red() -> Color32 { at(13) }
    pub fn violet() -> Color32 { at(14) }
    pub fn amd() -> Color32 { at(15) }
    pub fn intel() -> Color32 { at(16) }
    pub fn bolt() -> Color32 { at(17) }
    pub fn pcie() -> Color32 { at(18) }
    /// Cor de dados (tráfego, gráficos). No tema AMD não é o vermelho da marca,
    /// para vermelho continuar significando só problema.
    pub fn data() -> Color32 { at(19) }

    // --- elevação: a luz vem de cima; fundo < cartão < nó/bloco < selecionado
    /// Cor-base das sombras (no claro, um azul-ardósia; no Intel, um marinho).
    pub fn shade_base() -> (u8, u8, u8) {
        match current() {
            Th::Light => (30, 45, 70),
            Th::Intel => (0, 8, 20),
            _ => (0, 0, 0),
        }
    }
    /// Realce de 1 px na borda de cima dos cartões.
    pub fn hi() -> Color32 {
        match current() {
            Th::Light => Color32::TRANSPARENT,
            Th::Amd => Color32::from_white_alpha(10),
            Th::Intel => Color32::from_rgba_unmultiplied(150, 200, 255, 14),
            _ => Color32::from_white_alpha(12),
        }
    }
    /// Cor da seleção. No AMD brilha em neutro: vermelho fica só para problema.
    pub fn sel() -> Color32 {
        if current() == Th::Amd { c(235, 235, 240) } else { cyan() }
    }
    /// Sombra de elevação: nível 1 = cartão, 2 = nó/bloco. `lift` (0..1) = hover.
    pub fn elev(level: u8, lift: f32) -> Shadow {
        let th = current();
        let (b, dy, a): (f32, f32, f32) = match (th, level >= 2) {
            (Th::Light, false) => (14.0, 3.0, 22.0),
            (Th::Light, true) => (8.0, 2.0, 18.0),
            (Th::Dark, false) => (16.0, 4.0, 90.0),
            (Th::Dark, true) => (10.0, 3.0, 80.0),
            (Th::Amd, false) => (20.0, 6.0, 120.0),
            (Th::Amd, true) => (12.0, 4.0, 110.0),
            (Th::Nvidia, false) => (14.0, 4.0, 100.0),
            (Th::Nvidia, true) => (9.0, 3.0, 90.0),
            (Th::Intel, false) => (18.0, 5.0, 130.0),
            (Th::Intel, true) => (11.0, 3.0, 110.0),
        };
        let l = lift.clamp(0.0, 1.0);
        let da = if th.is_light() { 16.0 } else { 40.0 };
        let (r, g, bb) = shade_base();
        Shadow {
            offset: [0, (dy + 4.0 * l).round() as i8],
            blur: (b + 10.0 * l).round() as u8,
            spread: 0,
            color: Color32::from_rgba_unmultiplied(r, g, bb, (a + da * l).round().min(255.0) as u8),
        }
    }
    /// Topo do gradiente do corpo dos cartões (a base é `card()`).
    pub fn card_top() -> Color32 {
        match current() {
            Th::Light => Color32::WHITE,
            Th::Dark => c(28, 33, 41),
            Th::Nvidia => c(40, 40, 40),
            Th::Intel => c(20, 33, 54),
            Th::Amd => c(42, 42, 44),
        }
    }
    /// Base do gradiente do corpo dos cartões.
    pub fn card_bot() -> Color32 {
        if current().is_light() { c(249, 250, 252) } else { card() }
    }
}

use pal::Th;

/// Cor de brilho: no escuro sai aditiva (alfa 0 soma luz); no claro, só translúcida.
fn glow(c: Color32, k: f32) -> Color32 {
    if pal::current().is_light() {
        c.gamma_multiply(k)
    } else {
        let [r, g, b, _] = c.to_array();
        Color32::from_rgba_premultiplied((r as f32 * k) as u8, (g as f32 * k) as u8, (b as f32 * k) as u8, 0)
    }
}

/// Retângulo arredondado com gradiente vertical (topo → base), com bordas suavizadas.
fn vgrad(p: &egui::Painter, r: Rect, cr: u8, top: Color32, bot: Color32) -> egui::Shape {
    let mut m = egui::Mesh::default();
    let mut t = egui::epaint::Tessellator::new(p.ctx().pixels_per_point(), Default::default(), [1, 1], vec![]);
    t.tessellate_rect(&egui::epaint::RectShape::filled(r, CornerRadius::same(cr), Color32::WHITE), &mut m);
    let h = r.height().max(1.0);
    for v in &mut m.vertices {
        let k = ((v.pos.y - r.top()) / h).clamp(0.0, 1.0);
        v.color = top.lerp_to_gamma(bot, k).gamma_multiply(v.color.a() as f32 / 255.0);
    }
    egui::Shape::mesh(m)
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
    let icon = egui::IconData { rgba: crate::tray::icon_rgba(256, crate::tray::DOCK_ON.load(std::sync::atomic::Ordering::SeqCst)), width: 256, height: 256 };
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
    Psu,
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
    /// seleção do quadro anterior e instante da última troca (animação de entrada do painel lateral)
    sel_prev: Option<Sel>,
    sel_at: f64,
    range: Range,
    cats: Vec<(Cat, bool)>,
    hist_len: usize,
    events: Vec<Ev>,
    incidents: Vec<Incident>,
    open_inc: Option<i64>,
    only_problems: bool,
    flash: Option<String>,
    tab: Tab,
    vendor: Option<Th>,
    hist_limit: usize,
    logo_tex: Option<(Th, bool, u32, egui::TextureHandle)>,
    psu: crate::psu::Psu,
    psu_job: Option<Arc<Mutex<Option<Result<crate::psu::Found, String>>>>>,
    psu_msg: Option<(bool, String)>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        *UI_CTX.lock().unwrap() = Some(ctx.clone());
        fonts(&ctx);
        apply_theme(&ctx, resolve_theme(crate::config::get().theme, vendor_of(&pnp::read_topology())));
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
            // EGPU_SEL=gpu|dock|host|cable|psu|info|port0.. abre com o painel lateral de um nó (capturas e testes)
            sel: match std::env::var("EGPU_SEL").ok().as_deref() {
                Some("gpu") => Some(Sel::Gpu),
                Some("dock") => Some(Sel::Dock),
                Some("host") => Some(Sel::Host),
                Some("cable") => Some(Sel::Cable),
                Some("info") => Some(Sel::Info(Topic::Link)),
                Some("psu") if crate::psu::load().watts > 0 => Some(Sel::Psu),
                Some(v) if v.starts_with("port") => v[4..].parse().ok().map(Sel::Port),
                _ => None,
            },
            sel_prev: None,
            sel_at: 0.0,
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
            // EGPU_ABA=1..4 abre direto numa aba (usado para capturas e testes)
            tab: std::env::var("EGPU_ABA").ok().and_then(|v| v.parse::<usize>().ok()).and_then(|i| Tab::ALL.get(i).copied()).unwrap_or(Tab::Geral),
            vendor: vendor_of(&pnp::read_topology()),
            hist_limit: 30,
            logo_tex: None,
            psu: crate::psu::load(),
            psu_job: None,
            psu_msg: None,
        }
    }

    fn refresh_history(&mut self) {
        if crate::history::is_panel() {
            crate::history::get().tail();
        }
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
    // Segoe UI Variable (Windows 11) é mais macia; no Windows 10 cai na Segoe UI clássica
    let main = std::fs::read(dir.join("SegUIVar.ttf")).or_else(|_| std::fs::read(dir.join("segoeui.ttf")));
    if let Ok(bytes) = main {
        f.font_data.insert("segoe".into(), Arc::new(egui::FontData::from_owned(bytes)));
        front.push("segoe".to_string());
    }
    if let Ok(bytes) = std::fs::read(dir.join("seguisym.ttf")) {
        // símbolos um pouco menores e alinhados ao centro do texto
        let tweak = egui::FontTweak { scale: 0.92, y_offset_factor: 0.04, ..Default::default() };
        f.font_data.insert("segoe_sym".into(), Arc::new(egui::FontData::from_owned(bytes).tweak(tweak)));
        front.push("segoe_sym".to_string());
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
    apply_theme(ctx, pal::current());
}

fn apply_theme(ctx: &egui::Context, th: Th) {
    pal::set(th);
    let mut v = if th.is_light() { egui::Visuals::light() } else { egui::Visuals::dark() };
    v.panel_fill = pal::bg();
    v.window_fill = pal::card();
    v.extreme_bg_color = pal::well();
    v.faint_bg_color = pal::card2();
    v.override_text_color = Some(pal::fg());
    v.widgets.inactive.fg_stroke.color = pal::fg();
    v.widgets.noninteractive.fg_stroke.color = pal::fg();
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, pal::border());
    v.widgets.inactive.bg_fill = pal::card2();
    v.widgets.inactive.weak_bg_fill = pal::card2();
    v.widgets.hovered.weak_bg_fill = pal::card3();
    v.selection.bg_fill = pal::cyan().gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, pal::cyan());
    v.hyperlink_color = pal::cyan();
    v.window_corner_radius = CornerRadius::same(pal::radius());
    // o egui guarda estilos separados para claro/escuro e segue o tema do Windows; fixa o nosso
    ctx.set_theme(if th.is_light() { egui::Theme::Light } else { egui::Theme::Dark });
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        use egui::{FontFamily as F, TextStyle::*};
        // nada é cortado: textos, botões e painéis quebram a linha
        s.wrap_mode = Some(egui::TextWrapMode::Wrap);
        s.text_styles = [
            (Heading, FontId::new(21.0, F::Proportional)),
            (Body, FontId::new(15.0, F::Proportional)),
            (Button, FontId::new(15.0, F::Proportional)),
            (Small, FontId::new(12.5, F::Proportional)),
            (Monospace, FontId::new(13.5, F::Monospace)),
        ]
        .into();
        s.spacing.item_spacing = vec2(10.0, 8.0);
        s.spacing.button_padding = vec2(12.0, 6.0);
        s.spacing.interact_size.y = 26.0;
    });
}

/// Fabricante da GPU no dock, pelo Vendor ID do PCI.
fn vendor_of(t: &Topology) -> Option<Th> {
    match std::env::var("EGPU_TEMA").ok().as_deref() {
        Some("amd") => return Some(Th::Amd),
        Some("nvidia") => return Some(Th::Nvidia),
        Some("intel") => return Some(Th::Intel),
        _ => {}
    }
    let id = t.gpu.as_ref()?.id.to_uppercase();
    if id.contains("VEN_1002") {
        Some(Th::Amd)
    } else if id.contains("VEN_10DE") {
        Some(Th::Nvidia)
    } else if id.contains("VEN_8086") {
        Some(Th::Intel)
    } else {
        None
    }
}

/// config.theme: 0 = automático pela placa, 1 = claro, 2 = escuro.
fn resolve_theme(cfg: u8, vendor: Option<Th>) -> Th {
    // EGPU_TEMA=amd|nvidia|intel simula outra placa (para testar os temas automáticos)
    let vendor = match std::env::var("EGPU_TEMA").ok().as_deref() {
        Some("amd") => Some(Th::Amd),
        Some("nvidia") => Some(Th::Nvidia),
        Some("intel") => Some(Th::Intel),
        Some("claro") => return Th::Light,
        _ => vendor,
    };
    match cfg {
        1 => Th::Light,
        2 => Th::Dark,
        _ => vendor.unwrap_or(Th::Dark),
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tab {
    Geral,
    Enlace,
    Fluxo,
    Historico,
    Opcoes,
}

impl Tab {
    const ALL: [Tab; 5] = [Tab::Geral, Tab::Enlace, Tab::Fluxo, Tab::Historico, Tab::Opcoes];
    fn icon(self) -> &'static str {
        match self {
            Tab::Geral => "🏠",
            Tab::Enlace => "⚡",
            Tab::Fluxo => "📶",
            Tab::Historico => "🕓",
            Tab::Opcoes => "⚙",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Tab::Geral => "Visão geral",
            Tab::Enlace => "Cabo e energia",
            Tab::Fluxo => "Desempenho",
            Tab::Historico => "Histórico",
            Tab::Opcoes => "Opções",
        }
    }
}

// ---------------------------------------------------------------------------
// Peças visuais

fn card<R>(ui: &mut Ui, title: &str, accent: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    card_shell(ui, title, accent, None, |ui, _| add(ui))
}

/// Título sem o emoji inicial (o Adrenalin usa só texto em caixa alta).
fn bare(title: &str) -> &str {
    title.trim_start_matches(|c: char| !c.is_alphanumeric()).trim()
}

/// Moldura comum dos cartões. No tema AMD: cantos retos, faixa de título preta em caixa alta,
/// corpo com respiro de 18 px (como os blocos "EXPERIÊNCIA GLOBAL" / "DRIVER E SOFTWARE").
fn card_shell<R>(ui: &mut Ui, title: &str, accent: Color32, mut info: Option<(&mut Option<Sel>, Topic, &str)>, add: impl FnOnce(&mut Ui, Option<&mut Option<Sel>>) -> R) -> R {
    // sombra + gradiente do corpo entram num marcador pintado ANTES do conteúdo (ficam embaixo dele)
    let idx = ui.painter().add(egui::Shape::Noop);
    let cr = pal::radius();
    let out = if pal::current() == Th::Amd {
        egui::Frame::new()
            .fill(Color32::TRANSPARENT)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;
                let sidx = ui.painter().add(egui::Shape::Noop);
                let strip = egui::Frame::new().fill(Color32::TRANSPARENT).inner_margin(egui::Margin::symmetric(18, 11)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(bare(title).to_uppercase()).size(13.0).strong().color(pal::fg()));
                        if let Some((sel, topic, tip)) = info.as_mut() {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| info_btn(ui, sel, *topic, tip));
                        }
                    });
                });
                // faixa de título com gradiente e um chanfro claro embaixo dela
                let sr = strip.response.rect;
                let g = vgrad(ui.painter(), sr, 0, Color32::from_rgb(26, 26, 28), Color32::from_rgb(12, 12, 13));
                ui.painter().set(sidx, g);
                ui.painter().hline(sr.x_range(), sr.bottom() + 0.5, Stroke::new(1.0, Color32::from_white_alpha(10)));
                egui::Frame::new()
                    .inner_margin(egui::Margin::same(18))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 6.0;
                        add(ui, info.map(|i| i.0))
                    })
                    .inner
            })
    } else {
        egui::Frame::new()
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, pal::border()))
            .corner_radius(CornerRadius::same(pal::radius()))
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(4.0, 16.0), Sense::hover());
                    ui.painter().rect_filled(r, CornerRadius::same(2), accent);
                    ui.label(egui::RichText::new(title).size(15.0).strong().color(pal::fg()));
                    if let Some((sel, topic, tip)) = info.as_mut() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| info_btn(ui, sel, *topic, tip));
                    }
                });
                ui.add_space(6.0);
                add(ui, info.map(|i| i.0))
            })
    };
    // cartões não sobem no hover: não são clicáveis
    let r = out.response.rect;
    let body = vgrad(ui.painter(), r, cr, pal::card_top(), pal::card_bot());
    ui.painter().set(idx, egui::Shape::Vec(vec![pal::elev(1, 0.0).as_shape(r, CornerRadius::same(cr)).into(), body]));
    ui.painter().hline(egui::Rangef::new(r.left() + cr as f32, r.right() - cr as f32), r.top() + 0.5, Stroke::new(1.0, pal::hi()));
    out.inner
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
        .corner_radius(CornerRadius::same(pal::radius()))
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(text).color(color).size(13.5)).extend());
        });
}

fn big(ui: &mut Ui, value: String, unit: &str, label: &str, color: Color32) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label).color(pal::dim()).size(13.0));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.label(egui::RichText::new(value).size(22.0).strong().color(color));
            ui.label(egui::RichText::new(unit).size(13.5).color(pal::dim()));
        });
    });
}

/// Gráfico de linha simples (sem dependência extra).
fn chart(ui: &mut Ui, label: &str, series: &[(Color32, &str, Vec<f32>)], max: f32, unit: &str, h: f32) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(6), pal::well());
    let plot = Rect::from_min_max(pos2(rect.left() + 8.0, rect.top() + 30.0), pos2(rect.right() - 8.0, rect.bottom() - 8.0));
    for i in 0..=3 {
        let y = plot.bottom() - plot.height() * i as f32 / 3.0;
        p.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], Stroke::new(1.0, pal::grid()));
    }
    let maxv = series
        .iter()
        .flat_map(|s| s.2.iter().cloned())
        .fold(max, f32::max)
        .max(1e-3);
    let lg = p.layout_no_wrap(label.to_string(), FontId::proportional(13.5), pal::dim());
    let lw0 = lg.size().x;
    p.galley(rect.left_top() + vec2(10.0, 6.0), lg, pal::dim());
    p.text(rect.right_top() + vec2(-8.0, 4.0), egui::Align2::RIGHT_TOP, format!("máx {maxv:.0} {unit}"), FontId::proportional(12.0), pal::faint());
    let n = collect::HISTORY as f32;
    let mut legend_x = rect.left() + 10.0 + lw0 + 18.0;
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
            let g = p.layout_no_wrap(txt, FontId::proportional(12.5), *color);
            let lw = g.size().x;
            p.galley(pos2(legend_x, rect.top() + 6.0), g, *color);
            legend_x += lw + 14.0;
        }
    }
    if data_empty(series) {
        p.text(rect.center(), egui::Align2::CENTER_CENTER, "aguardando amostras…", FontId::proportional(13.5), pal::faint());
    }
    let _ = resp;
}

/// Barra de ocupação com o texto por cima, nas cores do tema.
fn meter_bar(ui: &mut Ui, frac: f32, text: &str, color: Color32) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let rad = CornerRadius::same(pal::radius().min(6));
    let p = ui.painter();
    p.rect_filled(r, rad, pal::well());
    let f = ui.ctx().animate_value_with_time(resp.id, frac.clamp(0.0, 1.0), 0.4);
    if f > 0.0 {
        p.rect_filled(Rect::from_min_size(r.min, vec2((r.width() * f).max(4.0), r.height())), rad, color.gamma_multiply(0.75));
    }
    p.text(r.left_center() + vec2(10.0, 0.0), egui::Align2::LEFT_CENTER, text, FontId::proportional(13.5), pal::fg());
    resp
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

#[allow(clippy::too_many_arguments)]
fn node_box(p: &egui::Painter, rect: Rect, title: &str, lines: &[String], accent: Color32, state: Color32, selected: bool, focus: bool, lift: f32, t: f64, art: Option<&dyn Fn(&egui::Painter, Rect)>) {
    node_box_r(p, rect, title, lines, accent, state, selected, focus, lift, t, art, 0.0);
}

/// Igual a node_box, deixando `reserve_bottom` px livres embaixo do texto (para um medidor por cima).
#[allow(clippy::too_many_arguments)]
fn node_box_r(p: &egui::Painter, rect: Rect, title: &str, lines: &[String], accent: Color32, state: Color32, selected: bool, focus: bool, lift: f32, t: f64, art: Option<&dyn Fn(&egui::Painter, Rect)>, reserve_bottom: f32) {
    // sombra (cresce no hover) e brilho respirando na seleção, antes do recorte do nó
    let cr = CornerRadius::same(pal::radius());
    p.add(pal::elev(2, lift).as_shape(rect, cr));
    if selected {
        let b = 0.85 + 0.15 * ((t * 2.1).sin() as f32);
        p.add(egui::Shadow { offset: [0, 0], blur: 18, spread: 2, color: pal::sel().gamma_multiply(0.30 * b) }.as_shape(rect, cr));
    }
    let p = &p.with_clip_rect(rect.intersect(p.clip_rect()));
    let fill = if focus { pal::card3() } else { pal::card2() };
    p.add(vgrad(p, rect, pal::radius(), fill, fill.lerp_to_gamma(Color32::BLACK, 0.06)));
    let stroke = if selected { Stroke::new(2.0, pal::sel()) } else { Stroke::new(1.0, pal::border().lerp_to_gamma(pal::fg(), 0.25 * lift)) };
    p.rect_stroke(rect, cr, stroke, StrokeKind::Inside);
    let bar = Rect::from_min_size(rect.min + vec2(1.0, 8.0), vec2(3.0, rect.height() - 16.0));
    p.rect_filled(bar, CornerRadius::same(2), accent);
    p.circle_filled(rect.right_top() + vec2(-12.0, 12.0), 4.5, state);
    // ilustração à esquerda; o texto começa depois dela
    let tx = match art {
        Some(f) => {
            let aw: f32 = if focus { 96.0 } else { (rect.width() * 0.3).clamp(44.0, 74.0) };
            let ar = Rect::from_min_size(rect.min + vec2(12.0, 8.0), vec2(aw, rect.height() - 16.0));
            f(p, ar);
            aw + 22.0
        }
        None => 12.0,
    };
    let wrap = (rect.right() - 26.0 - (rect.left() + tx)).max(40.0);
    let tg = p.layout(title.to_string(), FontId::proportional(if focus { 16.5 } else { 15.0 }), pal::fg(), wrap);
    let reserve = if focus { 24.0 } else { reserve_bottom }; // no dock, a faixa das portas fica embaixo
    let room = rect.height() - 14.0 - reserve;
    let mut gs = vec![];
    let mut block = tg.size().y;
    for l in lines {
        let g = p.layout(l.clone(), FontId::proportional(12.5), pal::dim(), wrap);
        if block + 2.0 + g.size().y > room {
            break;
        }
        block += 2.0 + g.size().y;
        gs.push(g);
    }
    let mut y = rect.top() + ((rect.height() - reserve - block) / 2.0).max(7.0);
    let x = rect.left() + tx;
    let th = tg.size().y;
    p.galley(pos2(x, y), tg, pal::fg());
    y += th + 2.0;
    for g in gs {
        let h = g.size().y;
        p.galley(pos2(x, y), g, pal::dim());
        y += h + 2.0;
    }
}

/// Ilustrações vetoriais desenhadas no pintor do egui: nítidas em qualquer DPI, sem bitmap,
/// sem logotipo de terceiros e animadas (ventoinhas giram com a GPU, LED do dock pulsa).
mod art {
    use super::pal;
    use eframe::egui::{pos2, vec2, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind};
    use std::f32::consts::TAU;

    fn fit(r: Rect, aspect: f32) -> Rect {
        // maior retângulo com a proporção pedida, centrado em r
        let (w, h) = if r.width() / r.height() > aspect { (r.height() * aspect, r.height()) } else { (r.width(), r.width() / aspect) };
        Rect::from_center_size(r.center(), vec2(w, h))
    }

    fn metal() -> Color32 {
        if pal::current().is_light() { Color32::from_rgb(200, 204, 210) } else { Color32::from_rgb(70, 72, 78) }
    }
    fn body() -> Color32 {
        if pal::current().is_light() { Color32::from_rgb(232, 235, 240) } else { Color32::from_rgb(28, 29, 32) }
    }

    pub fn laptop(p: &Painter, r: Rect, on: bool, t: f64) {
        let r = fit(r, 1.45);
        let k = r.width();
        let scr = Rect::from_min_max(pos2(r.left() + k * 0.12, r.top()), pos2(r.right() - k * 0.12, r.top() + r.height() * 0.74));
        p.rect_filled(scr, CornerRadius::same(3), metal());
        let inner = scr.shrink(k * 0.035);
        p.rect_filled(inner, CornerRadius::same(1), if on { Color32::from_rgb(12, 22, 40) } else { Color32::from_rgb(20, 20, 22) });
        if on {
            // "gráfico" na tela, com brilho que respira
            let a = 0.55 + 0.25 * ((t * 1.3).sin() as f32);
            let pts: Vec<Pos2> = (0..8)
                .map(|i| {
                    let x = inner.left() + inner.width() * (i as f32 / 7.0);
                    let y = inner.center().y + inner.height() * 0.28 * (((i as f32) * 1.7 + t as f32 * 0.8).sin());
                    pos2(x, y)
                })
                .collect();
            p.add(Shape::line(pts, Stroke::new(1.4, pal::cyan().gamma_multiply(a))));
        }
        let base = vec![
            pos2(r.left() + k * 0.06, scr.bottom() + 1.0),
            pos2(r.right() - k * 0.06, scr.bottom() + 1.0),
            pos2(r.right(), r.bottom()),
            pos2(r.left(), r.bottom()),
        ];
        p.add(Shape::convex_polygon(base, metal(), Stroke::NONE));
        p.line_segment([pos2(r.center().x - k * 0.1, r.bottom() - 1.5), pos2(r.center().x + k * 0.1, r.bottom() - 1.5)], Stroke::new(1.5, body()));
    }

    pub fn dock(p: &Painter, r: Rect, on: bool, t: f64) {
        let r = fit(r, 1.25);
        let k = r.width();
        let case = r.shrink2(vec2(0.0, k * 0.04));
        p.rect_filled(case, CornerRadius::same(5), metal());
        p.rect_filled(case.shrink(2.0), CornerRadius::same(4), body());
        // grade de ventilação
        for i in 0..6 {
            let y = case.top() + k * 0.12 + i as f32 * k * 0.07;
            p.line_segment([pos2(case.left() + k * 0.14, y), pos2(case.right() - k * 0.14, y)], Stroke::new(k * 0.03, metal()));
        }
        // ventoinha da fonte (gira devagar quando ligado)
        let fc = pos2(case.center().x, case.top() + k * 0.3);
        let _ = fc;
        // frente: porta TB + LED
        let fy = case.bottom() - k * 0.16;
        let port = Rect::from_center_size(pos2(case.left() + k * 0.3, fy), vec2(k * 0.2, k * 0.08));
        p.rect_filled(port, CornerRadius::same(4), Color32::from_rgb(10, 10, 12));
        p.text(port.center(), eframe::egui::Align2::CENTER_CENTER, "⚡", eframe::egui::FontId::proportional(k * 0.07), pal::bolt());
        let usb = Rect::from_center_size(pos2(case.center().x + k * 0.08, fy), vec2(k * 0.16, k * 0.07));
        p.rect_filled(usb, CornerRadius::same(1), Color32::from_rgb(10, 10, 12));
        p.rect_filled(usb.shrink2(vec2(k * 0.02, k * 0.022)), CornerRadius::ZERO, Color32::from_rgb(40, 110, 230));
        let led = pos2(case.right() - k * 0.15, fy);
        let col = if on { pal::green() } else { pal::red() };
        let pulse = 0.5 + 0.5 * ((t * 2.2).sin() as f32);
        p.circle_filled(led, k * 0.05 + k * 0.03 * pulse, col.gamma_multiply(0.2));
        p.circle_filled(led, k * 0.032, col);
    }

    /// Placa de vídeo de dois fans: as pás giram proporcionalmente à rotação/uso.
    pub fn gpu(p: &Painter, r: Rect, spin: f32, on: bool, t: f64) {
        let r = fit(r, 1.75);
        let k = r.width();
        // suporte (bracket) à esquerda
        let br = Rect::from_min_max(r.left_top(), pos2(r.left() + k * 0.05, r.bottom() - k * 0.04));
        p.rect_filled(br, CornerRadius::same(1), metal());
        // carenagem
        let shroud = Rect::from_min_max(pos2(r.left() + k * 0.06, r.top()), pos2(r.right(), r.bottom() - k * 0.08));
        p.rect_filled(shroud, CornerRadius::same(4), body());
        p.rect_stroke(shroud, CornerRadius::same(4), Stroke::new(1.0, metal()), StrokeKind::Inside);
        // filete de destaque (vermelho no tema AMD, verde no NVIDIA...)
        p.line_segment([pos2(shroud.left() + 4.0, shroud.top() + 2.5), pos2(shroud.right() - 4.0, shroud.top() + 2.5)], Stroke::new(1.6, pal::cyan()));
        // conector PCIe dourado
        let fy = shroud.bottom();
        let fx0 = shroud.left() + k * 0.22;
        let fx1 = shroud.right() - k * 0.2;
        p.rect_filled(Rect::from_min_max(pos2(fx0, fy), pos2(fx1, fy + k * 0.06)), CornerRadius::ZERO, Color32::from_rgb(196, 160, 60));
        for i in 0..14 {
            let x = fx0 + (fx1 - fx0) * (i as f32 + 0.5) / 14.0;
            p.line_segment([pos2(x, fy + 1.0), pos2(x, fy + k * 0.06)], Stroke::new(0.8, Color32::from_rgb(140, 110, 30)));
        }
        // dois fans
        let rad = shroud.height() * 0.4;
        let ang = (t as f32) * (0.6 + 9.0 * spin.clamp(0.0, 1.0)) * if on { 1.0 } else { 0.0 };
        for (i, cx) in [shroud.left() + shroud.width() * 0.29, shroud.left() + shroud.width() * 0.71].into_iter().enumerate() {
            let c = pos2(cx, shroud.center().y + 1.0);
            p.circle_filled(c, rad, Color32::from_rgb(14, 14, 16));
            p.circle_stroke(c, rad, Stroke::new(1.0, metal()));
            let a0 = ang + i as f32 * 0.5;
            for b in 0..7 {
                let a = a0 + b as f32 * TAU / 7.0;
                let p0 = c + vec2(a.cos(), a.sin()) * rad * 0.3;
                let p1 = c + vec2((a + 0.45).cos(), (a + 0.45).sin()) * rad * 0.65;
                let p2 = c + vec2((a + 0.7).cos(), (a + 0.7).sin()) * rad * 0.92;
                p.add(Shape::line(vec![p0, p1, p2], Stroke::new(rad * 0.16, metal())));
            }
            p.circle_filled(c, rad * 0.28, metal());
            p.circle_filled(c, rad * 0.12, if on { pal::cyan() } else { pal::faint() });
        }
    }

    pub fn usb_a(p: &Painter, r: Rect, on: bool) {
        let r = fit(r, 1.6);
        let k = r.width();
        let port = Rect::from_center_size(r.center(), vec2(k * 0.82, k * 0.34));
        p.rect_filled(port, CornerRadius::same(2), metal());
        p.rect_filled(port.shrink(k * 0.04), CornerRadius::same(1), Color32::from_rgb(10, 10, 12));
        let tongue = Rect::from_min_size(pos2(port.left() + k * 0.1, port.top() + k * 0.07), vec2(k * 0.62, k * 0.09));
        p.rect_filled(tongue, CornerRadius::ZERO, if on { Color32::from_rgb(40, 110, 230) } else { metal() });
    }

    pub fn usb_c(p: &Painter, r: Rect, on: bool) {
        let r = fit(r, 1.6);
        let k = r.width();
        let port = Rect::from_center_size(r.center(), vec2(k * 0.78, k * 0.3));
        p.rect_filled(port, CornerRadius::same(255), metal());
        p.rect_filled(port.shrink(k * 0.04), CornerRadius::same(255), Color32::from_rgb(10, 10, 12));
        p.rect_filled(Rect::from_center_size(port.center(), vec2(k * 0.46, k * 0.06)), CornerRadius::same(1), if on { pal::bolt() } else { metal() });
    }

    /// Cor do selo 80 PLUS pelo nome da certificação (None = sem selo).
    pub fn seal_color(rating: &str) -> Option<Color32> {
        let rt = rating.trim().to_lowercase();
        Some(if rt.is_empty() {
            return None;
        } else if rt.contains("titan") {
            Color32::from_rgb(110, 114, 122)
        } else if rt.contains("plat") {
            Color32::from_rgb(170, 182, 196)
        } else if rt.contains("gold") || rt.contains("ouro") {
            Color32::from_rgb(212, 175, 55)
        } else if rt.contains("silver") || rt.contains("prata") {
            Color32::from_rgb(196, 198, 204)
        } else if rt.contains("bronze") {
            Color32::from_rgb(176, 120, 64)
        } else {
            Color32::from_rgb(235, 235, 235)
        })
    }

    /// Fonte ATX vista de lado: ventoinha que gira mais rápido com a carga, conectores e selo 80 PLUS.
    pub fn psu(p: &Painter, r: Rect, on: bool, t: f64, rating: &str, load: f32) {
        let r = fit(r, 1.3);
        let k = r.width();
        let case = r.shrink2(vec2(0.0, k * 0.05));
        p.rect_filled(case, CornerRadius::same(4), metal());
        p.rect_filled(case.shrink(2.0), CornerRadius::same(3), body());
        // ventoinha com grade
        let c = pos2(case.left() + case.width() * 0.4, case.center().y);
        let rad = case.height() * 0.38;
        p.circle_filled(c, rad, Color32::from_rgb(14, 14, 16));
        let ang = if on { (t as f32) * (0.4 + 6.0 * load.clamp(0.0, 1.0)) } else { 0.0 };
        for b in 0..5 {
            let a = ang + b as f32 * TAU / 5.0;
            let p0 = c + vec2(a.cos(), a.sin()) * rad * 0.3;
            let p1 = c + vec2((a + 0.45).cos(), (a + 0.45).sin()) * rad * 0.65;
            let p2 = c + vec2((a + 0.7).cos(), (a + 0.7).sin()) * rad * 0.92;
            p.add(Shape::line(vec![p0, p1, p2], Stroke::new(rad * 0.16, metal().gamma_multiply(0.75))));
        }
        for f in [0.95, 0.66, 0.38] {
            p.circle_stroke(c, rad * f, Stroke::new(0.8, metal()));
        }
        p.line_segment([c - vec2(rad * 0.95, 0.0), c + vec2(rad * 0.95, 0.0)], Stroke::new(0.8, metal()));
        p.line_segment([c - vec2(0.0, rad * 0.95), c + vec2(0.0, rad * 0.95)], Stroke::new(0.8, metal()));
        p.circle_filled(c, rad * 0.24, metal());
        p.circle_filled(c, rad * 0.1, if on { pal::cyan() } else { pal::faint() });
        // conectores modulares (2 × 3)
        let s = k * 0.07;
        let x0 = case.right() - k * 0.24;
        for row in 0..3 {
            for col in 0..2 {
                let m = pos2(x0 + col as f32 * s * 1.35, case.top() + k * 0.1 + row as f32 * s * 1.3);
                p.rect_filled(Rect::from_min_size(m, vec2(s, s)), CornerRadius::same(1), Color32::from_rgb(10, 10, 12));
            }
        }
        // selo 80 PLUS
        if let Some(col) = seal_color(rating) {
            let seal = Rect::from_min_size(pos2(case.right() - k * 0.26, case.bottom() - k * 0.17), vec2(k * 0.22, k * 0.12));
            p.rect_filled(seal, CornerRadius::same(2), col);
            p.text(seal.center(), eframe::egui::Align2::CENTER_CENTER, "80+", eframe::egui::FontId::proportional(k * 0.075), Color32::from_rgb(20, 20, 22));
        }
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
        p.circle_filled(pos, (2.2 + 1.6 * intensity) * 2.4, glow(color, 0.22 * a));
        p.circle_filled(pos, 2.2 + 1.6 * intensity, color.gamma_multiply(a));
    }
}

fn link(p: &egui::Painter, pts: &[Pos2], alive: bool, width: f32, color: Color32) {
    let c = if alive { color.gamma_multiply(0.45) } else { pal::red().gamma_multiply(0.5) };
    if alive {
        p.add(egui::Shape::line(pts.to_vec(), Stroke::new(width + 5.0, glow(color, 0.10))));
        p.add(egui::Shape::line(pts.to_vec(), Stroke::new(width, c)));
    } else {
        for w in pts.windows(2) {
            p.extend(egui::Shape::dashed_line(&[w[0], w[1]], Stroke::new(width.min(2.0), c), 6.0, 5.0));
        }
    }
}

/// Entradas da estimativa de carga da fonte: GPU medida + PD/bateria + portas em uso.
fn psu_inputs(topo: &Topology, samples: &[Sample]) -> crate::psu::Inputs {
    let last = samples.last().copied().unwrap_or_default();
    crate::psu::Inputs {
        gpu_w: last.gpu.and_then(|g| g.tbp).unwrap_or(0.0) as f32,
        pd_on: last.power.map(|p| p.online).unwrap_or(false),
        batt_w: last.power.map(|p| p.charge_mw as f32 / 1000.0).unwrap_or(0.0),
        usb_devices: topo.usb_a.len(),
        tb_in_use: topo.ports.iter().any(|d| d.role == PortRole::TbExtra && d.child.is_some()),
    }
}

fn dname(n: &Node) -> String {
    n.dstate.map(pnp::dstate_name).unwrap_or("?").to_string()
}

fn topo_view(ui: &mut Ui, topo: &Topology, flow: &Flow, gpu_static: Option<&crate::collect::GpuStatic>, sel: &mut Option<Sel>, bench: Option<&BenchResult>, psu: Option<(&crate::psu::Psu, crate::psu::Load)>) {
    let w = ui.available_width();
    // com fonte cadastrada, uma faixa a mais embaixo do dock; a faixa de cima não muda
    let h = if psu.is_some() { 320.0 } else { 260.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    // recorte com folga: as sombras dos nós não são cortadas (a margem do cartão tem 14/18 px)
    let p = ui.painter().with_clip_rect(rect.expand(12.0));
    let t = ui.input(|i| i.time);
    // área clicável com cursor de mão; `lift` anima a sombra do hover (o nó não se mexe: os cabos ficam alinhados)
    let hover = |ui: &mut Ui, r: Rect, id: egui::Id| -> (egui::Response, f32) {
        let resp = ui.interact(r, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let lift = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.15);
        (resp, lift)
    };

    let dock_ok = topo.dock_router.is_some();
    let switch_ok = topo.dock_switch.is_some();
    let gpu_ok = topo.gpu.is_some();

    // geometria
    let host_w = (w * 0.18).clamp(200.0, 240.0);
    let bw = (w * 0.24).clamp(200.0, 300.0);
    let cy = rect.top() + 130.0;
    let host = Rect::from_min_size(pos2(rect.left() + 4.0, cy - 40.0), vec2(host_w, 80.0));
    let bx = rect.right() - bw - 4.0;
    // o dock fica centrado no vão entre o note e os ramos, deixando espaço para os cabos
    let gap = bx - host.right();
    let dock_w = (gap * 0.52).clamp(190.0, 320.0);
    let dock = Rect::from_center_size(pos2(host.right() + gap * 0.5, cy), vec2(dock_w, 136.0));
    let gpu_r = Rect::from_min_size(pos2(bx, rect.top() + 2.0), vec2(bw, 78.0));
    let usb_r = Rect::from_min_size(pos2(bx, rect.top() + 91.0), vec2(bw, 78.0));
    let tb_r = Rect::from_min_size(pos2(bx, rect.top() + 180.0), vec2(bw, 78.0));

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
            pulses(&p, &pts, t + l as f64 * 0.37, intensity, pal::data(), false);
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
        Some(0x8) => format!("Cabo a {} Gb/s · o dock aceita 40", lanes * 10),
        Some(0x4) => format!("Cabo a {} Gb/s", lanes * 20),
        Some(x) => format!("Cabo conectado (código {x})"),
        None if dock_ok => "Cabo conectado".into(),
        None => "Sem conexão".into(),
    };
    let narrow = b.x - a.x < 190.0;
    let cable_txt = if narrow { cable_txt.split(" · ").next().unwrap_or_default().to_string() } else { cable_txt };
    p.text(mid + vec2(0.0, -30.0), egui::Align2::CENTER_BOTTOM, cable_txt, FontId::proportional(13.5), if dock_ok { pal::bolt() } else { pal::red() });
    p.text(mid + vec2(0.0, 34.0), egui::Align2::CENTER_TOP, if flow.pd_on { if narrow { "Carregando" } else { "Carregando o notebook · até 85 W" } } else { "O dock não está carregando o notebook" }, FontId::proportional(12.5), if flow.pd_on { pal::bolt().gamma_multiply(0.8) } else { pal::amber() });
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
        pulses(&p, &gpu_pts, t, intensity, pal::data(), false);
        if flow.d3 > 1.0 {
            pulses(&p, &gpu_pts, t + 0.5, (flow.d3 / 100.0) * 0.4, pal::violet(), true);
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

    // --- fonte do dock: ramo vertical de energia subindo para o dock (só com fonte cadastrada)
    let psu_r = Rect::from_center_size(pos2(dock.center().x, dock.bottom() + 40.0 + 39.0), vec2(dock.width().min(300.0), 78.0));
    if let Some((_, l)) = &psu {
        let f = l.frac.clamp(0.0, 1.0);
        // entra no dock à direita das bolinhas das portas
        let lx = (dock.left() + 128.0).max(dock.center().x).min(dock.right() - 24.0);
        let pts = if (lx - psu_r.center().x).abs() < 1.0 {
            vec![pos2(lx, psu_r.top()), pos2(lx, dock.bottom())]
        } else {
            vec![pos2(psu_r.center().x, psu_r.top()), pos2(psu_r.center().x, psu_r.top() - 14.0), pos2(lx, psu_r.top() - 14.0), pos2(lx, dock.bottom())]
        };
        p.add(egui::Shape::line(pts.clone(), Stroke::new(8.0 + 6.0 * f, pal::bolt().gamma_multiply(0.08))));
        link(&p, &pts, true, 2.0 + 4.0 * f, pal::bolt());
        pulses(&p, &pts, t, f.max(0.08), pal::bolt(), false);
    }

    // --- nós
    let hostn = topo.host_router.as_ref();
    let host_lines = vec!["Thunderbolt 4".to_string(), format!("Firmware {}", hostn.and_then(|n| n.firmware.clone()).unwrap_or("?".into()))];
    let (hr, lift) = hover(ui, host, ui.id().with("host"));
    node_box(&p, host, "Notebook", &host_lines, pal::intel(), if hostn.is_some() { pal::green() } else { pal::amber() }, *sel == Some(Sel::Host), false, lift, t, Some(&|p: &egui::Painter, r: Rect| art::laptop(p, r, hostn.is_some(), t)));
    if hr.clicked() {
        *sel = Some(Sel::Host);
    }

    let dr = topo.dock_router.as_ref();
    let fw = dr.and_then(|n| n.firmware.clone()).unwrap_or("—".into());
    let model = topo.dock_usb4.iter().find(|p| p.pid == 10).map(|p| p.val.show()).unwrap_or("—".into());
    let mut dock_lines = vec![format!("Thunderbolt 3 · firmware {fw}")];
    let _ = &model;
    match dr.and_then(|n| n.last_arrival) {
        Some(ts) => dock_lines.push(format!("Conectado há {}", fmt_dur(now_ms() - ts))),
        None => dock_lines.push("Não encontrado".into()),
    }
    if let Some(br) = bench {
        dock_lines.push(format!("Banda medida: {:.1} GB/s", br.h2d_gbs.max(br.d2h_gbs)).replace('.', ","));
    }
    let (dkr, lift) = hover(ui, dock, ui.id().with("dock"));
    node_box(&p, dock, "Dock WKG-L19", &dock_lines, pal::cyan(), if dock_ok && switch_ok { pal::green() } else { pal::red() }, *sel == Some(Sel::Dock), true, lift, t, Some(&|p: &egui::Painter, r: Rect| art::dock(p, r.with_max_y(r.bottom() - 26.0), dock_ok, t)));
    // bolinhas de estado das portas downstream (verde = D0 ativo, cinza = D3 dormindo)
    for (i, dp) in topo.ports.iter().enumerate() {
        let c = pos2(dock.left() + 22.0 + i as f32 * 34.0, dock.bottom() - 16.0);
        let on = dp.node.dstate == Some(1);
        let col = if dp.node.problem != 0 { pal::red() } else if on { pal::green() } else { pal::faint() };
        if on {
            let k = (t * 2.0 + i as f64).sin() as f32 * 0.5 + 0.5;
            p.circle_filled(c, 7.0 + 2.0 * k, glow(col, 0.25));
        }
        p.circle_filled(c, 5.0, col);
        let icon = match dp.role {
            PortRole::Gpu => "🎮",
            PortRole::UsbA => "🖴",
            _ => "⚡",
        };
        p.text(c + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, icon, FontId::proportional(11.5), pal::dim());
        let hit = Rect::from_center_size(c + vec2(6.0, 0.0), vec2(26.0, 18.0));
        ui.interact(hit, ui.id().with(("dport", i)), Sense::hover()).on_hover_text(format!("porta downstream {} · {}", i + 1, dname(&dp.node)));
    }
    if dkr.clicked() {
        *sel = Some(Sel::Dock);
    }

    // GPU
    let gl = if let Some(g) = &topo.gpu {
        let gm = flow.gpu.unwrap_or_default();
        let _ = g;
        vec![
            format!("{:.0} °C · {:.0} W", gm.temp.unwrap_or(0.0), gm.tbp.unwrap_or(0.0)),
            match gpu_static {
                Some(s) => format!("{} x{}", s.bus, s.lanes),
                None => "…".into(),
            },
        ]
    } else {
        vec!["Não encontrada".into()]
    };
    let gm = flow.gpu.unwrap_or_default();
    let spin = gm.fan.map(|f| f as f32 / 3200.0).unwrap_or(gm.usage.unwrap_or(0.0) as f32 / 100.0);
    let (gr, lift) = hover(ui, gpu_r, ui.id().with("gpu"));
    node_box(&p, gpu_r, "Radeon RX 7600 XT", &gl, pal::data(), if gpu_ok { pal::green() } else { pal::red() }, *sel == Some(Sel::Gpu), false, lift, t, Some(&|p: &egui::Painter, r: Rect| art::gpu(p, r, spin, gpu_ok, t)));
    if gr.clicked() {
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
                    l.push("Livre".into());
                } else {
                    for d in topo.usb_a.iter().take(2) {
                        l.push(d.name.clone());
                    }
                }
                ("USB-A 10 Gb/s", l, pal::green())
            }
            _ => (
                "TB extra 15 W",
                vec![if dp.child.is_some() { "Em uso".into() } else { "Livre".into() }],
                pal::faint(),
            ),
        };
        let busy = dp.role == PortRole::UsbA && !topo.usb_a.is_empty() || dp.child.is_some();
        let draw: &dyn Fn(&egui::Painter, Rect) = if dp.role == PortRole::UsbA { &|p, r| art::usb_a(p, r, busy) } else { &|p, r| art::usb_c(p, r, busy) };
        let (pr, lift) = hover(ui, *r, ui.id().with(("port", i)));
        node_box(&p, *r, title, &lines, accent, if dp.node.problem == 0 { pal::green() } else { pal::red() }, *sel == Some(Sel::Port(i)), false, lift, t, Some(draw));
        if pr.clicked() {
            *sel = Some(Sel::Port(i));
        }
    }
    if ports.iter().all(|(d, _)| d.role != PortRole::UsbA) {
        node_box(&p, usb_r, "USB-A 10 Gb/s", &["não detectada".into()], pal::green(), pal::red(), false, false, 0.0, t, Some(&|p: &egui::Painter, r: Rect| art::usb_a(p, r, false)));
    }
    if ports.iter().all(|(d, _)| d.role != PortRole::TbExtra) {
        node_box(&p, tb_r, "TB extra 15 W", &["não detectada".into()], pal::faint(), pal::amber(), false, false, 0.0, t, Some(&|p: &egui::Painter, r: Rect| art::usb_c(p, r, false)));
    }

    // Fonte do dock
    if let Some((ps, l)) = &psu {
        let f = l.frac.clamp(0.0, 1.0);
        let wf = l.worst / ps.watts as f32;
        let title = if ps.model.trim().is_empty() { "Fonte do dock" } else { ps.model.trim() };
        let lines = [format!("≈ {:.0} W de {} W", l.now, ps.watts)];
        let state = if wf < 0.8 { pal::green() } else if wf < 1.0 { pal::amber() } else { pal::red() };
        let (psr, lift) = hover(ui, psu_r, ui.id().with("psu"));
        node_box_r(&p, psu_r, title, &lines, pal::bolt(), state, *sel == Some(Sel::Psu), false, lift, t, Some(&|p: &egui::Painter, r: Rect| art::psu(p, r, true, t, &ps.rating, f)), 16.0);
        // mini medidor: ▮ carga agora · ◆ pior caso
        let aw = (psu_r.width() * 0.3).clamp(44.0, 74.0);
        let x0 = psu_r.left() + aw + 22.0;
        let track = Rect::from_min_max(pos2(x0, psu_r.bottom() - 15.0), pos2(psu_r.right() - 46.0, psu_r.bottom() - 9.0));
        let fc = if f < 0.6 { pal::green() } else if f < 0.8 { pal::amber() } else { pal::red() };
        p.rect_filled(track, CornerRadius::same(3), pal::well());
        p.rect_stroke(track, CornerRadius::same(3), Stroke::new(1.0, pal::border()), StrokeKind::Outside);
        if f > 0.004 {
            p.rect_filled(Rect::from_min_max(track.min, pos2(track.left() + track.width() * f, track.bottom())), CornerRadius::same(3), fc);
        }
        for m in [0.6, 0.8] {
            let x = track.left() + track.width() * m;
            p.line_segment([pos2(x, track.center().y - 4.0), pos2(x, track.center().y + 4.0)], Stroke::new(1.0, pal::faint()));
        }
        let wx = track.left() + track.width() * wf.clamp(0.0, 1.0);
        let wy = track.center().y;
        p.add(egui::Shape::convex_polygon(vec![pos2(wx, wy - 5.0), pos2(wx + 4.0, wy), pos2(wx, wy + 5.0), pos2(wx - 4.0, wy)], pal::dim(), Stroke::new(1.0, pal::card2())));
        p.text(pos2(track.right() + 8.0, track.center().y), egui::Align2::LEFT_CENTER, format!("{:.0} %", f * 100.0), FontId::proportional(12.5), fc);
        let r = psr.on_hover_text("Estimativa: GPU (medida) + notebook + dock. A fonte não manda dados ao PC.\n▮ agora · ◆ pior caso");
        if r.clicked() {
            *sel = Some(Sel::Psu);
        }
    }
}

// ---------------------------------------------------------------------------

impl eframe::App for App {
    fn ui(&mut self, root: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        let cfg = crate::config::get();
        if (ctx.zoom_factor() - cfg.zoom).abs() > 0.001 {
            ctx.set_zoom_factor(cfg.zoom);
        }
        self.refresh_history();
        let (topo, samples, gpu_static, notes) = collect::with_live(|l| (l.topo.clone(), l.samples.iter().copied().collect::<Vec<Sample>>(), l.gpu_static.clone(), l.notes.clone()))
            .unwrap_or_default();
        let topo = topo.unwrap_or_default();
        if topo.read_ms != 0 {
            if let Some(v) = vendor_of(&topo) {
                self.vendor = Some(v);
            }
        }
        let th = resolve_theme(cfg.theme, self.vendor);
        if th != pal::current() {
            apply_theme(&ctx, th);
        }
        let last = samples.last().copied().unwrap_or_default();
        let flow = Flow { copy: last.copy, d3: last.d3, pd_on: last.power.map(|p| p.online).unwrap_or(true), gpu: last.gpu };
        let bench = self.bench.lock().unwrap().clone();
        let bench_done = if let BenchState::Done(r) = &bench { Some(r.clone()) } else { None };
        let t = ctx.input(|i| i.time);

        // ---------------- cabeçalho
        if th == Th::Amd {
            egui::Panel::top("amd-bar")
                .exact_size(60.0)
                .frame(egui::Frame::new().fill(Color32::from_rgb(32, 32, 34)))
                .show_inside(root, |ui| self.amd_bar(ui, &topo, &last, t));
        } else {
        egui::Panel::top("top").frame(egui::Frame::new().fill(if th.nav_left() { pal::well() } else { pal::bg() }).inner_margin(egui::Margin::symmetric(16, 10))).show_inside(root, |ui| {
            ui.horizontal(|ui| {
                logo(ui, topo.dock_router.is_some());
                ui.label(egui::RichText::new("eGPU Painel").size(20.0).strong().color(pal::fg()));
                if cfg.theme == 0 && self.vendor.is_some() {
                    pill(ui, th.name(), pal::cyan());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("{:.0} MB · {:.1}%", last.ws_mb, last.cpu_pct)).color(pal::faint()).size(12.5))
                        .on_hover_text("Consumo do próprio painel (RAM e CPU). Fechando a janela, ele volta para a bandeja e para de coletar.");
                    ui.add_space(pal::gap());
                    self.header_status(ui, &topo, t);
                });
            });
        });

        }
        // ---------------- navegação: abas no topo (claro/escuro), barra AMD ou menu lateral (NVIDIA/Intel)
        if th == Th::Amd {
        } else if th.nav_left() {
            egui::Panel::left("nav")
                .resizable(false)
                .exact_size(210.0)
                .frame(egui::Frame::new().fill(pal::well()).inner_margin(egui::Margin::symmetric(10, 12)))
                .show_inside(root, |ui| self.nav_left(ui));
        } else {
            egui::Panel::top("tabs")
                .frame(egui::Frame::new().fill(pal::bg()).inner_margin(egui::Margin { left: 16, right: 16, top: 0, bottom: 0 }))
                .show_inside(root, |ui| self.nav_top(ui, th));
        }

        // ---------------- painel lateral (detalhes / explicações)
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.sel = None;
        }
        if self.sel != self.sel_prev {
            self.sel_at = t;
            self.sel_prev = self.sel;
        }
        if self.sel.is_some() {
            // entrada curta (220 ms, ease-out): desliza da esquerda e aparece
            let k = ((t - self.sel_at) / 0.22).clamp(0.0, 1.0) as f32;
            let e = 1.0 - (1.0 - k).powi(3);
            if k < 1.0 {
                ctx.request_repaint();
            }
            egui::Panel::right("detalhes")
                .resizable(true)
                .default_size(430.0)
                .min_size(360.0)
                .max_size(560.0)
                .frame(egui::Frame::new().fill(pal::card()).inner_margin(egui::Margin::same(10)).stroke(Stroke::new(1.0, pal::border())))
                .show_inside(root, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        // margem para as sombras dos cartões não serem cortadas pela rolagem
                        egui::Frame::new()
                            .outer_margin(egui::Margin { left: 4 + ((1.0 - e) * 28.0) as i8, right: 8, top: 4, bottom: 12 })
                            .show(ui, |ui| {
                                ui.set_opacity(e);
                                self.details(ui, &topo, gpu_static.as_ref(), &samples, bench_done.as_ref(), &ctx)
                            });
                    });
                });
        }

        let cm = if th == Th::Amd { egui::Margin::same(20) } else { egui::Margin::symmetric(16, 8) };
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal::bg()).inner_margin(cm)).show_inside(root, |ui| {
            // troca de aba com fade-in suave
            let a = ctx.animate_bool_with_time(egui::Id::new(("tab-fade", self.tab as u8)), true, 0.25);
            ui.set_opacity(a);
            egui::ScrollArea::vertical().id_salt(("scroll", self.tab as u8)).auto_shrink([false, false]).show(ui, |ui| {
                match self.tab {
                    Tab::Geral => {
                        if let Some(v) = self.vendor {
                            brand(ui, v, &mut self.logo_tex);
                        }
                        self.verdict_banner(ui, &topo);
                        ui.add_space(pal::gap());
                        card(ui, "🔗  Topologia", pal::cyan(), |ui| {
                            let mut sel = self.sel;
                            let pl = (self.psu.watts > 0).then(|| (&self.psu, crate::psu::compute(&self.psu, &psu_inputs(&topo, &samples))));
                            topo_view(ui, &topo, &flow, gpu_static.as_ref(), &mut sel, bench_done.as_ref(), pl);
                            if sel != self.sel {
                                self.sel = sel;
                            }
                            hidden_strip(ui, &mut self.sel);
                        });
                        ui.add_space(pal::gap());
                        self.overview_rings(ui, &topo, &samples, bench_done.as_ref());
                        ui.add_space(pal::gap());
                        self.card_timeline(ui, true);
                    }
                    Tab::Enlace => {
                        let cols = if ui.available_width() > 1150.0 { 3 } else { 2 };
                        ui.spacing_mut().item_spacing.x = pal::gap();
                        ui.columns(cols, |c| {
                            self.card_link(&mut c[0], &topo, bench_done.as_ref());
                            let (pi, si) = if cols == 3 { (1, 2) } else { (1, 1) };
                            self.card_power(&mut c[pi], &samples);
                            c[si].add_space(if cols == 3 { 0.0 } else { 10.0 });
                            self.card_stability(&mut c[si], &topo);
                        });
                        ui.add_space(pal::gap());
                        self.card_psu(ui, &topo, &samples);
                    }
                    Tab::Fluxo => {
                        ui.spacing_mut().item_spacing.x = pal::gap();
                        ui.columns(2, |c| {
                            self.card_flow(&mut c[0], &samples, &bench, &ctx);
                            self.card_gpu(&mut c[1], &samples, gpu_static.as_ref());
                        });
                    }
                    Tab::Historico => self.card_timeline(ui, false),
                    Tab::Opcoes => self.page_options(ui, &ctx, th),
                }
                for n in &notes {
                    ui.label(egui::RichText::new(format!("⚠ {n}")).color(pal::amber()).size(13.0));
                }
                ui.add_space(14.0);
            });
        });

        let busy = flow.copy > 2.0 || flow.d3 > 5.0 || matches!(bench, BenchState::Running(_));
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let ms = match (cfg.eco || !focused, busy) {
            (true, _) => 200,
            (false, true) => 33,
            (false, false) => 66,
        };
        ctx.request_repaint_after(Duration::from_millis(ms));
    }
}

impl App {
    /// Abas no topo, no estilo do Adrenalin: rótulos em caixa alta e sublinhado deslizante.
    fn nav_top(&mut self, ui: &mut Ui, th: Th) {
        let mut active_rect = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 22.0;
            for tab in Tab::ALL {
                let on = self.tab == tab;
                let txt = if th == Th::Amd { tab.label().to_uppercase() } else { tab.label().to_string() };
                let r = ui.add(
                    egui::Label::new(egui::RichText::new(format!("{}  {}", tab.icon(), txt)).size(13.5).strong().color(if on { pal::fg() } else { pal::dim() }))
                        .sense(Sense::click()),
                );
                if r.hovered() && !on {
                    ui.painter().line_segment([r.rect.left_bottom() + vec2(0.0, 7.0), r.rect.right_bottom() + vec2(0.0, 7.0)], Stroke::new(2.0, pal::border()));
                }
                let rr = r.rect;
                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.tab = tab;
                }
                if on {
                    active_rect = Some(rr);
                }
            }
        });
        ui.add_space(8.0);
        // sublinhado animado da aba ativa
        if let Some(r) = active_rect {
            let x0 = ui.ctx().animate_value_with_time(egui::Id::new("tab-x0"), r.left(), 0.2);
            let x1 = ui.ctx().animate_value_with_time(egui::Id::new("tab-x1"), r.right(), 0.2);
            let y = ui.min_rect().bottom() - 1.0;
            ui.painter().line_segment([pos2(ui.max_rect().left(), y), pos2(ui.max_rect().right(), y)], Stroke::new(1.0, pal::border()));
            ui.painter().line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(3.0, pal::cyan()));
        }
    }

    /// Barra do tema AMD, no molde do Adrenalin: bloco vermelho com a marca, abas grandes com
    /// fundo claro + sublinhado vermelho na ativa, estado do dock à direita.
    fn amd_bar(&mut self, ui: &mut Ui, topo: &Topology, last: &Sample, t: f64) {
        let full = ui.max_rect();
        let p = ui.painter().clone();
        // bloco vermelho com a marca do painel (desenho próprio: dois nós ligados)
        let blk = Rect::from_min_size(full.min, vec2(64.0, full.height()));
        p.rect_filled(blk, CornerRadius::ZERO, pal::cyan());
        let a = blk.center() + vec2(-10.0, 9.0);
        let b = blk.center() + vec2(10.0, -9.0);
        p.line_segment([a, b], Stroke::new(3.0, Color32::WHITE));
        p.circle_filled(a, 6.0, Color32::WHITE);
        p.circle_stroke(b, 6.0, Stroke::new(3.0, Color32::WHITE));
        let mut x = blk.right() + 8.0;
        let mut active = None;
        for tab in Tab::ALL {
            let on = self.tab == tab;
            let g = ui.painter().layout_no_wrap(tab.label().to_string(), FontId::proportional(19.0), Color32::WHITE);
            let r = Rect::from_min_size(pos2(x, full.top()), vec2(g.size().x + 40.0, full.height()));
            let resp = ui.interact(r, ui.id().with(("amdtab", tab as u8)), Sense::click());
            let h = ui.ctx().animate_bool_with_time(resp.id, on || resp.hovered(), 0.15);
            if h > 0.0 {
                p.rect_filled(r, CornerRadius::ZERO, Color32::from_rgb(46, 46, 49).gamma_multiply(h));
            }
            let col = if on { Color32::WHITE } else if resp.hovered() { Color32::from_gray(225) } else { Color32::from_gray(175) };
            p.galley_with_override_text_color(pos2(r.left() + 20.0, r.center().y - g.size().y / 2.0), g.clone(), col);
            if on {
                active = Some(Rect::from_min_max(pos2(r.left() + 20.0, r.bottom() - 6.0), pos2(r.right() - 20.0, r.bottom() - 3.0)));
            }
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.tab = tab;
            }
            x = r.right();
        }
        if let Some(u) = active {
            let x0 = ui.ctx().animate_value_with_time(egui::Id::new("amd-u0"), u.left(), 0.2);
            let x1 = ui.ctx().animate_value_with_time(egui::Id::new("amd-u1"), u.right(), 0.2);
            p.rect_filled(Rect::from_min_max(pos2(x0, u.top()), pos2(x1, u.bottom())), CornerRadius::ZERO, pal::cyan());
        }
        // direita: estado do dock e consumo do painel
        let right = Rect::from_min_max(pos2(x + 10.0, full.top()), full.max - vec2(16.0, 0.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
            ui.label(egui::RichText::new(format!("{:.0} MB · {:.1}%", last.ws_mb, last.cpu_pct)).color(Color32::from_gray(130)).size(12.5))
                .on_hover_text("Consumo do próprio painel (RAM e CPU).");
            ui.add_space(12.0);
            self.header_status(ui, &topo, t);
        });
    }

    /// Fonte do dock: modelo digitado, specs buscadas na web, carga estimada em tempo real.
    fn card_psu(&mut self, ui: &mut Ui, topo: &Topology, samples: &[Sample]) {
        use crate::psu;
        // resultado da busca em segundo plano
        if let Some(job) = &self.psu_job {
            let done = job.lock().unwrap().take();
            if let Some(r) = done {
                self.psu_job = None;
                match r {
                    Ok(f) => {
                        let mut got = vec![];
                        if let Some(w) = f.watts {
                            self.psu.watts = w;
                            got.push(format!("{w} W"));
                        }
                        if let Some(a) = f.rail12_a {
                            self.psu.rail12_a = a;
                            got.push(format!("+12 V {a:.0} A"));
                        }
                        if let Some(r) = &f.rating {
                            self.psu.rating = r.clone();
                            got.push(format!("80 PLUS {r}"));
                        }
                        self.psu.origin = "web".into();
                        psu::save(&self.psu);
                        self.psu_msg = Some((true, format!("Achei no {}: {}. Confira na etiqueta da fonte e corrija se precisar.", f.engine, got.join(" · "))));
                    }
                    Err(e) => self.psu_msg = Some((false, format!("Não achei as specs ({e}). Preencha à mão, a etiqueta da fonte tem tudo."))),
                }
            }
        }
        let inp = psu_inputs(topo, samples);
        let before = self.psu.clone();
        card(ui, "🔌  Fonte do dock", pal::bolt(), |ui| {
            let avail = ui.available_width();
            let (n, cw) = psu_cols(avail);
            let gap = pal::gap();
            ui.spacing_mut().item_spacing = vec2(gap, 8.0);
            ui.spacing_mut().interact_size.y = 32.0;
            // ── linha 1: modelo + buscar (o botão cai na última coluna da grade)
            let busy = self.psu_job.is_some();
            let can = !busy && self.psu.model.trim().len() >= 3;
            let model_w = if n >= 4 { avail - cw - gap } else { avail };
            let mut go = false;
            let mut enter = false;
            let search_btn = |ui: &mut Ui, w: f32| {
                let txt = if busy { "      Buscando…" } else { "🔍  Buscar specs" };
                let b = ui.add_enabled(can, egui::Button::new(egui::RichText::new(txt).size(14.0)).min_size(vec2(w, 32.0)));
                if busy {
                    let c = b.rect.center() - vec2(44.0, 0.0);
                    egui::Spinner::new().size(16.0).paint_at(ui, Rect::from_center_size(c, vec2(16.0, 16.0)));
                }
                b.clicked()
            };
            ui.allocate_ui_with_layout(vec2(avail, 54.0), egui::Layout::left_to_right(egui::Align::Max), |ui| {
                field(ui, model_w, "🏷", "Modelo", |ui| {
                    let te = ui.add(
                        egui::TextEdit::singleline(&mut self.psu.model)
                            .hint_text("ex.: Corsair RM650x, MSI MAG A650BN")
                            .desired_width(f32::INFINITY)
                            .min_size(vec2(model_w, 32.0))
                            .vertical_align(egui::Align::Center),
                    );
                    enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                });
                if n >= 4 {
                    field(ui, cw, "", "", |ui| go = search_btn(ui, cw));
                }
            });
            if n < 4 {
                go = search_btn(ui, avail);
            }
            if go || (enter && can) {
                let slot = Arc::new(Mutex::new(None));
                let s2 = slot.clone();
                let model = self.psu.model.trim().to_string();
                let ctx = ui.ctx().clone();
                std::thread::spawn(move || {
                    let r = psu::lookup(&model);
                    *s2.lock().unwrap() = Some(r);
                    ctx.request_repaint();
                });
                self.psu_job = Some(slot);
                self.psu_msg = None;
            }
            // ── linha 2: ficha da fonte, uma célula por coluna
            let psu_v = &mut self.psu;
            let mut cell = |ui: &mut Ui, i: usize| match i {
                0 => field(ui, cw, "⚡", "Potência", |ui| {
                    ui.add_sized([cw, 32.0], egui::DragValue::new(&mut psu_v.watts).range(0..=2000).suffix(" W").speed(5));
                }),
                1 => field(ui, cw, "⎓", "Linha +12 V", |ui| {
                    ui.add_sized([cw, 32.0], egui::DragValue::new(&mut psu_v.rail12_a).range(0.0..=170.0).suffix(" A").speed(0.5));
                }),
                2 => field(ui, cw, "★", "Selo 80 PLUS", |ui| {
                    egui::ComboBox::from_id_salt("psu-rating")
                        .width(cw - 8.0)
                        .selected_text(if psu_v.rating.is_empty() { "sem selo / não sei" } else { psu_v.rating.as_str() })
                        .show_ui(ui, |ui| {
                            for r in psu::RATINGS {
                                ui.selectable_value(&mut psu_v.rating, r.to_string(), if r.is_empty() { "sem selo / não sei" } else { r });
                            }
                        });
                }),
                _ => field(ui, cw, "ⓘ", "Origem", |ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(cw, 32.0), Sense::hover());
                    ui.painter().rect_filled(r, CornerRadius::same(pal::radius()), pal::card2());
                    let t = if psu_v.origin == "web" { "🌐  da internet" } else { "✍  preenchido à mão" };
                    ui.painter().text(r.left_center() + vec2(12.0, 0.0), egui::Align2::LEFT_CENTER, t, FontId::proportional(14.0), pal::dim());
                }),
            };
            for row in [0usize, 1, 2, 3].chunks(n) {
                ui.horizontal(|ui| {
                    for &i in row {
                        cell(ui, i);
                    }
                });
            }
            if let Some((ok, m)) = &self.psu_msg {
                ui.add(egui::Label::new(egui::RichText::new(m).color(if *ok { pal::green() } else { pal::amber() }).size(13.0)).wrap());
            }
            // ── separador
            ui.add_space(10.0);
            let (hl, _) = ui.allocate_exact_size(vec2(avail, 1.0), Sense::hover());
            ui.painter().rect_filled(hl, CornerRadius::ZERO, pal::grid());
            ui.add_space(10.0);
            if self.psu.watts == 0 {
                empty_psu(ui, avail);
                return;
            }
            let l = psu::compute(&self.psu, &inp);
            let w = self.psu.watts as f32;
            // ── conclusão primeiro
            if l.spike > w {
                verdict_strip(ui, pal::red(), "✖", &format!("Os picos da placa (~{:.0} W) passam dos {:.0} W da fonte: o dock pode desligar inteiro, PD junto.", l.spike, w));
            } else if l.worst > w * 0.85 {
                verdict_strip(ui, pal::amber(), "▲", &format!("No pior caso a fonte chega a ~{:.0}%. Funciona, com pouca folga.", l.worst / w * 100.0));
            } else {
                verdict_strip(ui, pal::green(), "✔", &format!("Folga boa: no pior caso a fonte fica em ~{:.0}%.", l.worst / w * 100.0));
            }
            ui.add_space(6.0);
            load_bar(ui, &l, w, avail);
            ui.add_space(10.0);
            // ── medidores: o centro de cada um cai no centro da coluna dos campos acima
            let lc = |f: f32| if f > 0.8 { pal::red() } else if f > 0.6 { pal::amber() } else { pal::green() };
            let rail = self.psu.rail12_a;
            let rating = if self.psu.rating.is_empty() { "sem selo".to_string() } else { self.psu.rating.clone() };
            let gauge_at = |ui: &mut Ui, i: usize| {
                ui.vertical_centered(|ui| match i {
                    0 => {
                        gauge(ui, "⚡", l.now, w, &format!("W · {:.0}%", l.frac * 100.0), "Carga agora", lc(l.frac)).on_hover_text(format!(
                            "Placa {:.0} W (medido) + notebook pelo PD ~{:.0} W + dock ~{:.0} W + portas ~{:.0} W (estimados)",
                            l.gpu, l.pd, l.dock, l.usb
                        ));
                    }
                    1 => {
                        gauge(ui, "▲", l.spike, w, "W", "Pico possível", lc(l.spike / w)).on_hover_text(format!(
                            "A placa pode puxar picos de ~1,6× o limite ({:.0} W) por milissegundos, somados ao resto. Fonte fraca desarma nesses picos.",
                            psu::GPU_TBP_MAX
                        ));
                    }
                    2 => {
                        gauge(ui, "🔌", l.wall, w * 1.2, "W", "Na tomada (est.)", pal::data()).on_hover_text(format!("Eficiência estimada {:.0}% ({rating})", l.eff * 100.0));
                    }
                    _ => {
                        if rail > 0.0 {
                            gauge(ui, "⎓", l.amps12, rail, "A", "Linha +12 V", lc(l.amps12 / rail)).on_hover_text("Corrente estimada na linha de +12 V, contra o máximo da etiqueta da fonte.");
                        } else {
                            gauge(ui, "⎓", f32::NAN, 1.0, "A", "informe a +12 V", pal::dim()).on_hover_text("Preencha a corrente da linha +12 V (está na etiqueta da fonte) para ver este medidor.");
                        }
                    }
                });
            };
            for row in [0usize, 1, 2, 3].chunks(n) {
                ui.columns(n, |c| {
                    for (k, &i) in row.iter().enumerate() {
                        gauge_at(&mut c[k], i);
                    }
                });
            }
            ui.add_space(4.0);
            ui.add(egui::Label::new(egui::RichText::new("ⓘ Placa: medida. Notebook, dock e portas: estimados (a fonte não manda dados).").color(pal::faint()).size(12.0)).wrap());
        });
        if self.psu != before {
            if self.psu.model != before.model {
                self.psu.origin = "manual".into();
            }
            crate::psu::save(&self.psu);
        }
    }

    /// Veredito em uma frase: está bem, instável ou caído — e a causa provável.
    fn verdict(&self, topo: &Topology) -> (u8, String, String) {
        let now = now_ms();
        if topo.read_ms == 0 {
            return (0, "Lendo o estado do dock…".into(), String::new());
        }
        let drops: Vec<&Incident> = self.incidents.iter().filter(|i| i.is_drop).collect();
        let last = drops.last();
        if topo.dock_router.is_none() {
            return (2, "O dock está desconectado".into(), "Confira o cabo e se a fonte do dock está ligada na tomada.".into());
        }
        let d24 = drops.iter().filter(|i| now - i.start <= 86_400_000).count();
        if d24 > 0 {
            let cause = match last {
                Some(i) if i.pd_lost => "Na última queda a energia do dock caiu junto: suspeite da fonte ou da tomada.",
                Some(_) => "Na última queda só a conexão caiu, a energia continuou: suspeite do cabo.",
                None => "",
            };
            let lvl = if d24 >= 3 { 2 } else { 1 };
            let vezes = if d24 == 1 { "1 vez".to_string() } else { format!("{d24} vezes") };
            return (lvl, format!("Instável: o dock caiu {vezes} nas últimas 24 h"), cause.into());
        }
        let sub = match last {
            Some(i) => format!("Sem quedas há {}.", fmt_dur(now - i.start)),
            None => "Nenhuma queda registrada.".into(),
        };
        (0, "Tudo certo com o dock".into(), sub)
    }

    fn verdict_banner(&mut self, ui: &mut Ui, topo: &Topology) {
        let (lvl, title, sub) = self.verdict(topo);
        let (col, icon) = match lvl {
            0 => (pal::green(), "✔"),
            1 => (pal::amber(), "▲"),
            _ => (pal::red(), "✖"),
        };
        // halo colorido só quando há algo a ver; no verde, a sombra comum de cartão
        let light = pal::current().is_light();
        let sh = if lvl > 0 { egui::Shadow { offset: [0, 4], blur: 20, spread: 0, color: col.gamma_multiply(if light { 0.10 } else { 0.18 }) } } else { pal::elev(1, 0.0) };
        let r = egui::Frame::new()
            .fill(pal::bg().lerp_to_gamma(col, 0.10)) // opaco: a sombra não escurece o miolo
            .shadow(sh)
            .stroke(Stroke::new(1.0, col.gamma_multiply(0.55)))
            .corner_radius(CornerRadius::same(pal::radius()))
            .inner_margin(egui::Margin::symmetric(20, 14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(icon).size(30.0).color(col));
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&title).size(20.0).color(pal::fg()));
                        if !sub.is_empty() {
                            ui.label(egui::RichText::new(&sub).size(14.5).color(pal::dim()));
                        }
                    });
                    if lvl > 0 {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new("Ver o que aconteceu ›").size(14.0).color(col));
                        });
                    }
                });
            })
            .response;
        let c = ui.interact(r.rect, ui.id().with("veredito"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if c.clicked() {
            self.tab = if lvl > 0 { Tab::Historico } else { Tab::Enlace };
        }
    }

    /// Pílula do cabeçalho: segue o veredito (verde / âmbar / vermelho), com ícone além da cor.
    fn header_status(&mut self, ui: &mut Ui, topo: &Topology, t: f64) {
        let (lvl, _, _) = self.verdict(topo);
        let now = now_ms();
        let d24 = self.incidents.iter().filter(|i| i.is_drop && now - i.start <= 86_400_000).count();
        let (txt, col, pulse) = match (&topo.dock_router, lvl) {
            (None, _) if topo.read_ms == 0 => ("Procurando o dock…".to_string(), pal::dim(), false),
            (None, _) => ("✖ Dock desconectado".to_string(), pal::red(), true),
            (Some(n), 0) => (format!("✔ Dock conectado · {}", n.last_arrival.map(|a| fmt_dur(now - a)).unwrap_or_default()), pal::green(), false),
            (Some(_), l) => (format!("▲ Instável · {d24} queda{} em 24 h", if d24 == 1 { "" } else { "s" }), if l >= 2 { pal::red() } else { pal::amber() }, false),
        };
        if status_pill(ui, &txt, col, t, pulse).clicked() {
            self.tab = if lvl > 0 { Tab::Historico } else { Tab::Geral };
        }
    }

    /// Menu lateral (NVIDIA/Intel): ícone + rótulo, item ativo com barra de destaque.
    fn nav_left(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        for tab in Tab::ALL {
            let on = self.tab == tab;
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
            let hov = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || on, 0.15);
            if hov > 0.0 {
                ui.painter().rect_filled(rect, CornerRadius::same(6), pal::card2().gamma_multiply(hov));
            }
            if on {
                ui.painter().rect_filled(Rect::from_min_size(rect.min + vec2(0.0, 8.0), vec2(3.0, rect.height() - 16.0)), CornerRadius::same(2), pal::cyan());
            }
            ui.painter().text(rect.left_center() + vec2(16.0, 0.0), egui::Align2::LEFT_CENTER, tab.icon(), FontId::proportional(17.0), if on { pal::cyan() } else { pal::dim() });
            ui.painter().text(rect.left_center() + vec2(44.0, 0.0), egui::Align2::LEFT_CENTER, tab.label(), FontId::proportional(14.0), if on { pal::fg() } else { pal::dim() });
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.tab = tab;
            }
            ui.add_space(2.0);
        }
    }

    /// Visão geral: anéis com o essencial; cada um leva à aba correspondente.
    fn overview_rings(&mut self, ui: &mut Ui, topo: &Topology, samples: &[Sample], bench: Option<&BenchResult>) {
        let last = samples.last().copied().unwrap_or_default();
        let speed = topo.dock_usb4.iter().find(|p| p.pid == 20).and_then(|p| p.val.as_u32());
        let lanes = topo.dock_usb4.iter().find(|p| p.pid == 21).and_then(|p| p.val.as_u32()).unwrap_or(2) as f32;
        let gbps = match speed {
            Some(8) => 10.0 * lanes,
            Some(4) => 20.0 * lanes,
            Some(2) => 40.0 * lanes,
            _ => 0.0,
        };
        let now = now_ms();
        let drops24 = self.incidents.iter().filter(|i| i.is_drop && now - i.start <= 86_400_000).count() as f32;
        let pw = last.power;
        let pct = pw.map(|p| if p.full_mwh > 0 { p.rem_mwh as f32 / p.full_mwh as f32 * 100.0 } else { 0.0 }).unwrap_or(0.0);
        let temp = last.gpu.and_then(|g| g.temp).unwrap_or(0.0) as f32;
        egui::Frame::new()
            .fill(pal::card())
            .stroke(Stroke::new(1.0, pal::border()))
            .corner_radius(CornerRadius::same(pal::radius()))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    // seis medidores espalhados por igual na largura do cartão
                    let gw = 100.0;
                    let free = ui.available_width() - 6.0 * gw;
                    ui.spacing_mut().item_spacing.x = (free / 6.0).clamp(14.0, 160.0);
                    ui.add_space((free / 12.0).clamp(0.0, 80.0));
                    let link_col = if gbps >= 40.0 { pal::green() } else if gbps > 0.0 { pal::amber() } else { pal::red() };
                    if gauge(ui, "⚡", gbps, 40.0, "Gb/s", "Conexão", link_col).on_hover_text("Velocidade do enlace TB (chaves USB4 20 e 21)").clicked() {
                        self.tab = Tab::Enlace;
                    }
                    let up = bench.map(|b| b.h2d_gbs as f32).unwrap_or(0.0);
                    if gauge(ui, "⇅", up, 2.8, "GB/s", "Banda", if up >= 2.4 { pal::green() } else { pal::amber() }).on_hover_text("Banda host→GPU medida no último teste (esperado TB3: 2,5–2,8 GB/s)").clicked() {
                        self.tab = Tab::Fluxo;
                    }
                    let pd_col = if pw.map(|p| p.online).unwrap_or(false) { pal::green() } else { pal::amber() };
                    if gauge(ui, "🔌", pct, 100.0, "%", if pw.map(|p| p.online).unwrap_or(false) { "Carregando" } else { "Bateria" }, pd_col).on_hover_text("Bateria do note; verde = recebendo energia PD do dock").clicked() {
                        self.tab = Tab::Enlace;
                    }
                    if gauge(ui, "💥", drops24, 10.0, "24 h", "Quedas", if drops24 >= 3.0 { pal::red() } else if drops24 > 0.0 { pal::amber() } else { pal::green() }).on_hover_text("Quedas do dock/GPU nas últimas 24 h").clicked() {
                        self.tab = Tab::Historico;
                    }
                    let heat = if temp > 85.0 { pal::red() } else if temp > 70.0 { pal::amber() } else { pal::green() };
                    if gauge(ui, "🌡", temp, 100.0, "°C", "Temperatura", heat).clicked() {
                        self.tab = Tab::Fluxo;
                    }
                    if gauge(ui, "⇄", last.copy, 100.0, "%", "Tráfego", pal::data()).on_hover_text("Motor Copy da GPU: proxy do tráfego no túnel").clicked() {
                        self.tab = Tab::Fluxo;
                    }
                });
            });
    }

    fn page_options(&mut self, ui: &mut Ui, ctx: &egui::Context, th: Th) {
        let mut cfg = crate::config::get();
        let before = cfg;
        card(ui, "🎨  Tema", pal::cyan(), |ui| {
            ui.horizontal_wrapped(|ui| {
                let auto_name = match self.vendor {
                    Some(v) => format!("🔍 Automático · {}", v.name()),
                    None => "🔍 Automático (pela placa)".into(),
                };
                for (val, name, preview) in [(0u8, auto_name, self.vendor.unwrap_or(Th::Dark)), (1, "☀ Claro".into(), Th::Light), (2, "🌙 Escuro".into(), Th::Dark)] {
                    if theme_card(ui, &name, preview, cfg.theme == val).clicked() {
                        cfg.theme = val;
                    }
                }
            });
            ui.label(egui::RichText::new("O automático detecta a GPU do dock (AMD, NVIDIA ou Intel) e aplica a paleta do fabricante. AMD usa abas no topo; NVIDIA e Intel, menu à esquerda.").color(pal::dim()).size(13.5));
        });
        ui.add_space(pal::gap());
        card(ui, "🖥  Tela", pal::cyan(), |ui| {
            let ppp = ctx.native_pixels_per_point().unwrap_or(1.0);
            ui.horizontal(|ui| {
                pill(ui, &format!("Escala do Windows {:.0}% · {:.0} DPI", ppp * 100.0, ppp * 96.0), pal::dim());
                pill(ui, &format!("tema ativo: {}", th.name()), pal::dim());
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("🔎 Tamanho da interface");
                ui.add(egui::Slider::new(&mut cfg.zoom, 0.8..=1.6).step_by(0.05).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
                if ui.small_button("↺ padrão").clicked() {
                    cfg.zoom = 1.0;
                }
            });
        });
        ui.add_space(pal::gap());
        card(ui, "🔔  Comportamento", pal::cyan(), |ui| {
            let mut auto = crate::config::autostart_enabled();
            if ui.checkbox(&mut auto, "🚀 Iniciar com o Windows (direto na bandeja)").changed() {
                if let Err(e) = crate::config::set_autostart(auto) {
                    self.flash = Some(format!("Não consegui alterar a inicialização: {e}"));
                }
            }
            ui.checkbox(&mut cfg.notify, "🔔 Avisar na bandeja quando o dock cair");
            ui.checkbox(&mut cfg.eco, "🍃 Animação econômica (5 qps)");
        });
        ui.add_space(pal::gap());
        card(ui, "🧰  Ferramentas", pal::cyan(), |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("📏 Teste de banda").clicked() {
                    self.start_bench(ctx);
                    self.tab = Tab::Fluxo;
                }
                if ui.button("📄 Exportar histórico (CSV)").clicked() {
                    match crate::config::export_csv() {
                        Ok(p) => {
                            crate::config::open_folder(p.parent().unwrap_or(&p));
                            self.flash = Some(format!("Exportado: {}", p.display()));
                        }
                        Err(e) => self.flash = Some(format!("Falha ao exportar: {e}")),
                    }
                }
                if ui.button("📂 Pasta de dados").clicked() {
                    crate::config::open_folder(&crate::util::data_dir());
                }
            });
            if let Some(f) = &self.flash {
                ui.label(egui::RichText::new(f).color(pal::cyan()).size(13.5));
            }
        });
        if cfg != before {
            crate::config::set(cfg);
        }
    }
}

/// Miniatura clicável de um tema (pré-visualização com as cores dele).
fn theme_card(ui: &mut Ui, name: &str, th: Th, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(170.0, 104.0), Sense::click());
    let saved = pal::current();
    pal::set(th);
    let (bg, card, accent, fg, dim) = (pal::bg(), pal::card(), pal::cyan(), pal::fg(), pal::dim());
    pal::set(saved);
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(pal::radius()), bg);
    if th.nav_left() {
        p.rect_filled(Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(30.0, 62.0)), CornerRadius::same(4), card);
        p.rect_filled(Rect::from_min_size(rect.min + vec2(6.0, 14.0), vec2(3.0, 10.0)), CornerRadius::same(1), accent);
        p.rect_filled(Rect::from_min_size(rect.min + vec2(42.0, 6.0), vec2(122.0, 62.0)), CornerRadius::same(4), card);
    } else {
        p.rect_filled(Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(158.0, 10.0)), CornerRadius::same(3), card);
        p.rect_filled(Rect::from_min_size(rect.min + vec2(10.0, 14.0), vec2(26.0, 2.0)), CornerRadius::ZERO, accent);
        p.rect_filled(Rect::from_min_size(rect.min + vec2(6.0, 22.0), vec2(158.0, 46.0)), CornerRadius::same(4), card);
    }
    p.circle_filled(rect.min + vec2(70.0, 44.0), 9.0, accent.gamma_multiply(0.8));
    p.line_segment([rect.min + vec2(80.0, 44.0), rect.min + vec2(140.0, 44.0)], Stroke::new(2.0, accent));
    p.text(rect.left_bottom() + vec2(10.0, -12.0), egui::Align2::LEFT_CENTER, name, FontId::proportional(14.0), fg);
    let _ = dim;
    let k = ui.ctx().animate_bool_with_time(resp.id, selected || resp.hovered(), 0.15);
    p.rect_stroke(rect, CornerRadius::same(pal::radius()), Stroke::new(1.0 + 1.5 * k, if selected { accent } else { pal::border() }), StrokeKind::Inside);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------------
// Peças visuais extras

/// Logotipos oficiais (SVG do Wikimedia Commons, em assets/logos), como o HWiNFO faz:
/// identificam o fabricante da GPU detectada. Marcas registradas dos respectivos donos.
const LOGO_AMD: &[u8] = include_bytes!("../assets/logos/amd.svg");
const LOGO_NVIDIA: &[u8] = include_bytes!("../assets/logos/nvidia.svg");
const LOGO_INTEL: &[u8] = include_bytes!("../assets/logos/intel.svg");

/// Rasteriza a SVG na altura pedida em pixels físicos (nítida em qualquer DPI).
/// No tema escuro, o preto do logotipo vira branco (as cores da marca ficam).
fn render_logo(v: Th, height_px: u32, dark: bool) -> Option<egui::ColorImage> {
    use resvg::{tiny_skia, usvg};
    let data = match v {
        Th::Amd => LOGO_AMD,
        Th::Nvidia => LOGO_NVIDIA,
        _ => LOGO_INTEL,
    };
    let tree = usvg::Tree::from_data(data, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let k = height_px as f32 / size.height();
    let w = (size.width() * k).ceil() as u32;
    let mut pix = tiny_skia::Pixmap::new(w.max(1), height_px.max(1))?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(k, k), &mut pix.as_mut());
    let mut px = pix.data().to_vec();
    if dark {
        for c in px.chunks_exact_mut(4) {
            let a = c[3] as u16;
            if a > 0 && (c[0] as u16) * 4 < a && (c[1] as u16) * 4 < a && (c[2] as u16) * 4 < a {
                c[0] = c[3];
                c[1] = c[3];
                c[2] = c[3];
            }
        }
    }
    Some(egui::ColorImage::from_rgba_premultiplied([w as usize, height_px as usize], &px))
}

/// Logotipo do fabricante centralizado no topo da visão geral.
fn brand(ui: &mut Ui, v: Th, cache: &mut Option<(Th, bool, u32, egui::TextureHandle)>) {
    let h_pt = 26.0;
    let ppp = ui.ctx().pixels_per_point();
    let h_px = (h_pt * ppp).round() as u32;
    let dark = !pal::current().is_light();
    let fresh = matches!(cache, Some((cv, cd, ch, _)) if *cv == v && *cd == dark && *ch == h_px);
    if !fresh {
        *cache = render_logo(v, h_px, dark).map(|img| (v, dark, h_px, ui.ctx().load_texture("logo-fabricante", img, egui::TextureOptions::LINEAR)));
    }
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h_pt + 22.0), Sense::hover());
    if let Some((_, _, _, tex)) = cache.as_ref() {
        let [w, h] = tex.size();
        let size = vec2(w as f32 / ppp, h as f32 / ppp);
        let r = Rect::from_center_size(rect.center(), size);
        ui.painter().image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    resp.on_hover_text(format!("GPU {} detectada no dock", v.name()));
}

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
                ui.label(egui::RichText::new(text).color(color).size(14.0));
            });
        })
        .response;
    ui.interact(r.rect, ui.id().with(("pill", text)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Bloco número-grande clicável.
fn tile(ui: &mut Ui, icon: &str, value: String, unit: &str, label: &str, color: Color32) -> egui::Response {
    // a sombra sobe no hover (estado do quadro anterior), prometendo o clique
    let id = ui.id().with(("tile", label));
    let hov = ui.ctx().read_response(id).is_some_and(|r| r.hovered());
    let lift = ui.ctx().animate_bool_with_time(id, hov, 0.15);
    let cr = CornerRadius::same(pal::radius().min(8));
    let resp = egui::Frame::new()
        .fill(pal::card2())
        .shadow(pal::elev(2, lift))
        .corner_radius(cr)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(format!("{icon} {label}")).color(pal::dim()).size(12.5));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    ui.label(egui::RichText::new(value).size(21.0).strong().color(color));
                    ui.label(egui::RichText::new(unit).size(13.0).color(pal::dim()));
                });
            });
        })
        .response;
    let r = ui.interact(resp.rect, id, Sense::click());
    if lift > 0.01 {
        ui.painter().rect_stroke(resp.rect, cr, Stroke::new(1.0, color.gamma_multiply(0.7 * lift)), StrokeKind::Inside);
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
    card_shell(ui, title, accent, Some((sel, topic, tip)), |ui, sel| add(ui, sel.unwrap()))
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
        ui.label(egui::RichText::new("Não dá para medir:").color(pal::dim()).size(13.0)).on_hover_text("Coisas do dock que o Windows não enxerga. Passe o mouse em cada ícone.");
        for (icon, name, why) in HIDDEN {
            let r = egui::Frame::new()
                .fill(pal::well())
                .stroke(Stroke::new(1.0, pal::border()))
                .corner_radius(CornerRadius::same(pal::radius()))
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
        p.text(pos2(rect.left(), y + 5.5), egui::Align2::LEFT_CENTER, format!("Canal {}", l + 1), FontId::proportional(12.5), pal::dim());
        if let Some(v) = per_lane {
            let frac = (v / 20.0).clamp(0.0, 1.0);
            let fill = Rect::from_min_max(track.min, pos2(x0 + (x1 - x0) * frac, track.bottom()));
            let col = if frac < 0.99 { pal::amber() } else { pal::green() };
            p.rect_filled(fill, CornerRadius::same(5), col.gamma_multiply(0.85));
            // brilho correndo dentro da faixa
            let sx = fill.left() + ((t as f32 * 60.0 + l as f32 * 37.0) % fill.width().max(1.0));
            p.rect_filled(Rect::from_center_size(pos2(sx, fill.center().y), vec2(14.0, 9.0)), CornerRadius::same(4), Color32::WHITE.gamma_multiply(0.18));
            p.text(pos2(x1 + 8.0, y + 5.5), egui::Align2::LEFT_CENTER, format!("{v:.0}/20 Gb/s"), FontId::proportional(13.0), col);
        }
    }
    let total = per_lane.map(|v| v * lanes as f32);
    if let Some(t) = total {
        p.text(pos2(x1 + 8.0, rect.bottom() - 2.0), egui::Align2::LEFT_BOTTOM, format!("Total {t:.0} de 40 Gb/s"), FontId::proportional(12.5), pal::dim());
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
            p.text(pos2(x + bw / 2.0, base - h1 - h2 - 1.0), egui::Align2::CENTER_BOTTOM, format!("{}", link + pd), FontId::proportional(11.5), pal::dim());
        }
    }
    p.text(rect.left_bottom(), egui::Align2::LEFT_BOTTOM, "14 dias atrás", FontId::proportional(9.5), pal::faint());
    p.text(rect.right_bottom(), egui::Align2::RIGHT_BOTTOM, "hoje", FontId::proportional(9.5), pal::faint());
    resp.on_hover_text("Quedas por dia. Âmbar: o PD caiu junto (o dock inteiro apagou). Vermelho: só o enlace caiu.")
}

/// Medidor circular (arco de 270 graus) com valor animado.
fn gauge(ui: &mut Ui, icon: &str, value: f32, max: f32, unit: &str, label: &str, color: Color32) -> egui::Response {
    // largura cresce com o rótulo, para ele nunca sair cortado; valor NaN = desconhecido ("?")
    let lw = ui.painter().layout_no_wrap(format!("{icon} {label}"), FontId::proportional(12.5), pal::dim()).size().x;
    let (rect, resp) = ui.allocate_exact_size(vec2((lw + 8.0).max(96.0), 92.0), Sense::click());
    let unknown = value.is_nan();
    let v = if unknown { 0.0 } else { ui.ctx().animate_value_with_time(ui.id().with(("g", label)), value, 0.45) };
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
    let lift = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.15);
    p.add(egui::Shape::line(arc(1.0), Stroke::new(7.0, pal::grid())));
    let frac = (v / max).clamp(0.0, 1.0);
    if frac > 0.004 {
        // brilho atrás do arco; acende no hover (o medidor é clicável)
        p.add(egui::Shape::line(arc(frac), Stroke::new(7.0 + 6.0 * lift, glow(color, 0.06 + 0.14 * lift))));
        p.add(egui::Shape::line(arc(frac), Stroke::new(7.0, color)));
        let a = a0 + sweep * frac;
        p.circle_filled(c + vec2(a.cos(), a.sin()) * r, 4.5, color);
    }
    p.text(c + vec2(0.0, -3.0), egui::Align2::CENTER_CENTER, if unknown { "?".to_string() } else { format!("{v:.0}") }, FontId::proportional(19.0), if unknown { pal::dim() } else { pal::fg() });
    p.text(c + vec2(0.0, 14.0), egui::Align2::CENTER_CENTER, unit, FontId::proportional(12.0), pal::dim());
    p.text(pos2(c.x, rect.bottom() - 1.0), egui::Align2::CENTER_BOTTOM, format!("{icon} {label}"), FontId::proportional(12.5), pal::dim());
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Grade do cartão da fonte: N colunas iguais (4 no largo, 2 no estreito) usadas
/// pelos campos, pelo botão e pelos medidores, para as bordas caírem umas sob as outras.
fn psu_cols(avail: f32) -> (usize, f32) {
    let gap = pal::gap();
    let n = if avail >= 600.0 { 4 } else { 2 };
    (n, ((avail - gap * (n as f32 - 1.0)) / n as f32).max(40.0))
}

/// Campo com rótulo (e ícone) em cima e o controle embaixo, numa coluna de largura `w`.
fn field<R>(ui: &mut Ui, w: f32, icon: &str, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.allocate_ui_with_layout(vec2(w, 54.0), egui::Layout::top_down(egui::Align::Min), |ui| {
        ui.set_width(w);
        ui.spacing_mut().item_spacing.y = 0.0;
        let t = if icon.is_empty() && label.is_empty() { " ".to_string() } else { format!("{icon}  {label}") };
        ui.add(egui::Label::new(egui::RichText::new(t).size(12.5).color(pal::dim())).truncate());
        ui.add_space(4.0);
        add(ui)
    })
    .inner
}

/// Faixa de veredito: barra de cor à esquerda, ícone colorido e texto em cor neutra.
fn verdict_strip(ui: &mut Ui, col: Color32, icon: &str, msg: &str) {
    let r = egui::Frame::new()
        .fill(col.gamma_multiply(0.10))
        .corner_radius(CornerRadius::same(pal::radius()))
        .inner_margin(egui::Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(icon).size(20.0).color(col));
                ui.add(egui::Label::new(egui::RichText::new(msg).size(14.5).color(pal::fg())).wrap());
            });
        })
        .response
        .rect;
    let rad = pal::radius();
    let bar = Rect::from_min_max(r.min, pos2(r.left() + 4.0, r.bottom()));
    ui.painter().rect_filled(bar, CornerRadius { nw: rad, sw: rad, ne: 0, se: 0 }, col);
}

/// Hachura diagonal (marca "estimado") dentro de `r`.
fn hatch(p: &egui::Painter, r: Rect) {
    let p = p.with_clip_rect(r.intersect(p.clip_rect()));
    let s = Stroke::new(1.0, pal::bg().gamma_multiply(0.35));
    let h = r.height();
    let mut x = r.left() - h;
    while x < r.right() {
        p.line_segment([pos2(x, r.bottom()), pos2(x + h, r.top())], s);
        x += 6.0;
    }
}

/// Barra empilhada "quanto da fonte uso e com o quê": placa medida (cheia),
/// notebook/dock/portas estimados (hachurados), pior caso e pico marcados.
fn load_bar(ui: &mut Ui, l: &crate::psu::Load, w: f32, width: f32) {
    let id = ui.id().with("psu-bar");
    // cabeçalho
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Carga agora").size(12.5).color(pal::dim()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(format!("{:.0} de {:.0} W", l.now, w)).size(14.0).strong().color(pal::fg()));
        });
    });
    let (area, _) = ui.allocate_exact_size(vec2(width, 16.0 + 24.0 + 22.0), Sense::hover());
    let track = Rect::from_min_size(pos2(area.left(), area.top() + 16.0), vec2(width, 24.0));
    let p = ui.painter();
    let rad = pal::radius();
    p.rect_filled(track, CornerRadius::same(rad), pal::well());
    let sx = |v: f32| track.left() + (v / w).clamp(0.0, 1.0) * track.width();
    let segs = [
        ("🎮 Placa", l.gpu, pal::data(), true),
        ("💻 Notebook pelo PD", l.pd, pal::violet(), false),
        ("▣ Dock", l.dock, pal::dim(), false),
        ("🖴 Portas", l.usb, pal::faint(), false),
    ];
    let mut acc = 0.0;
    let mut end_x = track.left();
    for (k, (name, v, col, measured)) in segs.iter().enumerate() {
        let va = ui.ctx().animate_value_with_time(id.with(k), *v, 0.45);
        let x0 = sx(acc);
        acc += va;
        let x1 = sx(acc);
        if x1 - x0 < 0.5 {
            continue;
        }
        let r = Rect::from_min_max(pos2(x0, track.top()), pos2(x1, track.bottom()));
        let cr = CornerRadius {
            nw: if k == 0 { rad } else { 0 },
            sw: if k == 0 { rad } else { 0 },
            ne: if x1 >= track.right() - 0.5 { rad } else { 0 },
            se: if x1 >= track.right() - 0.5 { rad } else { 0 },
        };
        p.rect_filled(r, cr, *col);
        if !measured {
            hatch(p, r);
        }
        end_x = x1;
        let tip = format!("{name}: {}{v:.0} W ({})", if *measured { "" } else { "~" }, if *measured { "medido" } else { "estimado" });
        ui.interact(r, id.with(("seg", k)), Sense::hover()).on_hover_text(tip);
    }
    let p = ui.painter();
    // sombra interna na borda de cima
    p.line_segment([track.left_top(), track.right_top()], Stroke::new(1.0, Color32::from_black_alpha(60)));
    // pior caso
    let wx = sx(l.worst);
    let wc = if l.worst > w * 0.85 { pal::amber() } else { pal::dim() };
    p.add(egui::Shape::dashed_line(&[pos2(wx, track.top() - 3.0), pos2(wx, track.bottom() + 3.0)], Stroke::new(2.0, wc), 4.0, 3.0));
    // folga no espaço livre (desviando do tracinho do pior caso)
    let free = track.right() - end_x;
    if free >= 90.0 {
        let txt = format!("folga {:.0} W", (w - l.now).max(0.0));
        let (a, b) = if wx > end_x + 1.0 && wx < track.right() - 1.0 {
            if wx - end_x >= track.right() - wx { (end_x, wx) } else { (wx, track.right()) }
        } else {
            (end_x, track.right())
        };
        if b - a >= 90.0 {
            p.text(pos2((a + b) / 2.0, track.center().y), egui::Align2::CENTER_CENTER, txt, FontId::proportional(12.0), pal::dim());
        }
    }
    // pico: triângulo em cima do trilho (preso no fim) e rótulo à esquerda dele
    let over = l.spike > w;
    let kx = sx(l.spike).min(track.right() - 6.0);
    let kc = if over { pal::red() } else { pal::dim() };
    p.text(pos2(kx, track.top() - 1.0), egui::Align2::CENTER_BOTTOM, "▼", FontId::proportional(12.0), kc);
    let klabel = if over { "pico passa da fonte".to_string() } else { format!("pico ~{:.0} W", l.spike) };
    p.text(pos2(kx - 10.0, track.top() - 2.0), egui::Align2::RIGHT_BOTTOM, klabel, FontId::proportional(11.5), kc);
    if over {
        p.line_segment([pos2(track.right() - 1.0, track.top()), pos2(track.right() - 1.0, track.bottom())], Stroke::new(2.0, pal::red()));
    }
    // régua e rótulo do pior caso embaixo
    let ry = track.bottom() + 5.0;
    let f11 = FontId::proportional(11.0);
    let r0 = p.text(pos2(track.left(), ry), egui::Align2::LEFT_TOP, "0", f11.clone(), pal::faint());
    let r1 = p.text(pos2(track.right(), ry), egui::Align2::RIGHT_TOP, format!("{:.0} W", w), f11.clone(), pal::faint());
    let g = p.layout_no_wrap("pior caso".into(), FontId::proportional(11.5), wc);
    let gx = (wx - g.size().x / 2.0).clamp(r0.right() + 8.0, (r1.left() - 8.0 - g.size().x).max(r0.right() + 8.0));
    p.galley(pos2(gx, ry), g, wc);
    // legenda
    // altura de linha normal na legenda (o cartão usa 32 para os campos)
    ui.scope(|ui| {
    ui.spacing_mut().interact_size.y = 18.0;
    ui.spacing_mut().item_spacing = vec2(16.0, 4.0);
    ui.horizontal_wrapped(|ui| {
        for (k, (name, v, col, measured)) in segs.iter().enumerate() {
            if k == 3 && *v <= 0.0 {
                continue;
            }
            // item inteiro como um widget só, para a quebra de linha não separar o quadradinho do texto
            let short = if k == 1 { "💻 Notebook" } else { name };
            let g = ui.painter().layout_no_wrap(format!("{short} {}{v:.0} W", if *measured { "" } else { "~" }), FontId::proportional(13.0), pal::dim());
            let (r, _) = ui.allocate_exact_size(vec2(16.0 + g.size().x, g.size().y.max(18.0)), Sense::hover());
            let sq = Rect::from_center_size(pos2(r.left() + 5.0, r.center().y), vec2(10.0, 10.0));
            ui.painter().rect_filled(sq, CornerRadius::same(rad.min(2)), *col);
            if !measured {
                hatch(ui.painter(), sq);
            }
            ui.painter().galley(pos2(r.left() + 16.0, r.center().y - g.size().y / 2.0), g, pal::dim());
        }
    });
    });
}

/// Estado vazio do cartão da fonte (potência ainda não informada).
fn empty_psu(ui: &mut Ui, width: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(width, 70.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same(pal::radius()), pal::card2());
    let e = r.shrink(0.5);
    let pts = [e.left_top(), e.right_top(), e.right_bottom(), e.left_bottom(), e.left_top()];
    p.add(egui::Shape::dashed_line(&pts, Stroke::new(1.0, pal::grid()), 6.0, 4.0));
    p.text(pos2(r.left() + 34.0, r.center().y), egui::Align2::CENTER_CENTER, "🔌", FontId::proportional(26.0), pal::dim());
    let g = p.layout("Informe o modelo ou a potência para ver a carga ao vivo.".into(), FontId::proportional(14.5), pal::dim(), (r.width() - 80.0).max(60.0));
    p.galley(pos2(r.left() + 62.0, r.center().y - g.size().y / 2.0), g, pal::dim());
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
        .corner_radius(CornerRadius::same(pal::radius()))
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| ui.add(egui::Label::new(egui::RichText::new(text).color(tc).size(13.5)).extend()))
        .response;
    ui.interact(r.rect, ui.id().with(("chip", text)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

// ---------------------------------------------------------------------------
// Cards

impl App {
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
                let txt = if low { "ℹ O cabo está na metade da velocidade" } else { "ℹ O cabo parece estar na metade da velocidade" };
                if ui.add(egui::Label::new(egui::RichText::new(txt).color(pal::amber()).size(14.0)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    *sel = Some(Sel::Info(Topic::Link));
                }
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if tile(ui, "💾", dr.firmware.clone().unwrap_or("?".into()), "", "Firmware", pal::fg()).on_hover_text("Firmware do roteador Titan Ridge do dock").clicked() {
                    *sel = Some(Sel::Dock);
                }
                if let Some(ts) = dr.last_arrival {
                    if tile(ui, "🕒", fmt_dur(now_ms() - ts), "", "Conectado há", pal::green()).on_hover_text(format!("desde {}", fmt_local(ts, false))).clicked() {
                        *sel = Some(Sel::Info(Topic::Stability));
                    }
                }
                let (st, stc) = match dr.dstate {
                    Some(1) => ("Ativo", pal::green()),
                    Some(4) => ("☾ Em repouso", pal::dim()),
                    _ => ("?", pal::dim()),
                };
                if tile(ui, "🔀", st.to_string(), "", "Estado", stc)
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
                if tile(ui, "🔋", wtxt, "W", "Carga da bateria", if w >= 0.0 { pal::bolt() } else { pal::amber() }).on_hover_text("Taxa de carga (+) ou descarga (−) da bateria — WMI root\\wmi BatteryStatus").clicked() {
                    *sel = Some(Sel::Info(Topic::Power));
                }
                if tile(ui, "〰", format!("{:.2}", pw.mv as f64 / 1000.0), "V", "Tensão da bateria", pal::fg()).on_hover_text("Tensão da bateria (não é a tensão do PD)").clicked() {
                    *sel = Some(Sel::Info(Topic::Power));
                }
                if tile(ui, "⚡", "85".into(), "W", "Carregador oferece", pal::dim()).on_hover_text("Valor do DROM do dock (\"Module 2 PD 85W\"). O contrato negociado não é exposto.").clicked() {
                    *sel = Some(Sel::Info(Topic::Power));
                }
            });
            let series: Vec<f32> = samples.iter().filter_map(|s| s.power).map(|p| (p.charge_mw - p.discharge_mw) as f32 / 1000.0).collect();
            chart(ui, "Carga da bateria (W)", &[(pal::bolt(), "agora", series)], 10.0, " W", 64.0);
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
                if gauge(ui, "⇄", last.copy, 100.0, "%", "Tráfego", pal::data()).on_hover_text("Motor Copy: cópias DMA host↔GPU — o que mais passa pelo túnel").clicked() {
                    *sel = Some(Sel::Info(Topic::Flow));
                }
                if gauge(ui, "△", last.d3, 100.0, "%", "Gráficos 3D", pal::violet()).clicked() {
                    *sel = Some(Sel::Info(Topic::Flow));
                }
                ui.vertical(|ui| {
                    ui.add_space(14.0);
                    meter_bar(ui, last.ded_mb as f32 / 16368.0, &format!("Memória da placa (VRAM) · {:.0} MB", last.ded_mb), pal::data())
                        .on_hover_text("Memória dedicada da GPU em uso (PDH GPU Adapter Memory)");
                    ui.add_space(6.0);
                    meter_bar(ui, last.shared_mb as f32 / 8192.0, &format!("RAM do notebook usada pela placa · {:.0} MB", last.shared_mb), pal::violet())
                        .on_hover_text("RAM do note mapeada pela GPU — cada acesso atravessa o túnel TB");
                });
            });
            let copy: Vec<f32> = samples.iter().map(|s| s.copy).collect();
            let d3: Vec<f32> = samples.iter().map(|s| s.d3).collect();
            chart(ui, "Uso da placa (5 min)", &[(pal::data(), "Cópia de dados", copy), (pal::violet(), "Gráficos 3D", d3)], 100.0, "%", 84.0);
            ui.horizontal(|ui| {
                let running = matches!(bench, BenchState::Running(_));
                if ui.add_enabled(!running, egui::Button::new("📏 Teste de banda")).on_hover_text("Cópias D3D12 de 1 GiB na fila COPY, host↔VRAM (~5 s).").clicked() {
                    run = true;
                }
                match bench {
                    BenchState::Running(s) => {
                        ui.spinner();
                        ui.label(egui::RichText::new(s).color(pal::dim()).size(13.5));
                    }
                    BenchState::Failed(e) => {
                        ui.label(egui::RichText::new(e).color(pal::red()).size(13.5));
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
                meter_bar(ui, frac, &format!("VRAM {} de {} MB", g.vram.unwrap_or(0), s.vram_total_mb), pal::data());
            }
        });
        self.sel = sel;
    }

    fn card_timeline(&mut self, ui: &mut Ui, compact: bool) {
        let now = now_ms();
        let mut sel = self.sel;
        let mut go_all = false;
        let title = if compact { "🕓  O que aconteceu" } else { "🕓  Linha do tempo" };
        card_i(ui, title, pal::red(), &mut sel, Topic::Stability, "Fontes: Event Log, PnP ao vivo, Overwatch", |ui, _sel| {
            let enabled: Vec<Cat> = self.cats.iter().filter(|c| c.1).map(|c| c.0).collect();
            let t0 = if compact {
                now - 30 * 86_400_000
            } else if self.range == Range::All {
                self.events.first().map(|e| e.ts).unwrap_or(now - 86_400_000)
            } else {
                now - self.range.ms()
            };
            if !compact {
                ui.horizontal_wrapped(|ui| {
                    for r in [Range::H24, Range::D7, Range::D30, Range::All] {
                        if chip(ui, self.range == r, r.label(), pal::cyan()) {
                            self.range = r;
                        }
                    }
                    ui.add_space(8.0);
                    for (c, on) in self.cats.iter_mut() {
                        let name = match c {
                            Cat::Dock => "🔗",
                            Cat::Energia => "⚡",
                            Cat::Pcie => "⚠",
                            Cat::Tdr => "❄",
                            Cat::Gpu => "🎮",
                            Cat::UsbA => "🖴",
                            Cat::Sistema => "🌙",
                            Cat::App => "ℹ",
                        };
                        if chip(ui, *on, name, cat_color(*c)) {
                            *on = !*on;
                        }
                    }
                    ui.add_space(8.0);
                    if chip(ui, self.only_problems, "❗", pal::red()) {
                        self.only_problems = !self.only_problems;
                    }
                });
                ui.add_space(4.0);
                // faixa temporal
                let w = ui.available_width();
                let (rect, resp) = ui.allocate_exact_size(vec2(w, 46.0), Sense::hover());
                let p = ui.painter_at(rect);
                p.rect_filled(rect, CornerRadius::same(6), pal::well());
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
                p.text(rect.left_bottom() + vec2(8.0, -2.0), egui::Align2::LEFT_BOTTOM, fmt_local(t0, false), FontId::proportional(11.5), pal::faint());
                p.text(rect.right_bottom() + vec2(-8.0, -2.0), egui::Align2::RIGHT_BOTTOM, "agora", FontId::proportional(11.5), pal::faint());
                if let Some(pos) = resp.hover_pos() {
                    let ts = t0 + (((pos.x - rect.left() - 8.0) / (rect.width() - 16.0)) as f64 * (now - t0) as f64) as i64;
                    p.line_segment([pos2(pos.x, rect.top()), pos2(pos.x, rect.bottom())], Stroke::new(1.0, pal::dim().gamma_multiply(0.5)));
                    p.text(pos2(pos.x + 4.0, rect.top() + 2.0), egui::Align2::LEFT_TOP, fmt_local(ts, false), FontId::proportional(11.5), pal::dim());
                }
                ui.add_space(6.0);
            }

            let limit = if compact { 6 } else { self.hist_limit };
            let all: Vec<&Incident> = self
                .incidents
                .iter()
                .rev()
                .filter(|i| i.start >= t0)
                .filter(|i| compact || i.events.iter().any(|e| enabled.contains(&e.kind.cat())))
                .filter(|i| if compact { i.sev >= 1 } else { !self.only_problems || i.sev >= 1 })
                .collect();
            let total = all.len();
            if total == 0 {
                ui.label(egui::RichText::new(if compact { "✔ nenhum problema em 30 dias" } else { "✔ nada neste período" }).color(pal::green()));
            }
            let mut open = self.open_inc;
            for inc in all.into_iter().take(limit) {
                incident_row(ui, inc, &mut open);
            }
            self.open_inc = open;
            ui.horizontal(|ui| {
                if compact {
                    if total > limit && ui.add(egui::Button::new(egui::RichText::new(format!("Ver histórico completo ({total}) →")).color(pal::cyan())).frame(false)).clicked() {
                        go_all = true;
                    } else if total <= limit && ui.add(egui::Button::new(egui::RichText::new("Ver histórico completo →").color(pal::cyan())).frame(false)).clicked() {
                        go_all = true;
                    }
                } else if total > limit {
                    if ui.button(format!("mostrar mais ({} restantes)", total - limit)).clicked() {
                        self.hist_limit += 30;
                    }
                }
            });
        });
        self.sel = sel;
        if go_all {
            self.tab = Tab::Historico;
        }
    }

    // ---------------- painel lateral

    fn details(&mut self, ui: &mut Ui, topo: &Topology, gst: Option<&crate::collect::GpuStatic>, samples: &[Sample], bench: Option<&BenchResult>, ctx: &egui::Context) {
        let t = ctx.input(|i| i.time);
        let k = ((t - self.sel_at) / 0.22).clamp(0.0, 1.0) as f32;
        let e = 1.0 - (1.0 - k).powi(3);
        let sel_at = self.sel_at;
        let before = self.sel;
        let mut sel = self.sel;
        let last = samples.last().copied().unwrap_or_default();
        let dock_ok = topo.dock_router.is_some();
        let neutral = pal::pcie();
        let d0 = |n: &Node| n.dstate == Some(1);
        match before {
            Some(Sel::Info(tp)) => {
                let (icon, name) = tp.title().split_once(' ').unwrap_or(("ⓘ", tp.title()));
                side_hero(
                    ui,
                    e,
                    pal::cyan(),
                    &|p: &egui::Painter, r: Rect| {
                        let rr = r.height().min(r.width()) * 0.42;
                        p.circle_filled(r.center(), rr, pal::cyan().gamma_multiply(0.16));
                        p.circle_stroke(r.center(), rr, Stroke::new(1.0, pal::cyan().gamma_multiply(0.5)));
                        p.text(r.center(), egui::Align2::CENTER_CENTER, icon, FontId::proportional(40.0 * (r.height() / 88.0).min(1.0)), pal::fg());
                    },
                    name,
                    "Como funciona",
                    None,
                    &mut sel,
                );
                ui.add_space(10.0);
                self.info(ui, tp, topo, samples, bench, ctx);
            }
            Some(Sel::Gpu) => {
                let g = last.gpu.unwrap_or_default();
                let node = topo.gpu.as_ref();
                let (st, sc) = match node {
                    None => ("Não encontrada", pal::amber()),
                    Some(n) if n.problem != 0 => ("Com problema", pal::red()),
                    Some(n) if n.dstate.is_some_and(|d| d != 1) => ("Dormindo", pal::faint()),
                    _ => ("Funcionando", pal::green()),
                };
                let spin = (g.usage.unwrap_or(0.0) / 100.0) as f32;
                let title = gst.map(|s| s.name.trim()).filter(|s| !s.is_empty()).unwrap_or("Placa de vídeo");
                side_hero(ui, e, pal::cyan(), &|p: &egui::Painter, r: Rect| art::gpu(p, r, spin, node.is_some(), t), title, "Dentro do dock, ligada pelo Thunderbolt", Some((st, sc)), &mut sel);
                side_hint(
                    ui,
                    match st {
                        "Funcionando" => "Tudo normal com a placa. Clique num bloco para ver os gráficos.",
                        "Dormindo" => "A placa está em economia de energia. Ela acorda sozinha quando um programa usa.",
                        "Com problema" => "O Windows relatou um problema com a placa. Veja os detalhes técnicos no fim.",
                        _ => "A placa não apareceu no Windows. Confira se o dock está ligado e o cabo encaixado.",
                    },
                );
                let ser = |f: fn(&crate::collect::GpuM) -> Option<f32>| samples.iter().filter_map(|s| s.gpu.as_ref().and_then(f)).collect::<Vec<f32>>();
                let temp = g.temp.map(|v| v as f32);
                let hot = g.hot.map(|v| v as f32);
                let fan = g.fan;
                let vram_total = gst.map(|s| s.vram_total_mb).unwrap_or(0);
                let vram = g.vram.map(|v| {
                    if vram_total > 0 {
                        format!("{} / {}", dec(v as f64 / 1024.0, 1), (vram_total as f64 / 1024.0).round())
                    } else {
                        dec(v as f64 / 1024.0, 1)
                    }
                });
                let mut tiles = vec![
                    tl("🌡", temp.map(|v| format!("{v:.0}")), "°C", "Temperatura", heat(temp.unwrap_or(0.0))).vc(heat(temp.unwrap_or(0.0))).sp(ser(|g| g.temp.map(|v| v as f32))),
                    tl("🔥", hot.map(|v| format!("{v:.0}")), "°C", "Ponto mais quente", heat(hot.unwrap_or(0.0) - 10.0)).vc(heat(hot.unwrap_or(0.0) - 10.0)).sp(ser(|g| g.hot.map(|v| v as f32))),
                    tl("⚡", g.tbp.map(|v| format!("{v:.0}")), "W", "Consumo da placa", pal::bolt()).sp(ser(|g| g.tbp.map(|v| v as f32))),
                    tl("⏱", g.clk.map(|v| v.to_string()), "MHz", "Velocidade", pal::data()).sp(ser(|g| g.clk.map(|v| v as f32))),
                    tl("🌀", fan.map(|v| v.to_string()), "RPM", if fan == Some(0) { "Ventoinha parada" } else { "Ventoinha" }, pal::violet()).sp(ser(|g| g.fan.map(|v| v as f32))),
                    {
                        let x = tl("💾", vram, "GB", "Memória de vídeo", pal::data());
                        match (g.vram, vram_total) {
                            (Some(v), t) if t > 0 => x.bar(v as f32 / t as f32),
                            _ => x,
                        }
                    },
                    tl("📶", g.usage.map(|v| format!("{v:.0}")), "%", "Uso da GPU", pal::data()).sp(ser(|g| g.usage.map(|v| v as f32))),
                ];
                let link = node.and_then(|n| n.pcie);
                let mut lt = tl("🔗", link.map(|(cs, cw, _, _)| pcie_short(cs, cw)), "", "Enlace PCIe", neutral);
                if let Some((cs, cw, ms, mw)) = link {
                    if cs < ms || cw < mw {
                        lt = lt.note(format!("máx {}", pcie_short(ms, mw)));
                    }
                }
                tiles.push(lt);
                let tiles: Vec<Tile> = tiles.into_iter().map(|x| x.tip("Ver no Desempenho")).collect();
                if tile_grid(ui, &tiles, sel_at).is_some() {
                    self.tab = Tab::Fluxo;
                }
                ui.add_space(12.0);
                if ui.button(egui::RichText::new("📈 Ver desempenho ›").size(14.0)).clicked() {
                    self.tab = Tab::Fluxo;
                }
                side_tech(ui, "gpu", |ui| {
                    if let Some(s) = gst {
                        kv(ui, "ADLX:", format!("{} · {} MB · {} x{}", s.name, s.vram_total_mb, s.bus, s.lanes));
                        kv(ui, "LUID (PDH):", s.luid.clone().unwrap_or_default());
                    }
                    node_details(ui, "GPU", topo.gpu.as_ref());
                    for n in &topo.gpu_chain {
                        node_details(ui, "Switch interno da placa", Some(n));
                    }
                });
            }
            Some(Sel::Dock) => {
                let ok = dock_ok && topo.dock_switch.is_some() && topo.dock_router.as_ref().is_some_and(|n| n.problem == 0);
                let (st, sc) = if ok { ("Tudo certo", pal::green()) } else { ("Com problema", pal::red()) };
                side_hero(ui, e, pal::cyan(), &|p: &egui::Painter, r: Rect| art::dock(p, r, dock_ok, t), "Dock WKG-L19", "Caixa externa da GPU", Some((st, sc)), &mut sel);
                side_hint(ui, if ok { "O dock está ligado e conversando com o notebook." } else { "Algo no dock não respondeu. Veja os detalhes técnicos no fim." });
                let dr = topo.dock_router.as_ref();
                let active = topo.ports.iter().filter(|p| d0(&p.node)).count();
                let mut tiles = vec![
                    tl("🔧", dr.and_then(|n| n.firmware.clone()), "", "Firmware", neutral),
                    tl("🕑", dr.and_then(|n| n.last_arrival).map(|ts| fmt_dur(now_ms() - ts)), "", "Conectado há", neutral),
                    tl(if ok { "✔" } else { "⚠" }, Some(st.to_string()), "", "Estado", sc).vc(sc),
                    tl("🔌", (!topo.ports.is_empty()).then(|| format!("{active} de {}", topo.ports.len())), "", "Portas ativas", neutral).tip("Portas acordadas (D0). As outras estão dormindo para economizar."),
                ];
                if let Some(b) = bench {
                    tiles.push(tl("📏", Some(dec(b.h2d_gbs.max(b.d2h_gbs), 1)), "GB/s", "Banda medida", pal::data()));
                }
                tile_grid(ui, &tiles, sel_at);
                side_caption(ui, "Portas");
                ui.horizontal_wrapped(|ui| {
                    for (i, p) in topo.ports.iter().enumerate() {
                        let name = match p.role {
                            PortRole::Gpu => "🎮 GPU",
                            PortRole::UsbA => "🖴 USB",
                            PortRole::TbExtra => "⚡ Thunderbolt",
                            PortRole::Other => "🔌 Porta",
                        };
                        let dot = if d0(&p.node) { pal::green() } else { pal::faint() };
                        if side_chip(ui, name, Some(dot), true).on_hover_text(if d0(&p.node) { "Ativa" } else { "Dormindo" }).clicked() {
                            sel = Some(Sel::Port(i));
                        }
                    }
                });
                ui.add_space(6.0);
                if side_chip(ui, "⚡ Ver o cabo", None, true).clicked() {
                    sel = Some(Sel::Cable);
                }
                side_tech(ui, "dock", |ui| {
                    node_details(ui, "Roteador Thunderbolt do dock", topo.dock_router.as_ref());
                    usb4_table(ui, "Chaves USB4 do dock {5DF7E321-…}", &topo.dock_usb4);
                    node_details(ui, "Switch PCIe do dock (upstream)", topo.dock_switch.as_ref());
                    for (i, p) in topo.ports.iter().enumerate() {
                        node_details(ui, &format!("Porta downstream {} — {:?}", i + 1, p.role), Some(&p.node));
                    }
                });
            }
            Some(Sel::Cable) => {
                let speed = topo.dock_usb4.iter().find(|p| p.pid == 20).and_then(|p| p.val.as_u32());
                let lanes = topo.dock_usb4.iter().find(|p| p.pid == 21).and_then(|p| p.val.as_u32());
                let ln = lanes.unwrap_or(2).max(1);
                let gbps = match speed {
                    Some(0x8) => Some(ln * 10),
                    Some(0x4) => Some(ln * 20),
                    _ => None,
                };
                let (st, sc) = match (dock_ok, speed) {
                    (false, _) => ("Sem conexão", pal::red()),
                    (true, Some(0x8)) => ("Abaixo do máximo", pal::amber()),
                    (true, Some(0x4)) => ("Na velocidade máxima", pal::green()),
                    _ => ("Conectado", pal::green()),
                };
                side_hero(ui, e, pal::bolt(), &|p: &egui::Painter, r: Rect| art::usb_c(p, r, dock_ok), "Cabo Thunderbolt", "Liga o notebook ao dock", Some((st, sc)), &mut sel);
                side_hint(
                    ui,
                    match (dock_ok, speed) {
                        (false, _) => "O notebook não está enxergando o dock. Confira os dois lados do cabo.",
                        (true, Some(0x8)) => "O dock aceita 40 Gb/s, mas o cabo está entregando a metade. Um cabo Thunderbolt certificado de 40 Gb/s resolve.",
                        _ => "O cabo leva os dados da GPU e traz energia para o notebook.",
                    },
                );
                let pd = last.power.map(|p| p.online);
                let tiles = vec![
                    tl("⚡", gbps.map(|v| v.to_string()), "Gb/s", "Velocidade do cabo", pal::bolt()).vc_if(speed == Some(0x8), pal::amber()).note("≈ decodificado").tip("Lido das chaves USB4 20 e 21 do dock. Veja os detalhes técnicos."),
                    tl("⛓", lanes.map(|v| v.to_string()), "", "Canais ativos", neutral),
                    tl("📏", bench.map(|b| format!("↑{} ↓{}", dec(b.h2d_gbs, 1), dec(b.d2h_gbs, 1))), "GB/s", "Banda medida", pal::data()),
                    tl("🔋", pd.map(|o| if o { "Sim" } else { "Não" }.to_string()), "", "Carregando o notebook", pal::bolt()).vc_if(pd == Some(false), pal::amber()),
                ];
                tile_grid(ui, &tiles, sel_at);
                ui.add_space(12.0);
                let running = matches!(*self.bench.lock().unwrap(), BenchState::Running(_));
                if ui.add_enabled(!running, egui::Button::new(egui::RichText::new(if running { "📏 Medindo…" } else { "📏 Medir banda agora" }).size(14.0))).clicked() {
                    self.start_bench(ctx);
                }
                side_tech(ui, "cable", |ui| {
                    usb4_table(ui, "Chaves USB4 do dock {5DF7E321-…}", &topo.dock_usb4);
                    node_details(ui, "Roteador Thunderbolt do dock", topo.dock_router.as_ref());
                    usb4_table(ui, "Chaves USB4 do host", &topo.host_usb4);
                });
            }
            Some(Sel::Host) => {
                let ctrl = topo.host_ctrl.as_ref();
                let ok = ctrl.is_some_and(|n| n.problem == 0);
                let (st, sc) = if ok { ("Funcionando", pal::green()) } else { ("Com problema", pal::red()) };
                side_hero(ui, e, pal::cyan(), &|p: &egui::Painter, r: Rect| art::laptop(p, r, true, t), "Notebook", "Porta Thunderbolt 4", Some((st, sc)), &mut sel);
                side_hint(ui, if ok { "A porta Thunderbolt do notebook está pronta e levando a GPU." } else { "O controlador da porta Thunderbolt não respondeu. Veja os detalhes técnicos." });
                let pw = last.power;
                let pct = pw.filter(|p| p.full_mwh > 0).map(|p| (p.rem_mwh as f64 * 100.0 / p.full_mwh as f64).round());
                let tiles = vec![
                    tl("🔧", topo.host_router.as_ref().and_then(|n| n.firmware.clone()), "", "Firmware", neutral),
                    tl(if ok { "✔" } else { "⚠" }, Some(if ctrl.is_some() { "Funcionando" } else { "Ausente" }.to_string()), "", "Controlador USB4", sc).vc(sc),
                    tl("🔗", topo.root_port.as_ref().and_then(|n| n.pcie).map(|(cs, cw, _, _)| pcie_short(cs, cw)), "", "Túnel PCIe", neutral),
                    tl("🔋", pw.map(|p| if p.online { "Na tomada" } else { "Na bateria" }.to_string()), &pct.map(|v| format!("{v:.0}%")).unwrap_or_default(), "Energia", pal::bolt())
                        .vc_if(pw.is_some_and(|p| !p.online), pal::amber())
                        .sp(samples.iter().filter_map(|s| s.power.filter(|p| p.full_mwh > 0).map(|p| p.rem_mwh as f32 * 100.0 / p.full_mwh as f32)).collect()),
                ];
                tile_grid(ui, &tiles, sel_at);
                ui.add_space(8.0);
                if side_chip(ui, "⚡ Ver o cabo", None, true).clicked() {
                    sel = Some(Sel::Cable);
                }
                side_tech(ui, "host", |ui| {
                    node_details(ui, "Controlador USB4 do note", topo.host_ctrl.as_ref());
                    node_details(ui, "Roteador raiz USB4", topo.host_router.as_ref());
                    usb4_table(ui, "Chaves USB4 do host", &topo.host_usb4);
                    node_details(ui, "Porta raiz PCIe (túnel)", topo.root_port.as_ref());
                });
            }
            Some(Sel::Port(i)) => {
                let Some(p) = topo.ports.get(i) else {
                    // topologia ainda não lida (início): espera; lida e sem a porta: fecha
                    if topo.read_ms != 0 {
                        self.sel = None;
                    } else {
                        ui.spinner();
                    }
                    return;
                };
                let on = d0(&p.node);
                let (title, sub) = match p.role {
                    PortRole::Gpu => ("Porta da GPU", "Onde a placa de vídeo encaixa no dock"),
                    PortRole::UsbA => ("Porta USB", "As entradas USB do dock"),
                    PortRole::TbExtra => ("Porta Thunderbolt extra", "Saída para outro aparelho Thunderbolt"),
                    PortRole::Other => ("Porta do dock", "Saída interna do dock"),
                };
                let (st, sc) = if p.node.problem != 0 { ("Com problema", pal::red()) } else if on { ("Ativa", pal::green()) } else { ("Dormindo", pal::faint()) };
                let role = p.role.clone();
                side_hero(
                    ui,
                    e,
                    pal::cyan(),
                    &|pt: &egui::Painter, r: Rect| match role {
                        PortRole::Gpu => art::gpu(pt, r, 0.0, on, t),
                        PortRole::UsbA => art::usb_a(pt, r, on),
                        _ => art::usb_c(pt, r, on),
                    },
                    title,
                    sub,
                    Some((st, sc)),
                    &mut sel,
                );
                side_hint(ui, if on { "Ligada e em uso." } else { "Dormindo para economizar energia. Ela acorda quando algo for ligado nela." });
                let mut tiles = vec![
                    tl(if on { "✔" } else { "💤" }, Some(if on { "Ativa" } else { "Dormindo" }.to_string()), "", "Estado", sc).vc(sc),
                    {
                        let link = p.node.pcie;
                        let mut x = tl("🔗", link.map(|(cs, cw, _, _)| pcie_short(cs, cw)), "", "Enlace PCIe", neutral);
                        if let Some((cs, cw, ms, mw)) = link {
                            if cs < ms || cw < mw {
                                x = x.note(format!("máx {}", pcie_short(ms, mw)));
                            }
                        }
                        x
                    },
                ];
                if p.role == PortRole::UsbA {
                    tiles.push(tl("🖴", Some(topo.usb_a.len().to_string()), "", "Dispositivos USB", neutral));
                }
                tile_grid(ui, &tiles, sel_at);
                if p.role == PortRole::UsbA && !topo.usb_a.is_empty() {
                    side_caption(ui, "Ligados agora");
                    ui.horizontal_wrapped(|ui| {
                        for d in &topo.usb_a {
                            side_chip(ui, &format!("🖴 {}", d.name), None, false);
                        }
                    });
                }
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if p.role == PortRole::Gpu && side_chip(ui, "🎮 Ver a placa", None, true).clicked() {
                        sel = Some(Sel::Gpu);
                    }
                    if side_chip(ui, "🔗 Ver o dock", None, true).clicked() {
                        sel = Some(Sel::Dock);
                    }
                });
                side_tech(ui, &format!("port{i}"), |ui| {
                    node_details(ui, "Porta downstream do switch", Some(&p.node));
                    node_details(ui, "Dispositivo na porta", p.child.as_ref());
                    if p.role == PortRole::UsbA {
                        for d in &topo.usb_a {
                            node_details(ui, "Dispositivo USB", Some(d));
                        }
                    }
                });
            }
            Some(Sel::Psu) => {
                if self.psu.watts > 0 {
                    let l = crate::psu::compute(&self.psu, &psu_inputs(topo, samples));
                    let f = l.frac.clamp(0.0, 1.0);
                    let (st, sc) = if f < 0.6 { ("≈ Com folga", pal::green()) } else if f < 0.8 { ("≈ Perto do limite", pal::amber()) } else { ("≈ No limite", pal::red()) };
                    let model = self.psu.model.trim().to_string();
                    let rating = self.psu.rating.clone();
                    side_hero(
                        ui,
                        e,
                        pal::bolt(),
                        &|p: &egui::Painter, r: Rect| art::psu(p, r, dock_ok, t, &rating, f),
                        if model.is_empty() { "Fonte do dock" } else { &model },
                        &format!("Fonte do dock · {} W", self.psu.watts),
                        Some((st, sc)),
                        &mut sel,
                    );
                    ui.add_space(10.0);
                }
                self.psu_details(ui, topo, samples);
            }
            None => {}
        }
        if sel != before {
            self.sel = sel;
        }
    }

    /// Painel lateral da fonte do dock: medidor, de onde vem a carga, pior caso e pico. Tudo estimado.
    fn psu_details(&mut self, ui: &mut Ui, topo: &Topology, samples: &[Sample]) {
        if self.psu.watts == 0 {
            self.sel = None;
            return;
        }
        let l = crate::psu::compute(&self.psu, &psu_inputs(topo, samples));
        let w = self.psu.watts as f32;
        if let Some(c) = art::seal_color(&self.psu.rating) {
            ui.horizontal(|ui| {
                egui::Frame::new().fill(c).corner_radius(CornerRadius::same(3)).inner_margin(egui::Margin::symmetric(8, 2)).show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(format!("80+ {}", self.psu.rating.trim())).size(13.0).strong().color(Color32::from_rgb(20, 20, 22))).extend());
                });
                ui.label(egui::RichText::new(format!("{} W", self.psu.watts)).size(13.5).color(pal::dim()));
            });
            ui.add_space(6.0);
        }
        let f = l.frac.clamp(0.0, 1.0);
        let lc = if f < 0.6 { pal::green() } else if f < 0.8 { pal::amber() } else { pal::red() };
        ui.vertical_centered(|ui| {
            gauge(ui, "⚡", l.now, w, "W", "≈ agora", lc).on_hover_text(format!("{:.0}% de {} W", f * 100.0, self.psu.watts));
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            for (icon, v, tip) in [("🎮", l.gpu, "Placa de vídeo (medida)"), ("💻", l.pd, "Notebook pelo PD (estimado)"), ("🖴", l.dock + l.usb, "Dock e portas (estimado)")] {
                egui::Frame::new()
                    .fill(pal::card2())
                    .stroke(Stroke::new(1.0, pal::border()))
                    .corner_radius(CornerRadius::same(pal::radius()))
                    .inner_margin(egui::Margin::symmetric(10, 5))
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(egui::RichText::new(format!("{icon} {v:.0} W")).size(14.0).color(pal::fg())).extend());
                    })
                    .response
                    .on_hover_text(tip);
            }
        });
        ui.add_space(8.0);
        ui.label(egui::RichText::new(format!("◆ pior caso ≈ {:.0} W", l.worst)).size(14.0).color(if l.worst >= w { pal::red() } else { pal::fg() }))
            .on_hover_text("Placa no limite de potência + notebook carregando + todas as portas em uso.");
        ui.label(egui::RichText::new(format!("⚡ pico ≈ {:.0} W", l.spike)).size(14.0).color(if l.spike > w { pal::amber() } else { pal::fg() }))
            .on_hover_text("Picos de milissegundos da placa somados ao resto.");
        ui.add_space(10.0);
        if ui.button(egui::RichText::new("✏ Trocar fonte ›").size(14.0)).clicked() {
            self.tab = Tab::Enlace;
        }
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Valores estimados").size(12.0).color(pal::faint()));
    }

    fn info(&mut self, ui: &mut Ui, t: Topic, topo: &Topology, samples: &[Sample], bench: Option<&BenchResult>, ctx: &egui::Context) {
        // cada parágrafo num callout próprio (bloco recuado), mais fácil de ler em pedaços
        let para = |ui: &mut Ui, s: &str| {
            egui::Frame::new().fill(pal::well()).corner_radius(CornerRadius::same(pal::radius().min(6))).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new(s).size(14.0).color(pal::fg()));
            });
            ui.add_space(6.0);
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
                side_tech(ui, "info-link", |ui| usb4_table(ui, "Chaves USB4 do dock", &topo.dock_usb4));
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

// ---------------- peças do painel lateral (hero, blocos, faixas)

/// Cor de calor: verde, âmbar acima de 70 °C, vermelho acima de 85 °C.
fn heat(t: f32) -> Color32 {
    if t > 85.0 { pal::red() } else if t > 70.0 { pal::amber() } else { pal::green() }
}

/// "Gen4 x8" a partir do código de velocidade e da largura.
fn pcie_short(code: u32, w: u32) -> String {
    format!("{} x{w}", pnp::pcie_speed_name(code).split(' ').next().unwrap_or("?"))
}

fn dec(v: f64, n: usize) -> String {
    format!("{v:.n$}").replace('.', ",")
}

/// Um bloco da grade do painel lateral: ícone, valor grande, rótulo pequeno e, embaixo, uma sparkline ou barra.
struct Tile {
    icon: &'static str,
    value: Option<String>,
    unit: String,
    label: String,
    color: Color32,
    vcol: Option<Color32>,
    spark: Vec<f32>,
    bar: Option<f32>,
    note: Option<String>,
    tip: Option<String>,
}

fn tl(icon: &'static str, value: Option<String>, unit: &str, label: &str, color: Color32) -> Tile {
    Tile { icon, value, unit: unit.into(), label: label.into(), color, vcol: None, spark: vec![], bar: None, note: None, tip: None }
}

impl Tile {
    /// Valor pintado com cor de significado (temperatura, estado).
    fn vc(mut self, c: Color32) -> Self {
        self.vcol = Some(c);
        self
    }
    fn vc_if(self, cond: bool, c: Color32) -> Self {
        if cond { self.vc(c) } else { self }
    }
    fn sp(mut self, s: Vec<f32>) -> Self {
        self.spark = s;
        self
    }
    fn bar(mut self, f: f32) -> Self {
        self.bar = Some(f);
        self
    }
    fn note(mut self, s: impl Into<String>) -> Self {
        self.note = Some(s.into());
        self
    }
    fn tip(mut self, s: impl Into<String>) -> Self {
        self.tip = Some(s.into());
        self
    }
    fn strip(&self) -> bool {
        self.bar.is_some() || self.spark.len() >= 2
    }
}

const TILE_PAD: f32 = 10.0;

fn tile_height(ui: &Ui, x: &Tile, w: f32) -> f32 {
    let ww = w - 2.0 * TILE_PAD;
    let lh = ui.painter().layout(x.label.clone(), FontId::proportional(12.5), pal::dim(), ww).size().y;
    let nh = x.note.as_ref().map(|n| ui.painter().layout(n.clone(), FontId::proportional(12.0), pal::faint(), ww).size().y + 2.0).unwrap_or(0.0);
    let sh = if x.strip() { 22.0 } else { 0.0 };
    (TILE_PAD + 30.0 + 6.0 + lh + nh + sh + TILE_PAD).max(88.0)
}

/// Sparkline das últimas 90 amostras, escala min/max com 10% de folga e ponto no valor atual.
fn spark(p: &egui::Painter, r: Rect, s: &[f32], c: Color32) {
    let s = &s[s.len().saturating_sub(90)..];
    if s.len() < 2 {
        return;
    }
    let (mut lo, mut hi) = s.iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let pad = ((hi - lo) * 0.1).max(0.5);
    lo -= pad;
    hi += pad;
    let pts: Vec<Pos2> = s
        .iter()
        .enumerate()
        .map(|(i, &v)| pos2(r.left() + r.width() * i as f32 / (s.len() - 1) as f32, r.bottom() - r.height() * (v - lo) / (hi - lo)))
        .collect();
    let last = *pts.last().unwrap();
    p.add(egui::Shape::line(pts, Stroke::new(1.5, c)));
    p.circle_filled(last, 2.5, c);
}

fn side_tile(ui: &mut Ui, w: f32, h: f32, x: &Tile, alpha: f32) -> egui::Response {
    let (r0, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let hv = ui.ctx().animate_bool_with_time(resp.id.with("hv"), resp.hovered(), 0.12);
    let r = r0.translate(vec2(0.0, -1.5 * hv));
    let mut p = ui.painter().clone();
    p.multiply_opacity(alpha);
    let light = pal::current().is_light();
    let rad = CornerRadius::same(pal::radius().min(8));
    let sh = egui::Shadow { offset: [0, 2 + hv as i8], blur: 8 + (4.0 * hv) as u8, spread: 0, color: Color32::from_black_alpha(if light { 22 } else { 70 }) };
    p.add(sh.as_shape(r, rad));
    p.rect_filled(r, rad, pal::card2());
    if hv > 0.01 {
        p.rect_stroke(r, rad, Stroke::new(1.0, x.color.gamma_multiply(0.6 * hv)), StrokeKind::Inside);
    }
    // ícone num círculo tingido
    let ic = pos2(r.left() + TILE_PAD + 15.0, r.top() + TILE_PAD + 15.0);
    p.circle_filled(ic, 15.0, x.color.gamma_multiply(0.16));
    p.text(ic, egui::Align2::CENTER_CENTER, x.icon, FontId::proportional(17.0), x.color);
    // valor + unidade; encolhe a fonte até caber (nunca corta)
    let x0 = ic.x + 15.0 + 8.0;
    let room = r.right() - TILE_PAD - x0;
    let (vt, vcol) = match &x.value {
        Some(v) => (v.clone(), x.vcol.unwrap_or(pal::fg())),
        None => ("—".to_string(), pal::faint()),
    };
    let unit = if x.value.is_some() { x.unit.clone() } else { String::new() };
    let mut size = 24.0;
    let (vg, ug) = loop {
        let vg = p.layout_no_wrap(vt.clone(), FontId::proportional(size), vcol);
        let ug = p.layout_no_wrap(unit.clone(), FontId::proportional((size * 0.54).max(11.0)), pal::dim());
        if vg.size().x + 3.0 + ug.size().x <= room || size <= 14.0 {
            break (vg, ug);
        }
        size -= 1.0;
    };
    let vy = ic.y - vg.size().y / 2.0;
    let vw = vg.size().x;
    let vbottom = vy + vg.size().y;
    p.galley(pos2(x0, vy), vg, vcol);
    let uh = ug.size().y;
    p.galley(pos2(x0 + vw + 3.0, vbottom - uh - size * 0.1), ug, pal::dim());
    // rótulo (quebra linha) e nota
    let ww = w - 2.0 * TILE_PAD;
    let mut y = r.top() + TILE_PAD + 30.0 + 6.0;
    let lg = p.layout(x.label.clone(), FontId::proportional(12.5), pal::dim(), ww);
    let lh = lg.size().y;
    p.galley(pos2(r.left() + TILE_PAD, y), lg, pal::dim());
    y += lh;
    if let Some(n) = &x.note {
        let ng = p.layout(n.clone(), FontId::proportional(12.0), pal::faint(), ww);
        p.galley(pos2(r.left() + TILE_PAD, y + 2.0), ng, pal::faint());
    }
    // faixa de baixo: barra de ocupação ou sparkline
    let strip = Rect::from_min_max(pos2(r.left() + TILE_PAD, r.bottom() - TILE_PAD - 16.0), pos2(r.right() - TILE_PAD, r.bottom() - TILE_PAD));
    let sc = x.vcol.unwrap_or(x.color);
    if let Some(f) = x.bar {
        let b = Rect::from_center_size(strip.center(), vec2(strip.width(), 4.0));
        p.rect_filled(b, CornerRadius::same(2), pal::well());
        p.rect_filled(Rect::from_min_size(b.min, vec2(b.width() * f.clamp(0.0, 1.0), 4.0)), CornerRadius::same(2), sc);
    } else if x.spark.len() >= 2 {
        spark(&p, strip, &x.spark, sc.gamma_multiply(0.9));
    }
    match &x.tip {
        Some(tip) => resp.on_hover_text(tip.clone()).on_hover_cursor(egui::CursorIcon::PointingHand),
        None => resp,
    }
}

/// Grade de blocos: 2 colunas (3 com ≥ 520 pt). Entram escalonados de cima para baixo. Devolve o bloco clicado.
fn tile_grid(ui: &mut Ui, tiles: &[Tile], sel_at: f64) -> Option<usize> {
    let avail = ui.available_width();
    let cols = if avail >= 520.0 { 3 } else { 2 };
    let w = ((avail - 8.0 * (cols as f32 - 1.0)) / cols as f32).floor();
    let t = ui.input(|i| i.time);
    let mut clicked = None;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for (ri, row) in tiles.chunks(cols).enumerate() {
            let h = row.iter().map(|x| tile_height(ui, x, w)).fold(88.0, f32::max);
            ui.horizontal(|ui| {
                for (ci, x) in row.iter().enumerate() {
                    let i = ri * cols + ci;
                    let a = ((t - sel_at - 0.04 * i as f64) / 0.18).clamp(0.0, 1.0) as f32;
                    if a < 1.0 {
                        ui.ctx().request_repaint();
                    }
                    if side_tile(ui, w, h, x, a).clicked() {
                        clicked = Some(i);
                    }
                }
            });
        }
    });
    clicked
}

/// Cabeçalho do painel lateral: ilustração do item, título, subtítulo, pílula de estado e ✕.
fn side_hero(ui: &mut Ui, e: f32, accent: Color32, art: &dyn Fn(&egui::Painter, Rect), title: &str, sub: &str, state: Option<(&str, Color32)>, sel: &mut Option<Sel>) {
    let light = pal::current().is_light();
    let rad = CornerRadius::same(pal::radius());
    let t = ui.input(|i| i.time);
    let fr = egui::Frame::new()
        .fill(pal::card2())
        .corner_radius(rad)
        .inner_margin(egui::Margin::same(12))
        .shadow(egui::Shadow { offset: [0, 3], blur: 12, spread: 0, color: Color32::from_black_alpha(if light { 30 } else { 90 }) })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let close = Rect::from_min_size(ui.max_rect().right_top() + vec2(-24.0, -6.0), vec2(28.0, 28.0));
            let aw = (ui.available_width() * 0.4).clamp(100.0, 140.0);
            let ah = aw * 88.0 / 140.0;
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 88.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let (ar, _) = ui.allocate_exact_size(vec2(aw, ah), Sense::hover());
                art(ui.painter(), Rect::from_center_size(ar.center(), ar.size() * (0.9 + 0.1 * e)));
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - 30.0).max(80.0));
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.label(egui::RichText::new(title).size(19.0).strong().color(pal::fg()));
                    if !sub.is_empty() {
                        ui.label(egui::RichText::new(sub).size(13.0).color(pal::dim()));
                    }
                    if let Some((s, c)) = state {
                        ui.add_space(4.0);
                        status_pill(ui, s, c, t, c == pal::red());
                    }
                });
            });
            let r = ui.interact(close, ui.id().with("fechar"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            if r.hovered() {
                ui.painter().rect_filled(close, CornerRadius::same(6), pal::card3());
            }
            ui.painter().text(close.center(), egui::Align2::CENTER_CENTER, "✕", FontId::proportional(15.0), if r.hovered() { pal::fg() } else { pal::dim() });
            if r.on_hover_text("Fechar (Esc)").clicked() {
                *sel = None;
            }
        });
    // filete de 3 pt no topo, na cor do item, seguindo os cantos do cartão
    let r = fr.response.rect;
    ui.painter().with_clip_rect(Rect::from_min_max(r.min, pos2(r.right(), r.top() + 3.0))).rect_filled(r, rad, accent);
}

/// Frase curta e amigável logo abaixo do hero.
fn side_hint(ui: &mut Ui, s: &str) {
    ui.add_space(10.0);
    ui.label(egui::RichText::new(s).size(14.0).color(pal::dim()));
    ui.add_space(8.0);
}

/// Título pequeno de uma faixa (Portas, Dispositivos...).
fn side_caption(ui: &mut Ui, s: &str) {
    ui.add_space(12.0);
    ui.label(egui::RichText::new(s).size(13.0).strong().color(pal::dim()));
    ui.add_space(2.0);
}

/// Chip arredondado, opcionalmente com bolinha de estado. Clicável quando `click`.
fn side_chip(ui: &mut Ui, text: &str, dot: Option<Color32>, click: bool) -> egui::Response {
    // nome longo quebra linha dentro do chip em vez de alargar o painel
    let maxw = (ui.available_width() - 20.0 - if dot.is_some() { 14.0 } else { 0.0 } - if click { 14.0 } else { 0.0 }).max(60.0);
    let r = egui::Frame::new()
        .fill(pal::card2())
        .stroke(Stroke::new(1.0, pal::border()))
        .corner_radius(CornerRadius::same(pal::radius().max(2).min(14)))
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.set_max_width(maxw + 28.0);
                ui.add(egui::Label::new(egui::RichText::new(text).size(14.0).color(pal::fg())).wrap());
                if let Some(c) = dot {
                    let (d, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                    ui.painter().circle_filled(d.center(), 4.0, c);
                }
                if click {
                    ui.label(egui::RichText::new("›").size(14.0).color(pal::faint()));
                }
            });
        })
        .response;
    if !click {
        return r;
    }
    let resp = ui.interact(r.rect, ui.id().with(("chip", text)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.hovered() {
        ui.painter().rect_stroke(r.rect, CornerRadius::same(pal::radius().max(2).min(14)), Stroke::new(1.0, pal::cyan().gamma_multiply(0.7)), StrokeKind::Inside);
    }
    resp
}

/// Detalhes crus no fim, recolhidos.
fn side_tech(ui: &mut Ui, key: &str, add: impl FnOnce(&mut Ui)) {
    ui.add_space(14.0);
    egui::CollapsingHeader::new(egui::RichText::new("🔧 Detalhes técnicos").size(13.5).color(pal::dim()))
        .id_salt(("tech", key))
        .default_open(false)
        .show(ui, add);
}

/// "hoje 01:46", "ontem 14:04" ou "12/11 03:27".
fn friendly_time(ms: i64) -> String {
    use chrono::TimeZone;
    let Some(t) = chrono::Local.timestamp_millis_opt(ms).single() else { return String::new() };
    let today = chrono::Local::now().date_naive();
    let d = t.date_naive();
    if d == today {
        format!("hoje {}", t.format("%H:%M"))
    } else if Some(d) == today.pred_opt() {
        format!("ontem {}", t.format("%H:%M"))
    } else {
        t.format("%d/%m %H:%M").to_string()
    }
}

fn incident_row(ui: &mut Ui, inc: &Incident, open: &mut Option<i64>) {
    let head = inc.events.iter().max_by_key(|e| (e.kind.severity(), e.kind.is_drop() as u8)).unwrap();
    let color = if inc.is_drop { pal::red() } else { sev_color(inc.sev) };
    let is_open = *open == Some(inc.start);
    egui::Frame::new()
        .fill(if is_open { pal::card2() } else { Color32::TRANSPARENT })
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            let r = ui.horizontal(|ui| {
                ui.add_sized([24.0, 20.0], egui::Label::new(egui::RichText::new(kind_icon(head.kind)).color(color).size(15.0)));
                ui.add_sized([96.0, 18.0], egui::Label::new(egui::RichText::new(friendly_time(inc.start)).color(pal::dim()).size(14.0)));
                ui.label(egui::RichText::new(head.kind.label()).color(color).strong());
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
                    ui.label(egui::RichText::new(format!("×{}", inc.events.len())).color(pal::faint()).size(12.5));
                }
            });
            let click = ui.interact(r.response.rect, ui.id().with(("inc", inc.start)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            if click.clicked() {
                *open = if is_open { None } else { Some(inc.start) };
            }
            if is_open {
                for e in &inc.events {
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(22.0);
                        ui.label(egui::RichText::new(kind_icon(e.kind)).color(cat_color(e.kind.cat())));
                        ui.label(egui::RichText::new(fmt_local(e.ts, false)).size(13.0).color(pal::faint()));
                        ui.label(egui::RichText::new(&e.msg).size(13.5)).on_hover_text(format!("fonte: {}\n{}", e.src, e.dev.clone().unwrap_or_default()));
                    });
                    if let Some(s) = &e.snap {
                        ui.horizontal_wrapped(|ui| {
                            ui.add_space(40.0);
                            ui.label(egui::RichText::new(format!("📸 {s}")).size(13.0).color(pal::cyan()));
                        });
                    }
                }
                if inc.is_drop {
                    let before = history::get().before(inc.start, 120_000);
                    ui.horizontal(|ui| {
                        ui.add_space(22.0);
                        ui.label(egui::RichText::new(format!("⏪ 2 min antes: {}", if before.is_empty() { "nada no Event Log (queda sem aviso)".to_string() } else { format!("{} evento(s)", before.len()) })).strong().color(pal::amber()).size(13.5));
                    });
                    for e in before.iter().rev().take(20) {
                        ui.horizontal_wrapped(|ui| {
                            ui.add_space(40.0);
                            ui.label(egui::RichText::new(format!("−{:.1} s", (inc.start - e.ts) as f64 / 1000.0)).monospace().size(13.0).color(pal::faint()));
                            ui.label(egui::RichText::new(kind_icon(e.kind)).color(cat_color(e.kind.cat())));
                            ui.label(egui::RichText::new(&e.msg).size(13.0).color(pal::dim()));
                        });
                    }
                }
                ui.add_space(3.0);
            }
        });
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
            ui.label(egui::RichText::new(k).color(pal::dim()).size(13.0));
            ui.label(egui::RichText::new(v).size(13.0));
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
            ui.label(egui::RichText::new(h).color(pal::faint()).size(12.5));
        }
        ui.end_row();
        for p in props {
            ui.label(egui::RichText::new(p.pid.to_string()).monospace().size(13.0));
            ui.label(egui::RichText::new(format!("{} {}", p.val.type_name(), p.val.show())).monospace().size(12.5).color(pal::dim()));
            ui.label(egui::RichText::new(format!("{}\n{}", p.meaning, p.decoded)).size(13.0));
            let c = match p.confidence {
                "certo" => pal::green(),
                "forte" => pal::cyan(),
                "hipótese" => pal::amber(),
                _ => pal::faint(),
            };
            ui.label(egui::RichText::new(p.confidence).color(c).size(13.0));
            ui.end_row();
        }
    });
}

fn bars(ui: &mut Ui, r: &BenchResult) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 74.0), Sense::hover());
    let p = ui.painter_at(rect);
    let max = 3.2f32;
    let x_of = |v: f32| rect.left() + 150.0 + (rect.width() - 230.0) * (v / max).clamp(0.0, 1.0);
    // faixa esperada TB3 40G
    let band = Rect::from_x_y_ranges(x_of(2.5)..=x_of(2.8), rect.top()..=rect.bottom() - 14.0);
    for x in [band.left(), band.right()] {
        p.extend(egui::Shape::dashed_line(&[pos2(x, band.top()), pos2(x, band.bottom())], Stroke::new(1.0, pal::green().gamma_multiply(0.7)), 4.0, 3.0));
    }
    p.text(pos2(band.center().x, rect.bottom()), egui::Align2::CENTER_BOTTOM, "normal com cabo de 40 Gb/s", FontId::proportional(12.5), pal::green());
    for (i, (label, v, c)) in [("Notebook → placa", r.h2d_gbs, pal::data()), ("Placa → notebook", r.d2h_gbs, pal::violet())].iter().enumerate() {
        let y = rect.top() + 8.0 + i as f32 * 26.0;
        p.text(pos2(rect.left(), y + 8.0), egui::Align2::LEFT_CENTER, *label, FontId::proportional(13.5), pal::dim());
        let bar = Rect::from_min_max(pos2(x_of(0.0), y), pos2(x_of(*v as f32), y + 16.0));
        p.rect_filled(bar, CornerRadius::same(3), *c);
        p.text(pos2(bar.right() + 6.0, y + 8.0), egui::Align2::LEFT_CENTER, format!("{v:.2} GB/s"), FontId::proportional(14.0), pal::fg());
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
