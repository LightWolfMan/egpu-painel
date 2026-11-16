//! Interface egui/eframe (renderizador glow). Foco na placa do dock: topologia animada,
//! enlace USB4/TB, switch PCIe e portas, PD, estabilidade e linha do tempo de quedas.

use crate::i18n::t;
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
/// detectada no dock). `cyan()` é a cor de destaque/marca do tema: só faixa, logo, aba ativa e
/// filetes. Dado usa `data()`/`violet()`/`bolt()`, estado usa `green()`/`amber()`/`red()`, link usa `link()`.
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
                Th::Light => crate::i18n::t("Claro", "Light"),
                Th::Dark => crate::i18n::t("Escuro", "Dark"),
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
        [c(242, 244, 247), c(255, 255, 255), c(244, 246, 249), c(236, 240, 246), c(246, 248, 251), c(226, 230, 236), c(214, 220, 227), c(28, 33, 40), c(90, 99, 110), c(150, 158, 168), c(2, 132, 199), c(26, 127, 55), c(176, 112, 0), c(207, 34, 46), c(130, 80, 223), c(200, 30, 40), c(0, 104, 181), c(146, 134, 84), c(100, 125, 160), c(31, 111, 178)],
        // escuro
        [c(13, 17, 23), c(22, 27, 34), c(30, 36, 45), c(26, 33, 44), c(16, 20, 26), c(33, 39, 47), c(48, 54, 61), c(230, 237, 243), c(150, 160, 170), c(118, 126, 136), c(56, 189, 248), c(63, 185, 80), c(210, 153, 34), c(248, 81, 73), c(163, 113, 247), c(237, 70, 70), c(64, 140, 230), c(222, 212, 156), c(120, 150, 190), c(56, 189, 248)],
        // AMD (Adrenalin): fundo #111, cartões #262626 com faixa de título preta, vermelho #E4002B
        [c(17, 17, 18), c(38, 38, 40), c(48, 48, 50), c(54, 54, 58), c(12, 12, 13), c(52, 52, 55), c(38, 38, 40), c(242, 242, 244), c(190, 190, 196), c(140, 140, 148), c(228, 0, 43), c(46, 204, 113), c(255, 170, 30), c(255, 99, 120), c(175, 125, 255), c(237, 28, 36), c(80, 150, 230), c(228, 218, 168), c(150, 150, 162), c(110, 185, 235)],
        // NVIDIA: grafite + verde. O verde de marca (118,185,0) é só destaque (logo, aba, filete);
        // o verde de estado "ok" é mais frio (63,185,80) para não se confundir com ele.
        [c(24, 24, 24), c(34, 34, 34), c(44, 44, 44), c(50, 50, 50), c(28, 28, 28), c(52, 52, 52), c(58, 58, 58), c(238, 238, 238), c(180, 180, 180), c(132, 132, 132), c(118, 185, 0), c(63, 185, 80), c(240, 170, 0), c(255, 88, 88), c(160, 125, 240), c(237, 70, 70), c(80, 150, 230), c(222, 214, 160), c(135, 155, 120), c(79, 163, 224)],
        // Intel: azul-marinho + ciano
        [c(9, 17, 31), c(16, 27, 45), c(23, 37, 59), c(29, 44, 68), c(11, 21, 37), c(31, 47, 71), c(35, 53, 79), c(232, 240, 250), c(156, 176, 200), c(110, 130, 158), c(0, 199, 253), c(70, 200, 120), c(255, 180, 0), c(255, 92, 92), c(150, 130, 255), c(237, 70, 70), c(0, 104, 181), c(226, 220, 172), c(110, 150, 200), c(120, 170, 255)],
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
    /// Energia (fios, PD, fonte): amarelo-pálido dessaturado, sem significado de estado.
    /// Não confundir com `amber()`, que é só atenção.
    pub fn bolt() -> Color32 { at(17) }
    pub fn pcie() -> Color32 { at(18) }
    /// Cor de dados (tráfego, gráficos): sempre um azul neutro, nunca a cor da marca nem a de
    /// estado (no AMD não é o vermelho; no NVIDIA não é o verde de "ok"; no Intel não é o ciano).
    pub fn data() -> Color32 { at(19) }
    /// Série "Temperatura" (medidor, gráfico e painel lateral): verde-azulado, sem estado.
    pub fn temp() -> Color32 {
        if current().is_light() { c(0, 128, 134) } else { c(72, 196, 192) }
    }
    /// Série "Ponto mais quente": rosa-magenta, distinto da ventoinha (violeta) e do vermelho.
    pub fn hot() -> Color32 {
        if current().is_light() { c(176, 58, 146) } else { c(226, 124, 200) }
    }
    /// Cor de link: no AMD o vermelho é só marca (faixa, logo, aba ativa), então link é
    /// cinza-claro (e sublinhado, ver `link_text`); nos outros temas, o destaque do tema.
    pub fn link() -> Color32 {
        if current() == Th::Amd { c(214, 214, 220) } else { cyan() }
    }

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
    /// Cor da seleção: neutra em todos os temas (texto com 60% de alfa). A cor de marca
    /// (vermelha no AMD, verde no NVIDIA) não marca estado nem seleção.
    pub fn sel() -> Color32 {
        fg().gamma_multiply(0.6)
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

/// Cor de categoria de evento (dado, não estado): nunca a cor da marca nem o vermelho de erro
/// (antes Dock = destaque da marca, vermelho no AMD, e GPU = vermelho AMD em todos os temas).
fn cat_color(c: Cat) -> Color32 {
    match c {
        Cat::Dock => pal::data(),
        Cat::Gpu => pal::violet(),
        Cat::Pcie => pal::pcie(),
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
        crate::history::note(Kind::Note, tf!("Falha ao abrir a janela: {e}", "Failed to open the window: {e}"));
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
            Range::H24 => "24\u{A0}h",
            Range::D7 => t("7\u{A0}dias", "7\u{A0}days"),
            Range::D30 => t("30\u{A0}dias", "30\u{A0}days"),
            Range::All => t("tudo", "all"),
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
    /// Carga da fonte calculada uma vez por quadro (App::ui): nó, painel lateral e cartão usam os mesmos números.
    psu_load: Option<crate::psu::Load>,
    fb: Option<FbPanel>,
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
                Some(v) if v.starts_with("info") => match v[4..].parse::<u8>().ok() {
                    Some(0) => Some(Sel::Info(Topic::Link)),
                    Some(1) => Some(Sel::Info(Topic::Power)),
                    Some(2) => Some(Sel::Info(Topic::Stability)),
                    Some(3) => Some(Sel::Info(Topic::Flow)),
                    Some(4) => Some(Sel::Info(Topic::Gpu)),
                    Some(5) => Some(Sel::Info(Topic::Hidden)),
                    _ => None,
                },
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
            psu_load: None,
            // EGPU_FEEDBACK=1 abre com o painel de feedback (capturas e testes)
            fb: std::env::var("EGPU_FEEDBACK").is_ok().then(FbPanel::default),
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
        *st.lock().unwrap() = BenchState::Running(t("iniciando…", "starting…").into());
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
                        tf!(
                            "host→GPU {:.2}\u{A0}GB/s · GPU→host {:.2}\u{A0}GB/s ({} MiB por medição, fila COPY D3D12)",
                            "host→GPU {:.2}\u{A0}GB/s · GPU→host {:.2}\u{A0}GB/s ({} MiB per run, D3D12 COPY queue)",
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
    // seleção e links nunca na cor de marca do AMD (o vermelho lá é só faixa/logo/aba ativa)
    v.selection.bg_fill = if th == Th::Amd { Color32::from_gray(125) } else { pal::cyan().gamma_multiply(0.45) };
    v.selection.stroke = Stroke::new(1.0, pal::link());
    v.hyperlink_color = pal::link();
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
    // EGPU_TEMA=amd|nvidia|intel simula outra placa (para testar os temas automáticos);
    // EGPU_TEMA=claro|escuro força o tema neutro sem tocar no config.json
    let vendor = match std::env::var("EGPU_TEMA").ok().as_deref() {
        Some("amd") => Some(Th::Amd),
        Some("nvidia") => Some(Th::Nvidia),
        Some("intel") => Some(Th::Intel),
        Some("claro") => return Th::Light,
        Some("escuro") => return Th::Dark,
        _ => vendor,
    };
    match cfg {
        1 => Th::Light,
        2 => Th::Dark,
        _ => vendor.unwrap_or(Th::Dark),
    }
}

/// Cor da barra de título nativa por tema (None = padrão do sistema, no claro).
fn caption_rgb(th: Th) -> Option<(u8, u8, u8)> {
    match th {
        Th::Light => None,
        Th::Dark => {
            let c = pal::bg();
            Some((c.r(), c.g(), c.b()))
        }
        // igual à faixa do topo (amd_bar)
        Th::Amd => Some((32, 32, 34)),
        // verde bem escuro, da família do card3 NVIDIA (36,48,18)
        Th::Nvidia => Some((20, 36, 8)),
        // azul-marinho um tom acima do well Intel (11,21,37)
        Th::Intel => Some((11, 24, 44)),
    }
}

const URL_SPONSORS: &str = "https://github.com/sponsors/LightWolfMan";
const URL_PIX: &str = "https://github.com/LightWolfMan/egpu-painel#apoie";

/// Abre um endereço no navegador padrão.
fn open_url(url: &str) {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let u = crate::util::wide(url);
    unsafe {
        ShellExecuteW(None, windows::core::w!("open"), windows::core::PCWSTR(u.as_ptr()), windows::core::PCWSTR::null(), windows::core::PCWSTR::null(), SW_SHOWNORMAL);
    }
}

#[derive(Clone, Copy)]
enum FootIcon {
    Text(&'static str),
    Pix,
}

/// Botão-link do rodapé: ícone + texto curto (ou só ícone quando estreito), realce e sublinhado no hover.
/// `url` = None: só devolve o clique (o Feedback abre um painel, não um site).
fn foot_link(ui: &mut Ui, icon: FootIcon, label: &str, tip: &str, url: Option<&'static str>, compact: bool) -> bool {
    let font = egui::FontId::proportional(12.5);
    let tg = ui.painter().layout_no_wrap(label.to_owned(), font.clone(), pal::dim());
    let iw = 14.0;
    let w = 8.0 + iw + if compact { 0.0 } else { 5.0 + tg.size().x } + 8.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 22.0), egui::Sense::click());
    let k = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let col = lerp_color(pal::dim(), pal::fg(), k);
    let p = ui.painter();
    if k > 0.0 {
        let f = pal::card2();
        p.rect_filled(rect, (pal::radius() as f32).min(6.0), Color32::from_rgba_unmultiplied(f.r(), f.g(), f.b(), (k * 255.0) as u8));
    }
    let ic = pos2(rect.left() + 8.0 + iw / 2.0, rect.center().y);
    match icon {
        FootIcon::Text(s) => {
            p.text(ic, egui::Align2::CENTER_CENTER, s, egui::FontId::proportional(13.0), col);
        }
        FootIcon::Pix => {
            // selo simples: losango vazado com miolo cheio (lembra o símbolo do Pix)
            let pix = lerp_color(col, Color32::from_rgb(50, 188, 173), k);
            let d = |s: f32| vec![pos2(ic.x, ic.y - s), pos2(ic.x + s, ic.y), pos2(ic.x, ic.y + s), pos2(ic.x - s, ic.y)];
            p.add(egui::Shape::closed_line(d(6.0), Stroke::new(1.6, pix)));
            p.add(egui::Shape::convex_polygon(d(2.4), pix, Stroke::NONE));
        }
    }
    if !compact {
        let tp = pos2(ic.x + iw / 2.0 + 5.0, rect.center().y - tg.size().y / 2.0);
        let ty = tg.size().y;
        let tw = tg.size().x;
        p.galley_with_override_text_color(tp, tg, col);
        if resp.hovered() {
            p.hline(tp.x..=tp.x + tw, tp.y + ty - 1.0, Stroke::new(1.0, col));
        }
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
    if let Some(url) = url.filter(|_| resp.clicked()) {
        open_url(url);
    }
    resp.clicked()
}

fn lerp_color(a: Color32, b: Color32, k: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Rodapé discreto: versão e licença à esquerda; feedback e apoio à direita.
/// Botão "Ciente" do aviso de instabilidade (reconhecer, estilo Zabbix).
fn ack_button(ui: &mut Ui, col: Color32) -> bool {
    ui.add(
        egui::Button::new(egui::RichText::new(t("✔ Ciente", "✔ Got it")).size(14.0).color(pal::fg()))
            .fill(pal::card())
            .stroke(Stroke::new(1.0, col.gamma_multiply(0.7)))
            .corner_radius(CornerRadius::same(6)),
    )
    .on_hover_text(t("Esconde este aviso até a próxima queda. As quedas continuam registradas no Histórico.", "Hides this warning until the next drop. Drops stay recorded in History."))
    .clicked()
}

/// Devolve `true` quando o link "Feedback" foi clicado (abre o painel de feedback).
fn footer(ui: &mut Ui) -> bool {
    let full = ui.max_rect().expand2(vec2(16.0, 0.0));
    ui.painter().hline(full.x_range(), full.top() + 0.5, Stroke::new(1.0, pal::grid()));
    let ver = concat!("v", env!("CARGO_PKG_VERSION"));
    let long = tf!("eGPU Painel {ver} · código aberto (GPL-3.0)", "eGPU Painel {ver} · open source (GPL-3.0)");
    let font = egui::FontId::proportional(12.0);
    let lw = ui.painter().layout_no_wrap(long.clone(), font.clone(), pal::faint()).size().x;
    // largura dos três botões com texto (ícone 14 + 5 + texto + 16 de folga, ~8 entre eles)
    let links_full = 3.0 * (14.0 + 5.0 + 16.0) + 70.0 + 55.0 + 22.0 + 2.0 * 8.0;
    let avail = ui.available_width();
    let compact = avail < lw + links_full + 40.0;
    let left = if avail < lw + 3.0 * 30.0 + 40.0 { format!("{ver} · GPL-3.0") } else { long };
    let mut fb = false;
    ui.horizontal_centered(|ui| {
        ui.label(egui::RichText::new(left).font(font).color(pal::faint()))
            .on_hover_text(t("Código aberto sob a GPL-3.0: use, estude, modifique e compartilhe à vontade. Quem distribuir uma versão modificada mantém o código aberto também.", "Open source under the GPL-3.0: use, study, modify and share freely. Anyone who distributes a modified version must keep it open source too."));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            foot_link(ui, FootIcon::Pix, "Pix", t("Pix (QR Code ou copia e cola), para quem está no Brasil", "Pix (QR Code or copy-and-paste), for people in Brazil"), Some(URL_PIX), compact);
            foot_link(ui, FootIcon::Text("❤"), t("Apoiar", "Support"), t("GitHub Sponsors — de qualquer país, cartão internacional", "GitHub Sponsors — from any country, international card"), Some(URL_SPONSORS), compact);
            fb = foot_link(ui, FootIcon::Text("💬"), "Feedback", t("Feedback — relatar um problema ou sugerir uma melhoria", "Feedback — report a problem or suggest an improvement"), None, compact);
        });
    });
    fb
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
            Tab::Geral => t("Visão geral", "Overview"),
            Tab::Enlace => t("Cabo e energia", "Cable & power"),
            Tab::Fluxo => t("Desempenho", "Performance"),
            Tab::Historico => t("Histórico", "History"),
            Tab::Opcoes => t("Opções", "Options"),
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

// ---- linhas de cartões com a mesma altura
// `eq_cols` mede, a cada quadro, a altura natural (sem esticar) de cada coluna e quantos cartões
// de primeiro nível ela tem. No quadro seguinte, o ÚLTIMO cartão de cada coluna estica até a
// coluna mais alta do quadro anterior: as bases alinham. A medida ignora o esticão, então não oscila.
struct EqCol {
    last: usize,
    extra: f32,
    count: usize,
    applied: f32,
}

thread_local! {
    static EQ: std::cell::RefCell<Option<EqCol>> = const { std::cell::RefCell::new(None) };
    static CARD_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static FLEX: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
}

/// Sobra de altura do cartão que está esticando numa linha `eq_cols` (fora dele é 0).
/// Um elemento flexível (gráfico, faixas, blocos) chama `flex(parte, teto)` ANTES de se alocar e
/// cresce o que recebeu: `parte` da sobra que resta, no máximo `teto` px. O que ninguém pega fica
/// como vão no pé do cartão. Vem da medida do quadro anterior (que ignora o crescimento): não oscila.
fn flex(share: f32, max: f32) -> f32 {
    FLEX.with(|f| {
        let s = f.get();
        let take = (s * share).min(max).max(0.0).floor();
        f.set(s - take);
        take
    })
}

/// `ui.columns(n)` com os cartões de cada coluna terminando na mesma linha de base.
fn eq_cols(ui: &mut Ui, id: &str, n: usize, mut col: impl FnMut(usize, &mut Ui)) {
    let gid = egui::Id::new(("eq-row", id, n));
    let prev: Vec<(f32, usize)> = ui.ctx().data(|d| d.get_temp(gid)).unwrap_or_default();
    let target = prev.iter().map(|p| p.0).fold(0.0, f32::max);
    let mut now = vec![(0.0f32, 0usize); n];
    ui.columns(n, |c| {
        for (i, cu) in c.iter_mut().enumerate() {
            let (nat, last) = prev.get(i).copied().unwrap_or((0.0, 0));
            let extra = if prev.len() == n && last > 0 && n > 1 { (target - nat).max(0.0) } else { 0.0 };
            EQ.with(|e| *e.borrow_mut() = Some(EqCol { last, extra, count: 0, applied: 0.0 }));
            col(i, cu);
            let e = EQ.with(|e| e.borrow_mut().take()).unwrap_or(EqCol { last: 0, extra: 0.0, count: 0, applied: 0.0 });
            now[i] = (cu.min_rect().height() - e.applied, e.count);
        }
    });
    // tolera 1 px: variações menores não reescrevem a medida (sem tremedeira)
    let changed = prev.len() != n || prev.iter().zip(&now).any(|(a, b)| (a.0 - b.0).abs() > 1.0 || a.1 != b.1);
    if changed {
        ui.ctx().data_mut(|d| d.insert_temp(gid, now));
        ui.ctx().request_repaint();
    }
}

/// Moldura comum dos cartões. No tema AMD: cantos retos, faixa de título preta em caixa alta,
/// corpo com respiro de 18 px (como os blocos "EXPERIÊNCIA GLOBAL" / "DRIVER E SOFTWARE").
fn card_shell<R>(ui: &mut Ui, title: &str, accent: Color32, info: Option<(&mut Option<Sel>, Topic, &str)>, add: impl FnOnce(&mut Ui, Option<&mut Option<Sel>>) -> R) -> R {
    // dentro de uma linha `eq_cols`, o último cartão da coluna estica até a base comum
    let depth = CARD_DEPTH.with(|d| d.get());
    let stretch = if depth == 0 {
        EQ.with(|e| {
            e.borrow_mut().as_mut().and_then(|e| {
                e.count += 1;
                (e.count == e.last && e.extra > 0.5).then_some(e.extra)
            })
        })
    } else {
        None
    };
    CARD_DEPTH.with(|d| d.set(depth + 1));
    if let Some(x) = stretch {
        FLEX.with(|f| f.set(x));
    }
    let out = card_shell_in(ui, title, accent, info, |ui, sel| {
        let r = add(ui, sel);
        if let Some(x) = stretch {
            // o que os elementos flexíveis já cresceram sai da sobra; o resto vira vão no pé
            let used = x - FLEX.with(|f| f.replace(0.0));
            // (set_min_height mede a partir do cursor, não do topo: aqui a base é posta direto)
            let m = ui.min_rect();
            let rest = (x - used).max(0.0);
            if rest > 0.5 {
                ui.expand_to_include_rect(Rect::from_min_max(pos2(m.left(), m.bottom()), pos2(m.left() + 1.0, m.bottom() + rest)));
            }
            let grown = ui.min_rect().height() - m.height();
            EQ.with(|e| {
                if let Some(e) = e.borrow_mut().as_mut() {
                    e.applied = used + grown;
                }
            });
        }
        r
    });
    CARD_DEPTH.with(|d| d.set(depth));
    out
}

fn card_shell_in<R>(ui: &mut Ui, title: &str, accent: Color32, mut info: Option<(&mut Option<Sel>, Topic, &str)>, add: impl FnOnce(&mut Ui, Option<&mut Option<Sel>>) -> R) -> R {
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

/// Texto de link: cor de link do tema; no AMD, sublinhado (lá o link é cinza-claro, não vermelho).
fn link_text(s: impl Into<String>) -> egui::RichText {
    let t = egui::RichText::new(s.into()).color(pal::link());
    if pal::current() == Th::Amd { t.underline() } else { t }
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

/// Gráfico de linhas com legenda. O nome da série aceita unidade própria em "Nome|unidade"
/// (ex.: "Temperatura|°C"); sem ela, vale `unit`. A legenda quebra em linhas (nunca corta) e o
/// valor fica preso à unidade por espaço não separável. Séries usam cor de dado, não de estado.
fn chart(ui: &mut Ui, label: &str, series: &[(Color32, &str, Vec<f32>)], max: f32, unit: &str, h: f32) {
    let w = ui.available_width();
    let p0 = ui.painter().clone();
    let lg = p0.layout_no_wrap(label.to_string(), FontId::proportional(13.5), pal::dim());
    let lw0 = lg.size().x;
    // legenda: mede antes para reservar as linhas que ela precisar
    let items: Vec<(Color32, std::sync::Arc<egui::Galley>)> = series
        .iter()
        .filter(|s| s.2.len() >= 2)
        .map(|(color, name, data)| {
            let (nm, u) = match name.split_once('|') {
                Some((n, u)) => (n, format!("\u{A0}{u}")),
                None => (*name, unit.replace(' ', "\u{A0}")),
            };
            let last = data.last().copied().unwrap_or(0.0);
            let txt = format!("{nm}\u{A0}{last:.0}{u}");
            (*color, p0.layout_no_wrap(txt, FontId::proportional(12.5), *color))
        })
        .collect();
    let (x_min, x_max) = (10.0, w - 8.0);
    let x_first = x_min + lw0 + 18.0;
    let mut x = x_first;
    let mut row = 0usize;
    let mut pos: Vec<(f32, usize)> = Vec::new();
    for (_, g) in &items {
        if x + g.size().x > x_max && x > x_first + 0.5 || (row > 0 && x + g.size().x > x_max && x > x_min + 0.5) {
            row += 1;
            x = x_min;
        }
        pos.push((x, row));
        x += g.size().x + 14.0;
    }
    let extra = 17.0 * row as f32;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h + extra), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(6), pal::well());
    let plot = Rect::from_min_max(pos2(rect.left() + 8.0, rect.top() + 30.0 + extra), pos2(rect.right() - 8.0, rect.bottom() - 8.0));
    for i in 0..=3 {
        let y = plot.bottom() - plot.height() * i as f32 / 3.0;
        p.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], Stroke::new(1.0, pal::grid()));
    }
    let maxv = series
        .iter()
        .flat_map(|s| s.2.iter().cloned())
        .fold(max, f32::max)
        .max(1e-3);
    p.galley(rect.left_top() + vec2(10.0, 6.0), lg, pal::dim());
    let n = collect::HISTORY as f32;
    for (color, _name, data) in series {
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
            p.add(egui::Shape::line(pts, Stroke::new(1.6, *color)));
        }
    }
    let mut legend_end = rect.left() + x_min + lw0;
    for ((color, g), (lx, r)) in items.into_iter().zip(pos) {
        if r == 0 {
            legend_end = rect.left() + lx + g.size().x;
        }
        p.galley(pos2(rect.left() + lx, rect.top() + 6.0 + 17.0 * r as f32), g, color);
    }
    // "máx" só quando sobra lugar depois da legenda (nunca encosta nela)
    let mg = p.layout_no_wrap(tf!("máx {maxv:.0}\u{A0}{}", "max {maxv:.0}\u{A0}{}", unit.trim()).trim_end().to_string(), FontId::proportional(12.0), pal::faint());
    if legend_end + 14.0 + mg.size().x <= rect.right() - 8.0 {
        p.galley(pos2(rect.right() - 8.0 - mg.size().x, rect.top() + 4.0), mg, pal::faint());
    }
    if data_empty(series) {
        p.text(rect.center(), egui::Align2::CENTER_CENTER, t("aguardando amostras…", "waiting for samples…"), FontId::proportional(13.5), pal::faint());
    }
    let _ = resp;
}

/// Medidor de ocupação: rótulo à esquerda e valor à direita EM CIMA, barra fina embaixo (como a
/// barra de carga da fonte), para a borda do preenchimento nunca cortar o texto. O rótulo usado é o
/// primeiro de `labels` que cabe ao lado do valor (o último é o mais curto).
fn meter_bar(ui: &mut Ui, frac: f32, labels: &[&str], value: &str, color: Color32) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0 + 4.0 + 7.0), Sense::hover());
    let p = ui.painter();
    let vg = p.layout_no_wrap(value.to_string(), FontId::proportional(13.5), pal::fg());
    let room = r.width() - vg.size().x - 10.0;
    let label = labels.iter().copied().find(|t| p.layout_no_wrap(t.to_string(), FontId::proportional(12.5), pal::dim()).size().x <= room);
    if let Some(l) = label {
        p.text(pos2(r.left(), r.top() + 9.0), egui::Align2::LEFT_CENTER, l, FontId::proportional(12.5), pal::dim());
    }
    p.galley(pos2(r.right() - vg.size().x, r.top() + 9.0 - vg.size().y / 2.0), vg, pal::fg());
    let track = Rect::from_min_max(pos2(r.left(), r.bottom() - 7.0), r.max);
    let rad = CornerRadius::same(3);
    // trilho com contraste nos dois temas (no claro o `well` some sobre o cartão)
    if pal::current().is_light() {
        p.rect_filled(track, rad, pal::grid());
    } else {
        p.rect_filled(track, rad, pal::well());
    }
    let f = ui.ctx().animate_value_with_time(resp.id, frac.clamp(0.0, 1.0), 0.4);
    if f > 0.0 {
        p.rect_filled(Rect::from_min_size(track.min, vec2((track.width() * f).max(4.0), track.height())), rad, color);
    }
    resp
}

/// "2,2 de 16 GB": usado com 1 casa, total arredondado (mesmo formato em todo o painel).
fn gb_of(used_mb: f64, total_mb: f64) -> String {
    tf!("{} de {:.0}\u{A0}GB", "{} of {:.0}\u{A0}GB", dec(used_mb / 1024.0, 1), total_mb / 1024.0)
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
    // selecionado: sobe como no hover máximo (anel neutro + sombra maior, sem cor de marca)
    p.add(pal::elev(2, if selected { 1.0 } else { lift }).as_shape(rect, cr));
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
    // faixa lateral: cor do tipo do nó (neutra); com problema/atenção, a cor do estado
    let bar_col = if state == pal::red() || state == pal::amber() { state } else { accent };
    p.rect_filled(bar, CornerRadius::same(2), bar_col);
    p.circle_filled(rect.right_top() + vec2(-12.0, 12.0), 4.5, state);
    // ilustração à esquerda e o texto depois dela; em nó estreito (< 180 pt) a ilustração vai
    // para uma faixa no alto e o texto ocupa a largura toda embaixo (só em nó alto, ≥ 90 pt)
    let stacked = !focus && art.is_some() && rect.width() < 180.0 && rect.height() >= 90.0;
    let (tx, top_pad) = match art {
        Some(f) if stacked => {
            let ar = Rect::from_min_size(rect.min + vec2(12.0, 8.0), vec2((rect.width() - 44.0).min(70.0), 30.0));
            f(p, ar);
            (12.0, 40.0)
        }
        Some(f) => {
            let aw: f32 = if focus {
                (rect.width() * 0.3).clamp(56.0, 96.0)
            } else if rect.width() < 170.0 {
                (rect.width() * 0.24).clamp(30.0, 44.0) // nó estreito: arte menor, sobra para o nome
            } else {
                (rect.width() * 0.3).clamp(44.0, 74.0)
            };
            let ar = Rect::from_min_size(rect.min + vec2(12.0, 8.0), vec2(aw, rect.height() - 16.0));
            f(p, ar);
            (aw + 22.0, 0.0)
        }
        None => (12.0, 0.0),
    };
    let wrap = (rect.right() - if stacked { 12.0 } else { 26.0 } - (rect.left() + tx)).max(40.0);
    // as linhas de baixo passam por baixo da bolinha de estado: usam a largura até a borda
    let lwrap = (rect.right() - 12.0 - (rect.left() + tx)).max(40.0);
    // nunca quebra palavra no meio: o título encolhe até 3 pt e, se ainda não couber, perde as
    // últimas palavras ("Dock WKG-L19" → "Dock"); linhas com palavra maior que a largura ficam de fora
    let word_fits = |s: &str, size: f32| s.split(' ').all(|wd| p.layout_no_wrap(wd.to_string(), FontId::proportional(size), pal::fg()).size().x <= wrap);
    let base = if focus { 16.5 } else { 15.0 };
    let (title, tsize) = {
        let mut pick = None;
        let words: Vec<&str> = title.split(' ').collect();
        'outer: for n in (1..=words.len()).rev() {
            let cand = words[..n].join(" ");
            for k in 0..4 {
                let size = base - k as f32;
                if word_fits(&cand, size) {
                    pick = Some((cand, size));
                    break 'outer;
                }
            }
        }
        pick.unwrap_or_else(|| (words[0].to_string(), base - 3.0))
    };
    let tg = p.layout(title, FontId::proportional(tsize), pal::fg(), wrap);
    let reserve = if focus { 24.0 } else { reserve_bottom }; // no dock, a faixa das portas fica embaixo
    let room = rect.height() - 14.0 - reserve - top_pad;
    let mut gs = vec![];
    let mut block = tg.size().y;
    // linha que não cabe inteira e tem " · " vira duas (separa no ponto, não no meio de um par)
    let mut parts: Vec<String> = vec![];
    for l in lines {
        if l.contains(" · ") && p.layout_no_wrap(l.clone(), FontId::proportional(12.5), pal::dim()).size().x > lwrap {
            parts.extend(l.split(" · ").map(str::to_string));
        } else {
            parts.push(l.clone());
        }
    }
    for l in &parts {
        // linha que quase cabe encolhe até 11 pt e fica numa linha só ("Firmware 23.3" é uma
        // palavra só, presa por espaço não separável); senão quebra nos espaços normais
        let one_row = [12.5f32, 12.0, 11.5, 11.0].into_iter().find(|&s| p.layout_no_wrap(l.clone(), FontId::proportional(s), pal::dim()).size().x <= lwrap);
        let size = match one_row {
            Some(s) => s,
            None if l.split(' ').all(|wd| p.layout_no_wrap(wd.to_string(), FontId::proportional(12.5), pal::dim()).size().x <= lwrap) => 12.5,
            None => continue,
        };
        let g = p.layout(l.clone(), FontId::proportional(size), pal::dim(), lwrap);
        if block + 2.0 + g.size().y > room {
            break;
        }
        block += 2.0 + g.size().y;
        gs.push(g);
    }
    // título na mesma linha de base entre nós da mesma fileira: o bloco conta sempre com duas
    // linhas de subtítulo (vazias mantêm a altura; com medidor embaixo, uma); com mais linhas,
    // centraliza o bloco. Em nó empilhado (arte no alto) o texto ancora logo abaixo da arte.
    let th = tg.size().y;
    let avail = rect.height() - top_pad - reserve;
    let lh = p.layout_no_wrap("Ag".into(), FontId::proportional(12.5), pal::dim()).size().y + 2.0;
    let refb = th + lh * if reserve > 0.0 { 1.0 } else { 2.0 };
    let y0 = if stacked { 6.0 } else { ((avail - refb) / 2.0).min((avail - block) / 2.0).max(7.0) };
    let mut y = rect.top() + top_pad + y0;
    let x = rect.left() + tx;
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
            p.add(Shape::line(pts, Stroke::new(1.4, pal::data().gamma_multiply(a))));
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

/// `dock_lvl` é o nível do veredito (0 ok, 1 atenção, 2 problema): o nó do dock usa o mesmo estado do banner.
fn topo_view(ui: &mut Ui, topo: &Topology, flow: &Flow, gpu_static: Option<&crate::collect::GpuStatic>, sel: &mut Option<Sel>, bench: Option<&BenchResult>, psu: Option<(&crate::psu::Psu, crate::psu::Load)>, add_psu: &mut bool, dock_lvl: u8) {
    let w = ui.available_width();
    // Duas geometrias. Larga (≥ 760 pt): note | cabo | dock | ramos empilhados à direita, fonte embaixo do dock.
    // Estreita (painel lateral aberto, janela mínima): note e dock em cima (fonte embaixo do note),
    // os três ramos lado a lado embaixo, cada um com o seu cabo descendo do dock. Nada se sobrepõe.
    let narrow = w < 760.0;
    // sem fonte informada, o lugar dela fica com um nó "fantasma" (convite a informar): a geometria é a mesma
    let has_psu = true;
    let h = if narrow {
        (if has_psu { 170.0 } else { 136.0 }) + 52.0 + 100.0 + 8.0
    } else if has_psu {
        320.0
    } else {
        260.0
    };
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
    let (host, dock, gpu_r, usb_r, tb_r, psu_r) = if narrow {
        let cable = (w * 0.22).clamp(80.0, 150.0);
        let host_w = ((w - 8.0 - cable) * 0.46).floor();
        let dock_w = w - 8.0 - cable - host_w;
        let dock_h = if has_psu { 170.0 } else { 136.0 };
        let dock = Rect::from_min_size(pos2(rect.right() - 4.0 - dock_w, rect.top() + 4.0), vec2(dock_w, dock_h));
        let host = if has_psu {
            Rect::from_min_size(pos2(rect.left() + 4.0, dock.top()), vec2(host_w, 80.0))
        } else {
            Rect::from_center_size(pos2(rect.left() + 4.0 + host_w / 2.0, dock.center().y), vec2(host_w, 80.0))
        };
        let psu_r = Rect::from_min_size(pos2(host.left(), dock.bottom() - 78.0), vec2(host_w, 78.0));
        let bw = ((w - 8.0 - 2.0 * 10.0) / 3.0).floor();
        let by = dock.bottom() + 52.0;
        let col = |i: f32| Rect::from_min_size(pos2(rect.left() + 4.0 + i * (bw + 10.0), by), vec2(bw, 100.0));
        (host, dock, col(0.0), col(1.0), col(2.0), psu_r)
    } else {
        let host_w = (w * 0.18).clamp(200.0, 240.0);
        let bw = (w * 0.24).clamp(200.0, 300.0);
        let cy = rect.top() + 130.0;
        let host = Rect::from_min_size(pos2(rect.left() + 4.0, cy - 40.0), vec2(host_w, 80.0));
        let bx = rect.right() - bw - 4.0;
        // o dock fica centrado no vão entre o note e os ramos, deixando espaço para os cabos
        let gap = bx - host.right();
        let dock_w = (gap * 0.52).clamp(190.0, 320.0).min(gap - 120.0);
        let dock = Rect::from_center_size(pos2(host.right() + gap * 0.5, cy), vec2(dock_w, 136.0));
        // fonte embaixo do dock, sem passar do vão antes dos cabos dos ramos
        let mx = (dock.right() + bx) / 2.0;
        let pw = dock.width().min(300.0).min(2.0 * (mx - 12.0 - dock.center().x));
        let psu_r = Rect::from_center_size(pos2(dock.center().x, dock.bottom() + 40.0 + 39.0), vec2(pw, 78.0));
        (
            host,
            dock,
            Rect::from_min_size(pos2(bx, rect.top() + 2.0), vec2(bw, 78.0)),
            Rect::from_min_size(pos2(bx, rect.top() + 91.0), vec2(bw, 78.0)),
            Rect::from_min_size(pos2(bx, rect.top() + 180.0), vec2(bw, 78.0)),
            psu_r,
        )
    };

    // --- cabo TB (2 faixas) + PD voltando para o note
    let lanes = topo.dock_usb4.iter().find(|p| p.pid == 21).and_then(|p| p.val.as_u32()).unwrap_or(2).max(1);
    let a = pos2(host.right(), host.center().y);
    let b = pos2(dock.left(), a.y); // no estreito com fonte o note fica no alto do dock
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
        Some(0x8) => tf!("Cabo a {}\u{A0}Gb/s · o dock aceita 40", "Cable at {}\u{A0}Gb/s · the dock supports 40", lanes * 10),
        Some(0x4) => tf!("Cabo a {}\u{A0}Gb/s", "Cable at {}\u{A0}Gb/s", lanes * 20),
        Some(x) => tf!("Cabo conectado (código {x})", "Cable connected (code {x})"),
        None if dock_ok => crate::i18n::t("Cabo conectado", "Cable connected").into(),
        None => crate::i18n::t("Sem conexão", "No connection").into(),
    };
    // rótulos do cabo: usa a versão mais longa que cabe no vão entre o note e o dock (nunca fica escondida atrás deles)
    let room = b.x - a.x - 10.0;
    let fit = |cands: &[String], size: f32| -> Option<String> { cands.iter().find(|c| p.layout_no_wrap(c.to_string(), FontId::proportional(size), pal::fg()).size().x <= room).cloned() };
    let short = cable_txt.split(" · ").next().unwrap_or_default().to_string();
    let tiny = short.trim_start_matches(crate::i18n::t("Cabo a ", "Cable at ")).to_string();
    if let Some(s) = fit(&[cable_txt.clone(), short, tiny], 13.5) {
        p.text(mid + vec2(0.0, -30.0), egui::Align2::CENTER_BOTTOM, s, FontId::proportional(13.5), if !dock_ok { pal::red() } else if speed == Some(0x8) { pal::amber() } else { pal::bolt() });
    }
    let pd_c: Vec<String> = if flow.pd_on { vec![crate::i18n::t("Carregando o notebook · até 85\u{A0}W", "Charging the laptop · up to 85\u{A0}W").into(), crate::i18n::t("Carregando", "Charging").into(), "⚡".into()] } else { vec![crate::i18n::t("O dock não está carregando o notebook", "The dock isn't charging the laptop").into(), crate::i18n::t("Sem carga", "Not charging").into()] };
    if let Some(s) = fit(&pd_c, 12.5) {
        p.text(mid + vec2(0.0, 34.0), egui::Align2::CENTER_TOP, s, FontId::proportional(12.5), if flow.pd_on { pal::bolt().gamma_multiply(0.8) } else { pal::amber() });
    }
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
        if narrow {
            // desce do pé do dock, anda na horizontal (um nível por ramo: os cabos não se cruzam) e desce no nó
            let s = pos2(dock.center().x + (i - 1.0) * 26.0, dock.bottom());
            let e = pos2(target.center().x, target.top());
            let my = dock.bottom() + 14.0 + i * 12.0;
            return vec![s, pos2(s.x, my), pos2(e.x, my), e];
        }
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

    // --- fonte do dock: ramo de energia chegando ao dock (tracejado e apagado enquanto a fonte não é informada)
    {
        let pts = if narrow {
            // a fonte fica embaixo do note: o cabo entra pela esquerda do dock, na altura dela
            let y = psu_r.center().y;
            vec![pos2(psu_r.right(), y), pos2(dock.left(), y)]
        } else {
            // entra no dock à direita das bolinhas das portas
            let lx = (dock.left() + 128.0).max(dock.center().x).min(dock.right() - 24.0);
            if (lx - psu_r.center().x).abs() < 1.0 {
                vec![pos2(lx, psu_r.top()), pos2(lx, dock.bottom())]
            } else {
                vec![pos2(psu_r.center().x, psu_r.top()), pos2(psu_r.center().x, psu_r.top() - 14.0), pos2(lx, psu_r.top() - 14.0), pos2(lx, dock.bottom())]
            }
        };
        if let Some((_, l)) = &psu {
            let f = l.frac.clamp(0.0, 1.0);
            p.add(egui::Shape::line(pts.clone(), Stroke::new(8.0 + 6.0 * f, pal::bolt().gamma_multiply(0.08))));
            link(&p, &pts, true, 2.0 + 4.0 * f, pal::bolt());
            pulses(&p, &pts, t, f.max(0.08), pal::bolt(), false);
        } else {
            p.extend(egui::Shape::dashed_line(&pts, Stroke::new(1.5, pal::faint()), 5.0, 5.0));
        }
    }

    // --- nós
    let hostn = topo.host_router.as_ref();
    let host_lines = vec!["Thunderbolt\u{A0}4".to_string(), format!("Firmware\u{A0}{}", hostn.and_then(|n| n.firmware.clone()).unwrap_or("?".into()))];
    let (hr, lift) = hover(ui, host, ui.id().with("host"));
    node_box(&p, host, crate::i18n::t("Notebook", "Laptop"), &host_lines, pal::pcie(), if hostn.is_some() { pal::green() } else { pal::amber() }, *sel == Some(Sel::Host), false, lift, t, Some(&|p: &egui::Painter, r: Rect| art::laptop(p, r, hostn.is_some(), t)));
    if hr.clicked() {
        *sel = Some(Sel::Host);
    }

    let dr = topo.dock_router.as_ref();
    let fw = dr.and_then(|n| n.firmware.clone()).unwrap_or("—".into());
    let model = topo.dock_usb4.iter().find(|p| p.pid == 10).map(|p| p.val.show()).unwrap_or("—".into());
    let mut dock_lines = vec![format!("Thunderbolt\u{A0}3 · firmware\u{A0}{fw}")];
    let _ = &model;
    match dr.and_then(|n| n.last_arrival) {
        Some(ts) => dock_lines.push(tf!("Conectado há {}", "Connected for {}", fmt_dur(now_ms() - ts))),
        None => dock_lines.push(crate::i18n::t("Não encontrado", "Not found").into()),
    }
    if let Some(br) = bench {
        // notebook → placa: o sentido que pesa em jogo (o mesmo do medidor "Banda" da visão geral)
        dock_lines.push(tf!("Banda medida: {}\u{A0}Gb/s", "Bandwidth: {}\u{A0}Gb/s", gbit(br.h2d_gbs)));
    }
    let (dkr, lift) = hover(ui, dock, ui.id().with("dock"));
    let dock_state = if !(dock_ok && switch_ok) || dock_lvl >= 2 { pal::red() } else if dock_lvl == 1 { pal::amber() } else { pal::green() };
    node_box(&p, dock, "Dock WKG-L19", &dock_lines, pal::pcie(), dock_state, *sel == Some(Sel::Dock), true, lift, t, Some(&|p: &egui::Painter, r: Rect| art::dock(p, r.with_max_y(r.bottom() - 26.0), dock_ok, t)));
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
        ui.interact(hit, ui.id().with(("dport", i)), Sense::hover()).on_hover_text(tf!("porta downstream {} · {}", "downstream port {} · {}", i + 1, dname(&dp.node)));
    }
    if dkr.clicked() {
        *sel = Some(Sel::Dock);
    }

    // GPU
    let gl = if let Some(g) = &topo.gpu {
        let gm = flow.gpu.unwrap_or_default();
        let _ = g;
        vec![
            format!("{:.0}\u{A0}°C · {:.0}\u{A0}W", gm.temp.unwrap_or(0.0), gm.tbp.unwrap_or(0.0)),
            match gpu_static {
                // ADLX: o enlace da placa com o slot do dock, que segue pelo túnel até o notebook
                Some(s) if s.lanes > 0 => tf!("Túnel {}\u{A0}x{}", "Tunnel {}\u{A0}x{}", s.bus.replace(' ', "\u{A0}"), s.lanes),
                Some(_) => String::new(),
                None => "…".into(),
            },
        ]
        .into_iter()
        .filter(|l| !l.is_empty())
        .collect()
    } else {
        vec![crate::i18n::t("Não encontrada", "Not found").into()]
    };
    let gm = flow.gpu.unwrap_or_default();
    let spin = gm.fan.map(|f| f as f32 / 3200.0).unwrap_or(gm.usage.unwrap_or(0.0) as f32 / 100.0);
    let (gr, lift) = hover(ui, gpu_r, ui.id().with("gpu"));
    node_box(&p, gpu_r, if narrow { "RX 7600 XT" } else { "Radeon RX 7600 XT" }, &gl, pal::data(), if gpu_ok { pal::green() } else { pal::red() }, *sel == Some(Sel::Gpu), false, lift, t, Some(&|p: &egui::Painter, r: Rect| art::gpu(p, r, spin, gpu_ok, t)));
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
                    l.push(crate::i18n::t("Livre", "Free").into());
                } else {
                    for d in topo.usb_a.iter().take(2) {
                        l.push(d.name.clone());
                    }
                }
                ("USB-A 10\u{A0}Gb/s", l, pal::pcie())
            }
            _ => (
                "TB extra 15\u{A0}W",
                vec![if dp.child.is_some() { crate::i18n::t("Em uso", "In use").into() } else { crate::i18n::t("Livre", "Free").into() }],
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
        node_box(&p, usb_r, "USB-A 10\u{A0}Gb/s", &[crate::i18n::t("não detectada", "not detected").into()], pal::pcie(), pal::red(), false, false, 0.0, t, Some(&|p: &egui::Painter, r: Rect| art::usb_a(p, r, false)));
    }
    if ports.iter().all(|(d, _)| d.role != PortRole::TbExtra) {
        node_box(&p, tb_r, "TB extra 15\u{A0}W", &[crate::i18n::t("não detectada", "not detected").into()], pal::faint(), pal::amber(), false, false, 0.0, t, Some(&|p: &egui::Painter, r: Rect| art::usb_c(p, r, false)));
    }

    // Fonte do dock (ou o nó fantasma "+ Informar a fonte")
    let (psr, lift) = hover(ui, psu_r, ui.id().with("psu"));
    if let Some((ps, l)) = &psu {
        let w = ps.watts.max(1) as f32;
        let f = l.frac.clamp(0.0, 1.0);
        let wf = l.worst / w;
        let st = psu_state(l, ps.watts);
        // compacto (nó estreito): o número vem antes do nome — modelo curto ("SF750") e "≈ 61 / 750 W"
        let compact = psu_r.width() < 210.0;
        let aw = if psu_r.width() < 170.0 { (psu_r.width() * 0.24).clamp(30.0, 44.0) } else { (psu_r.width() * 0.3).clamp(44.0, 74.0) }; // mesma regra do node_box
        let room = psu_r.width() - aw - 22.0 - 26.0;
        let model = ps.model.trim();
        let title = if model.is_empty() {
            crate::i18n::t("Fonte do dock", "Dock PSU")
        } else if !compact && p.layout_no_wrap(model.to_string(), FontId::proportional(13.0), pal::fg()).size().x <= room {
            model
        } else {
            short_model(model)
        };
        let lines = [if compact { format!("≈ {:.0} / {}\u{A0}W", l.now, ps.watts) } else { tf!("≈ {:.0}\u{A0}W de {}\u{A0}W", "≈ {:.0}\u{A0}W of {}\u{A0}W", l.now, ps.watts) }];
        node_box_r(&p, psu_r, title, &lines, pal::bolt(), st.col, *sel == Some(Sel::Psu), false, lift, t, Some(&|p: &egui::Painter, r: Rect| art::psu(p, r, true, t, &ps.rating, f)), 16.0);
        // mini medidor: ▮ carga agora · ┆ uso pesado · ▼ picos rápidos (o ▼ some no compacto)
        let x0 = psu_r.left() + aw + 22.0;
        let pct = format!("≈ {:.0}\u{A0}%", f * 100.0);
        let pw = p.layout_no_wrap(pct.clone(), FontId::proportional(12.5), pal::fg()).size().x;
        let track = Rect::from_min_max(pos2(x0, psu_r.bottom() - 15.0), pos2((psu_r.right() - pw - 18.0).max(x0 + 16.0), psu_r.bottom() - 9.0));
        let fc = load_col(f);
        // trilho com contraste nos dois temas (no claro o `well` some sobre o nó)
        let (tf, ts) = if pal::current().is_light() { (pal::grid(), pal::faint().gamma_multiply(0.7)) } else { (pal::well(), pal::border()) };
        p.rect_filled(track, CornerRadius::same(3), tf);
        p.rect_stroke(track, CornerRadius::same(3), Stroke::new(1.0, ts), StrokeKind::Outside);
        if f > 0.004 {
            p.rect_filled(Rect::from_min_max(track.min, pos2(track.left() + track.width() * f, track.bottom())), CornerRadius::same(3), fc);
        }
        let wx = track.left() + track.width() * wf.clamp(0.0, 1.0);
        let wc = if wf > 0.85 { pal::amber() } else { pal::dim() };
        p.extend(egui::Shape::dashed_line(&[pos2(wx, track.top() - 3.0), pos2(wx, track.bottom() + 3.0)], Stroke::new(1.5, wc), 2.0, 2.0));
        if !compact {
            let kx = (track.left() + track.width() * (l.spike / w).clamp(0.0, 1.0)).min(track.right() - 3.0);
            let kc = if l.spike > w { pal::amber() } else { pal::dim() };
            p.text(pos2(kx, track.top() - 1.0), egui::Align2::CENTER_BOTTOM, "▼", FontId::proportional(9.0), kc);
        }
        p.text(pos2(psu_r.right() - 10.0, track.center().y), egui::Align2::RIGHT_CENTER, pct, FontId::proportional(12.5), fc);
        let r = psr.on_hover_text(tf!(
            "{}\nEstimativa: placa (medida) + notebook + dock. A fonte não manda dados ao PC.\n▮ agora ≈ {:.0}\u{A0}W · ┆ uso pesado ≈ {:.0}\u{A0}W · ▼ picos rápidos ≈ {:.0}\u{A0}W",
            "{}\nEstimate: GPU (measured) + laptop + dock. The PSU doesn't send data to the PC.\n▮ now ≈ {:.0}\u{A0}W · ┆ heavy use ≈ {:.0}\u{A0}W · ▼ quick spikes ≈ {:.0}\u{A0}W",
            st.pill, l.now, l.worst, l.spike
        ));
        if r.clicked() {
            *sel = Some(Sel::Psu);
        }
    } else {
        // nó fantasma: borda tracejada, ilustração esmaecida e o convite; leva à aba Cabo e energia
        let cr = CornerRadius::same(pal::radius());
        p.rect_filled(psu_r, cr, pal::card2().gamma_multiply(0.25 + 0.5 * lift));
        let e = psu_r.shrink(0.5);
        let bc = pal::faint().lerp_to_gamma(pal::fg(), 0.3 * lift);
        p.extend(egui::Shape::dashed_line(&[e.left_top(), e.right_top(), e.right_bottom(), e.left_bottom(), e.left_top()], Stroke::new(1.0, bc), 6.0, 4.0));
        let aw = if psu_r.width() < 170.0 { (psu_r.width() * 0.24).clamp(30.0, 44.0) } else { (psu_r.width() * 0.3).clamp(44.0, 74.0) };
        let ar = Rect::from_min_size(psu_r.min + vec2(12.0, 8.0), vec2(aw, psu_r.height() - 16.0));
        let mut faded = p.clone();
        faded.multiply_opacity(0.3 + 0.2 * lift);
        art::psu(&faded, ar, false, t, "", 0.0);
        let tx = psu_r.left() + aw + 22.0;
        let wrap = (psu_r.right() - 12.0 - tx).max(40.0);
        // não cabendo numa linha, quebra em "+ Informar / a fonte" (nunca deixa "fonte" sozinha)
        let full_t1 = crate::i18n::t("+ Informar a fonte", "+ Add your PSU");
        let t1 = if p.layout_no_wrap(full_t1.into(), FontId::proportional(15.0), pal::dim()).size().x <= wrap { full_t1 } else { crate::i18n::t("+ Informar\na fonte", "+ Add\nyour PSU") };
        let g1 = p.layout(t1.into(), FontId::proportional(15.0), pal::dim().lerp_to_gamma(pal::fg(), lift), wrap);
        // a 2ª linha usa a versão que cabe numa linha só (nunca sobra uma palavra sozinha embaixo)
        let sub = [crate::i18n::t("Para ver a carga ao vivo", "To see the live load"), crate::i18n::t("Carga ao vivo", "Live load")].into_iter().find(|c| p.layout_no_wrap(c.to_string(), FontId::proportional(12.5), pal::faint()).size().x <= wrap);
        let g2 = p.layout_no_wrap(sub.unwrap_or_default().to_string(), FontId::proportional(12.5), pal::faint());
        let two = sub.is_some() && g1.size().y + 4.0 + g2.size().y <= psu_r.height() - 16.0;
        let hgt = if two { g1.size().y + 4.0 + g2.size().y } else { g1.size().y };
        let y0 = psu_r.center().y - hgt / 2.0;
        let g1h = g1.size().y;
        p.galley(pos2(tx, y0), g1, pal::dim());
        if two {
            p.galley(pos2(tx, y0 + g1h + 4.0), g2, pal::faint());
        }
        if psr.on_hover_text(crate::i18n::t("Informe a fonte do dock (aba Cabo e energia) para ver a carga estimada, o uso pesado e os picos rápidos.", "Tell the app which PSU your dock uses (Cable & power tab) to see the estimated load, heavy use and quick spikes.")).clicked() {
            *add_psu = true;
        }
    }
}

// ---------------------------------------------------------------------------

impl eframe::App for App {
    fn ui(&mut self, root: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        let cfg = crate::config::get();
        if (ctx.zoom_factor() - cfg.zoom).abs() > 0.001 {
            ctx.set_zoom_factor(cfg.zoom);
        }
        self.refresh_history();
        let (topo, samples, gpu_static, notes) = collect::with_live(|l| (l.topo.clone(), l.samples.iter().copied().collect::<Vec<Sample>>(), l.gpu_static.clone(), l.notes.clone()))
            .unwrap_or_default();
        let topo = topo.unwrap_or_default();
        self.psu_frame(&topo, &samples);
        if topo.read_ms != 0 {
            if let Some(v) = vendor_of(&topo) {
                self.vendor = Some(v);
            }
        }
        let th = resolve_theme(cfg.theme, self.vendor);
        if th != pal::current() {
            apply_theme(&ctx, th);
        }
        // barra de título nativa na cor do tema (só age quando o tema, a janela ou o tema do sistema muda)
        crate::titlebar::sync(&ctx, frame, th as u8, caption_rgb(th));
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
                    ui.label(egui::RichText::new(format!("{:.0}\u{A0}MB · {}%", last.ws_mb, dec(last.cpu_pct as f64, 1))).color(pal::faint()).size(12.5))
                        .on_hover_text(crate::i18n::t("Consumo do próprio painel (RAM e CPU). Fechando a janela, ele volta para a bandeja e para de coletar.", "The panel's own usage (RAM and CPU). Closing the window sends it back to the tray and it stops collecting."));
                    ui.add_space(pal::gap());
                    // pílula curta quando o cabeçalho aperta (sem quebrar a linha)
                    let compact = ui.available_width() < 560.0;
                    self.header_status(ui, &topo, t, compact);
                });
            });
        });

        }
        // ---------------- rodapé: versão, licença, feedback e apoio (largura toda, em todas as abas)
        if egui::Panel::bottom("rodape")
            .resizable(false)
            .exact_size(30.0)
            .frame(egui::Frame::new().fill(pal::well()).inner_margin(egui::Margin::symmetric(16, 0)))
            .show_inside(root, |ui| footer(ui))
            .inner
        {
            self.fb = Some(FbPanel::default());
        }
        // ---------------- navegação: abas no topo (claro/escuro), barra AMD ou menu lateral (NVIDIA/Intel)
        if th == Th::Amd {
        } else if th.nav_left() {
            // estreito (< 1150 pt): menu só com ícones (dica com o nome), sobra largura para o conteúdo
            let compact = root.available_width() < 1150.0;
            egui::Panel::left("nav")
                .resizable(false)
                .exact_size(if compact { 64.0 } else { 210.0 })
                .frame(egui::Frame::new().fill(pal::well()).inner_margin(egui::Margin::symmetric(10, 12)))
                .show_inside(root, |ui| self.nav_left(ui, compact));
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
            let pr = egui::Panel::right("detalhes")
                .resizable(true)
                .default_size(430.0)
                .min_size(360.0)
                .max_size(560.0)
                .frame(egui::Frame::new().fill(pal::card()).inner_margin(egui::Margin::same(10)))
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
            // o painel "flutua" sobre a página: sombra suave à esquerda, filete de 1 px e realce no topo
            let pr = pr.response.rect;
            let lp = ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("sombra-painel")));
            let (sr, sg, sb) = pal::shade_base();
            let sa = match th {
                Th::Light => 26,
                Th::Intel => 120,
                _ => 110,
            };
            let dark = Color32::from_rgba_unmultiplied(sr, sg, sb, sa);
            let sh = Rect::from_min_max(pos2(pr.left() - 18.0, pr.top()), pos2(pr.left(), pr.bottom()));
            let mut m = egui::Mesh::default();
            m.colored_vertex(sh.left_top(), Color32::TRANSPARENT);
            m.colored_vertex(sh.right_top(), dark);
            m.colored_vertex(sh.right_bottom(), dark);
            m.colored_vertex(sh.left_bottom(), Color32::TRANSPARENT);
            m.add_triangle(0, 1, 2);
            m.add_triangle(0, 2, 3);
            lp.add(egui::Shape::mesh(m));
            lp.vline(pr.left() + 0.5, pr.y_range(), Stroke::new(1.0, pal::border()));
            lp.hline(pr.x_range(), pr.top() + 0.5, Stroke::new(1.0, pal::hi()));
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
                        let dock_lvl = self.verdict(&topo).0;
                        card(ui, crate::i18n::t("🔗  Topologia", "🔗  Topology"), pal::cyan(), |ui| {
                            let mut sel = self.sel;
                            let pl = self.psu_load.map(|l| (&self.psu, l));
                            let mut add_psu = false;
                            topo_view(ui, &topo, &flow, gpu_static.as_ref(), &mut sel, bench_done.as_ref(), pl, &mut add_psu, dock_lvl);
                            if add_psu {
                                self.tab = Tab::Enlace;
                            }
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
                        // 3 colunas no largo; no médio, Enlace | Estabilidade (alturas parecidas) e a
                        // Energia na largura toda embaixo; 1 coluna no estreito. Em todas, as bases
                        // dos cartões lado a lado alinham (sem vão embaixo do mais baixo)
                        let aw = ui.available_width();
                        let cols = if aw > 1150.0 { 3 } else if aw >= 720.0 { 2 } else { 1 };
                        ui.spacing_mut().item_spacing.x = pal::gap();
                        match cols {
                            3 => eq_cols(ui, "enlace3", 3, |i, ui| match i {
                                0 => self.card_link(ui, &topo, bench_done.as_ref()),
                                1 => self.card_power(ui, &samples),
                                _ => self.card_stability(ui, &topo),
                            }),
                            2 => {
                                eq_cols(ui, "enlace2", 2, |i, ui| match i {
                                    0 => self.card_link(ui, &topo, bench_done.as_ref()),
                                    _ => self.card_stability(ui, &topo),
                                });
                                ui.add_space(pal::gap());
                                self.card_power(ui, &samples);
                            }
                            _ => {
                                self.card_link(ui, &topo, bench_done.as_ref());
                                ui.add_space(pal::gap());
                                self.card_power(ui, &samples);
                                ui.add_space(pal::gap());
                                self.card_stability(ui, &topo);
                            }
                        }
                        ui.add_space(pal::gap());
                        self.card_psu(ui);
                    }
                    Tab::Fluxo => {
                        ui.spacing_mut().item_spacing.x = pal::gap();
                        let cols = if ui.available_width() >= 900.0 { 2 } else { 1 };
                        eq_cols(ui, "fluxo", cols, |i, ui| {
                            if i == 0 {
                                self.card_flow(ui, &samples, &bench, &ctx);
                            }
                            if cols == 1 {
                                ui.add_space(pal::gap());
                            }
                            if i == cols - 1 {
                                self.card_gpu(ui, &samples, gpu_static.as_ref());
                            }
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

        self.feedback_panel(&ctx, &topo, &samples, gpu_static.as_ref());

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
        // estreito (< 1150 pt ou rótulos que não cabem): só o ícone, com o nome na dica
        let need: f32 = Tab::ALL.iter().map(|t| ui.painter().layout_no_wrap(format!("{}  {}", t.icon(), t.label()), FontId::proportional(13.5), pal::fg()).size().x + 22.0).sum();
        let compact = ui.available_width() < 1150.0 && need > ui.available_width();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = if compact { 26.0 } else { 22.0 };
            for tab in Tab::ALL {
                let on = self.tab == tab;
                let txt = if th == Th::Amd { tab.label().to_uppercase() } else { tab.label().to_string() };
                let shown = if compact { tab.icon().to_string() } else { format!("{}  {}", tab.icon(), txt) };
                let r = ui.add(egui::Label::new(egui::RichText::new(shown).size(if compact { 17.0 } else { 13.5 }).strong().color(if on { pal::fg() } else { pal::dim() })).extend().sense(Sense::click()));
                let r = if compact { r.on_hover_text(tab.label()) } else { r };
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
        // estreito (< 1150 pt ou se os rótulos não couberem): abas só com ícone + dica, pílula curta
        let tabs_w: f32 = Tab::ALL.iter().map(|t| ui.painter().layout_no_wrap(t.label().to_string(), FontId::proportional(19.0), Color32::WHITE).size().x + 40.0).sum();
        let compact = full.width() < 1150.0 || 72.0 + tabs_w + 420.0 > full.width();
        for tab in Tab::ALL {
            let on = self.tab == tab;
            let g = if compact {
                ui.painter().layout_no_wrap(tab.icon().to_string(), FontId::proportional(21.0), Color32::WHITE)
            } else {
                ui.painter().layout_no_wrap(tab.label().to_string(), FontId::proportional(19.0), Color32::WHITE)
            };
            let pad = if compact { 36.0 } else { 40.0 };
            let r = Rect::from_min_size(pos2(x, full.top()), vec2(g.size().x + pad, full.height()));
            let resp = ui.interact(r, ui.id().with(("amdtab", tab as u8)), Sense::click());
            let resp = if compact { resp.on_hover_text(tab.label()) } else { resp };
            let h = ui.ctx().animate_bool_with_time(resp.id, on || resp.hovered(), 0.15);
            if h > 0.0 {
                p.rect_filled(r, CornerRadius::ZERO, Color32::from_rgb(46, 46, 49).gamma_multiply(h));
            }
            let col = if on { Color32::WHITE } else if resp.hovered() { Color32::from_gray(225) } else { Color32::from_gray(175) };
            p.galley_with_override_text_color(pos2(r.left() + pad / 2.0, r.center().y - g.size().y / 2.0), g.clone(), col);
            if on {
                active = Some(Rect::from_min_max(pos2(r.left() + pad / 2.0, r.bottom() - 6.0), pos2(r.right() - pad / 2.0, r.bottom() - 3.0)));
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
            ui.label(egui::RichText::new(format!("{:.0}\u{A0}MB · {}%", last.ws_mb, dec(last.cpu_pct as f64, 1))).color(Color32::from_gray(130)).size(12.5))
                .on_hover_text(crate::i18n::t("Consumo do próprio painel (RAM e CPU).", "The panel's own usage (RAM and CPU)."));
            ui.add_space(12.0);
            self.header_status(ui, &topo, t, compact);
        });
    }

    /// Fonte do dock: modelo digitado, specs buscadas na web, carga estimada em tempo real.
    /// Recolhe o resultado da busca de specs (segundo plano) e calcula a carga da fonte do quadro.
    fn psu_frame(&mut self, topo: &Topology, samples: &[Sample]) {
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
                            got.push(format!("{w}\u{A0}W"));
                        }
                        if let Some(a) = f.rail12_a {
                            self.psu.rail12_a = a;
                            got.push(format!("+12\u{A0}V {a:.0}\u{A0}A"));
                        }
                        if let Some(r) = &f.rating {
                            self.psu.rating = r.clone();
                            got.push(format!("80 PLUS {r}"));
                        }
                        self.psu.origin = "web".into();
                        psu::save(&self.psu);
                        self.psu_msg = Some((true, tf!("Achei no {}: {}. Confira na etiqueta da fonte e corrija se precisar.", "Found it on {}: {}. Check it against the PSU label and correct it if needed.", f.engine, got.join(" · "))));
                    }
                    Err(e) => self.psu_msg = Some((false, tf!("Não achei as specs ({e}). Preencha à mão, a etiqueta da fonte tem tudo.", "Couldn't find the specs ({e}). Fill them in by hand, the PSU label has it all."))),
                }
            }
        }
        self.psu_load = (self.psu.watts > 0).then(|| psu::compute(&self.psu, &psu_inputs(topo, samples)));
    }

    fn card_psu(&mut self, ui: &mut Ui) {
        use crate::psu;
        let before = self.psu.clone();
        let load = self.psu_load;
        card(ui, crate::i18n::t("🔌  Fonte do dock", "🔌  Dock PSU"), pal::bolt(), |ui| {
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
                let txt = if busy { crate::i18n::t("      Buscando…", "      Searching…") } else { crate::i18n::t("🔍  Buscar specs", "🔍  Look up specs") };
                let b = ui.add_enabled(can, egui::Button::new(egui::RichText::new(txt).size(14.0)).min_size(vec2(w, 32.0)));
                if busy {
                    let c = b.rect.center() - vec2(44.0, 0.0);
                    egui::Spinner::new().size(16.0).paint_at(ui, Rect::from_center_size(c, vec2(16.0, 16.0)));
                }
                b.clicked()
            };
            ui.allocate_ui_with_layout(vec2(avail, 54.0), egui::Layout::left_to_right(egui::Align::Max), |ui| {
                field(ui, model_w, "🏷", crate::i18n::t("Modelo", "Model"), |ui| {
                    let te = ui.add(
                        egui::TextEdit::singleline(&mut self.psu.model)
                            .hint_text(crate::i18n::t("ex.: Corsair RM650x, MSI MAG A650BN", "e.g. Corsair RM650x, MSI MAG A650BN"))
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
            // ── linha 2: ficha da fonte, uma célula por coluna. 0 = falta informar (dica esmaecida, nunca "0 W")
            let psu_v = &mut self.psu;
            let mut cell = |ui: &mut Ui, i: usize| match i {
                0 => field(ui, cw, "⚡", crate::i18n::t("Potência", "Power"), |ui| {
                    if let Some(v) = num_field(ui, "psu-w", cw, psu_v.watts as f32, 0, "W", "— W") {
                        psu_v.watts = v.round().clamp(0.0, 2000.0) as u32;
                    }
                }),
                1 => field(ui, cw, "⎓", crate::i18n::t("Linha +12\u{A0}V", "+12\u{A0}V rail"), |ui| {
                    if let Some(v) = num_field(ui, "psu-a", cw, psu_v.rail12_a, 1, "A", crate::i18n::t("ex.: 62,5\u{A0}A", "e.g. 62.5\u{A0}A")) {
                        psu_v.rail12_a = (v * 10.0).round().clamp(0.0, 1700.0) / 10.0;
                    }
                }),
                2 => field(ui, cw, "★", crate::i18n::t("Selo 80 PLUS", "80 PLUS rating"), |ui| {
                    // largura exata da célula
                    egui::ComboBox::from_id_salt("psu-rating")
                        .width(cw)
                        .height(320.0)
                        .selected_text(if psu_v.rating.is_empty() { crate::i18n::t("sem selo / não sei", "no rating / not sure") } else { psu_v.rating.as_str() })
                        .show_ui(ui, |ui| {
                            for r in psu::RATINGS {
                                ui.selectable_value(&mut psu_v.rating, r.to_string(), if r.is_empty() { crate::i18n::t("sem selo / não sei", "no rating / not sure") } else { r });
                            }
                        });
                }),
                _ => field(ui, cw, "ⓘ", crate::i18n::t("Origem", "Source"), |ui| {
                    // selo (não parece campo editável), na altura dos campos
                    let (r, _) = ui.allocate_exact_size(vec2(cw, 32.0), Sense::hover());
                    let (t, c) = if psu_v.origin == "web" {
                        (crate::i18n::t("🌐  Da internet", "🌐  From the web"), pal::data())
                    } else if psu_v.watts == 0 && psu_v.model.trim().is_empty() {
                        (crate::i18n::t("…  Ainda não informada", "…  Not set yet"), pal::faint())
                    } else {
                        (crate::i18n::t("✍  Preenchida à mão", "✍  Filled in by hand"), pal::dim())
                    };
                    let g = ui.painter().layout_no_wrap(t.to_string(), FontId::proportional(13.5), c);
                    let chip = Rect::from_min_size(r.min, vec2((g.size().x + 28.0).min(cw), 32.0));
                    ui.painter().rect_filled(chip, CornerRadius::same(16), c.gamma_multiply(0.14));
                    ui.painter().rect_stroke(chip, CornerRadius::same(16), Stroke::new(1.0, c.gamma_multiply(0.45)), StrokeKind::Inside);
                    ui.painter().galley(pos2(chip.left() + 14.0, chip.center().y - g.size().y / 2.0), g, c);
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
            // os números do quadro (App::psu_load): os mesmos do nó da topologia e do painel lateral
            let Some(l) = load.filter(|_| before.watts > 0 && self.psu.watts > 0) else {
                empty_psu(ui, avail);
                return;
            };
            let w = before.watts as f32;
            // ── conclusão primeiro
            let st = psu_state(&l, before.watts);
            verdict_strip(ui, st.col, st.icon, &st.msg);
            ui.add_space(6.0);
            load_bar(ui, &l, w, avail);
            ui.add_space(10.0);
            // ── medidores: o centro de cada um cai no centro da coluna dos campos acima
            let rail = before.rail12_a;
            let rating = if before.rating.is_empty() { crate::i18n::t("sem selo", "no rating").to_string() } else { before.rating.clone() };
            let gauge_at = |ui: &mut Ui, i: usize| {
                ui.vertical_centered(|ui| match i {
                    0 => {
                        gauge(ui, "⚡", l.now, w, &format!("W · {:.0}%", l.frac * 100.0), crate::i18n::t("Carga agora (≈)", "Load now (≈)"), load_col(l.frac)).on_hover_text(tf!(
                            "≈ {:.0}\u{A0}W = placa {:.0}\u{A0}W (medido) + notebook pelo PD {:.0}\u{A0}W + dock {:.0}\u{A0}W + portas {:.0}\u{A0}W (estimados)",
                            "≈ {:.0}\u{A0}W = GPU {:.0}\u{A0}W (measured) + laptop via PD {:.0}\u{A0}W + dock {:.0}\u{A0}W + ports {:.0}\u{A0}W (estimated)",
                            l.now, l.gpu, l.pd, l.dock, l.usb
                        ));
                    }
                    1 => {
                        gauge(ui, "▼", l.spike, w, "W", crate::i18n::t("Picos rápidos (≈)", "Quick spikes (≈)"), if l.spike > w { pal::amber() } else { pal::green() }).on_hover_text(tf!(
                            "Transientes de milissegundos: a placa pode puxar ~1,6× o limite ({:.0}\u{A0}W), somados ao resto. Fonte fraca desarma nesses picos.",
                            "Millisecond transients: the GPU can draw ~1.6× its limit ({:.0}\u{A0}W), on top of everything else. A weak PSU trips on these spikes.",
                            psu::GPU_TBP_MAX
                        ));
                    }
                    2 => {
                        gauge(ui, "🔌", l.wall, w, "W", crate::i18n::t("Na tomada (≈)", "At the wall (≈)"), pal::data()).on_hover_text(tf!("Eficiência estimada {:.0}% ({rating})", "Estimated efficiency {:.0}% ({rating})", l.eff * 100.0));
                    }
                    _ => {
                        if rail > 0.0 {
                            let gw = gauge_nat(ui, "⎓", crate::i18n::t("Linha +12\u{A0}V (≈)", "+12\u{A0}V rail (≈)"));
                            gauge_wd(ui, gw, "⎓", l.amps12, rail, "A", crate::i18n::t("Linha +12\u{A0}V (≈)", "+12\u{A0}V rail (≈)"), load_col(l.amps12 / rail), 1).on_hover_text(crate::i18n::t("Corrente estimada na linha de +12\u{A0}V, contra o máximo da etiqueta da fonte.", "Estimated current on the +12\u{A0}V rail, against the maximum on the PSU label."));
                        } else {
                            gauge(ui, "⎓", f32::NAN, 1.0, "A", crate::i18n::t("+12\u{A0}V: falta informar", "+12\u{A0}V: not set"), pal::faint()).on_hover_text(crate::i18n::t("Preencha a corrente da linha +12\u{A0}V (está na etiqueta da fonte) para ver este medidor.", "Enter the +12\u{A0}V rail current (it's on the PSU label) to see this gauge."));
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
            ui.add(egui::Label::new(egui::RichText::new(crate::i18n::t("ⓘ Placa: medida. Notebook, dock e portas: estimados (≈), a fonte não manda dados.", "ⓘ GPU: measured. Laptop, dock and ports: estimated (≈), the PSU doesn't send data.")).color(pal::faint()).size(12.0)).wrap());
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
            return (0, crate::i18n::t("Lendo o estado do dock…", "Reading the dock status…").into(), String::new());
        }
        let drops: Vec<&Incident> = self.incidents.iter().filter(|i| i.is_drop).collect();
        let last = drops.last();
        if topo.dock_router.is_none() {
            return (2, crate::i18n::t("O dock está desconectado", "The dock is disconnected").into(), crate::i18n::t("Confira o cabo e se a fonte do dock está ligada na tomada.", "Check the cable and whether the dock's power supply is plugged into the wall.").into());
        }
        let d24 = drops.iter().filter(|i| now - i.start <= 86_400_000).count();
        // "Ciente": quedas já reconhecidas não mantêm o aviso; uma queda nova o traz de volta
        let acked = last.is_some_and(|i| i.start <= crate::config::get().ack_ts);
        if d24 > 0 && acked {
            let n = if d24 == 1 { crate::i18n::t("A queda das últimas 24\u{A0}h foi marcada", "The drop in the last 24\u{A0}h was marked").to_string() } else { tf!("As {d24} quedas das últimas 24\u{A0}h foram marcadas", "The {d24} drops in the last 24\u{A0}h were marked") };
            return (0, crate::i18n::t("Tudo certo com o dock", "The dock is all good").into(), tf!("{n} como ciente. Elas continuam no Histórico.", "{n} as acknowledged. They're still in History."));
        }
        if d24 > 0 {
            let cause = match last {
                Some(i) if i.pd_lost => crate::i18n::t("Na última queda a energia do dock caiu junto: suspeite da fonte ou da tomada.", "In the last drop the dock's power went down too: suspect the PSU or the outlet."),
                Some(_) => crate::i18n::t("Na última queda só a conexão caiu, a energia continuou: suspeite do cabo.", "In the last drop only the connection went down, power stayed on: suspect the cable."),
                None => "",
            };
            let lvl = if d24 >= 3 { 2 } else { 1 };
            let vezes = if d24 == 1 { crate::i18n::t("1 vez", "once").to_string() } else { tf!("{d24} vezes", "{d24} times") };
            return (lvl, tf!("Instável: o dock caiu {vezes} nas últimas 24\u{A0}h", "Unstable: the dock dropped {vezes} in the last 24\u{A0}h"), cause.into());
        }
        let sub = match last {
            Some(i) => tf!("Sem quedas há {}.", "No drops for {}.", fmt_dur(now - i.start)),
            None => crate::i18n::t("Nenhuma queda registrada.", "No drops recorded.").into(),
        };
        (0, crate::i18n::t("Tudo certo com o dock", "The dock is all good").into(), sub)
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
        // aviso de queda (dock presente) pode ser reconhecido; desconectado é estado atual, não
        let ackable = lvl > 0 && topo.dock_router.is_some();
        let mut ack = false;
        // o clique do fundo é registrado antes do conteúdo: o botão "Ciente" fica por cima
        let scope = ui.scope_builder(egui::UiBuilder::new().sense(Sense::click()), |ui| egui::Frame::new()
            .fill(pal::bg().lerp_to_gamma(col, 0.10)) // opaco: a sombra não escurece o miolo
            .shadow(sh)
            .stroke(Stroke::new(1.0, col.gamma_multiply(0.55)))
            .corner_radius(CornerRadius::same(pal::radius()))
            .inner_margin(egui::Margin::symmetric(20, 14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // o link é medido: o bloco de texto fica com o resto da largura; se sobrar pouco
                // (< 300 pt), o link desce para baixo do texto em vez de quebrar letra por letra
                let link = crate::i18n::t("Ver o que aconteceu ›", "See what happened ›");
                let bw = if ackable { ui.painter().layout_no_wrap(crate::i18n::t("✔ Ciente", "✔ Got it").into(), FontId::proportional(14.0), col).size().x + 2.0 * ui.spacing().button_padding.x + 12.0 } else { 0.0 };
                let lw = ui.painter().layout_no_wrap(link.into(), FontId::proportional(14.0), col).size().x + bw;
                let full = ui.available_width();
                let iw = 36.0 + 2.0 * ui.spacing().item_spacing.x; // ícone (30) + respiro (6) + 2 espaçamentos
                let side = lvl > 0 && full - iw - lw - 24.0 >= 300.0;
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(30.0);
                        ui.label(egui::RichText::new(icon).size(30.0).color(col));
                    });
                    ui.add_space(6.0);
                    let tw = if side { full - iw - lw - 24.0 } else { full - iw };
                    ui.vertical(|ui| {
                        ui.set_width(tw);
                        ui.label(egui::RichText::new(&title).size(20.0).color(pal::fg()));
                        if !sub.is_empty() {
                            ui.label(egui::RichText::new(&sub).size(14.5).color(pal::dim()));
                        }
                        if lvl > 0 && !side {
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.add(egui::Label::new(egui::RichText::new(link).size(14.0).color(col)).extend().selectable(false));
                                if ackable {
                                    ui.add_space(12.0);
                                    ack |= ack_button(ui, col);
                                }
                            });
                        }
                    });
                    if side {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ackable {
                                ack |= ack_button(ui, col);
                                ui.add_space(12.0);
                            }
                            ui.add(egui::Label::new(egui::RichText::new(link).size(14.0).color(col)).extend().selectable(false));
                        });
                    }
                });
            }));
        if ack {
            if let Some(i) = self.incidents.iter().filter(|i| i.is_drop).last() {
                let mut c = crate::config::get();
                c.ack_ts = i.start;
                crate::config::set(c);
            }
            return;
        }
        let c = scope.response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if c.clicked() {
            self.tab = if lvl > 0 { Tab::Historico } else { Tab::Enlace };
        }
    }

    /// Pílula do cabeçalho: segue o veredito (verde / âmbar / vermelho), com ícone além da cor.
    fn header_status(&mut self, ui: &mut Ui, topo: &Topology, t: f64, compact: bool) {
        let (lvl, _, _) = self.verdict(topo);
        let now = now_ms();
        let d24 = self.incidents.iter().filter(|i| i.is_drop && now - i.start <= 86_400_000).count();
        let (txt, col, pulse) = match (&topo.dock_router, lvl) {
            (None, _) if topo.read_ms == 0 => (crate::i18n::t("Procurando o dock…", "Looking for the dock…").to_string(), pal::dim(), false),
            (None, _) => (crate::i18n::t("✖ Dock desconectado", "✖ Dock disconnected").to_string(), pal::red(), true),
            (Some(n), 0) => (tf!("✔ Dock conectado · {}", "✔ Dock connected · {}", n.last_arrival.map(|a| fmt_dur(now - a)).unwrap_or_default()), pal::green(), false),
            (Some(_), l) => (tf!("▲ Instável · {d24} queda{} em 24\u{A0}h", "▲ Unstable · {d24} drop{} in 24\u{A0}h", if d24 == 1 { "" } else { "s" }), if l >= 2 { pal::red() } else { pal::amber() }, false),
        };
        // compacto: só o ícone e o número ("▲ 3"); o texto inteiro vai para a dica
        let short = match (&topo.dock_router, lvl) {
            (None, _) if topo.read_ms == 0 => "…".to_string(),
            (None, _) => "✖".to_string(),
            (Some(_), 0) => "✔".to_string(),
            (Some(_), _) => format!("▲ {d24}"),
        };
        let r = if compact { status_pill(ui, &short, col, t, pulse).on_hover_text(&txt) } else { status_pill(ui, &txt, col, t, pulse) };
        if r.clicked() {
            self.tab = if lvl > 0 { Tab::Historico } else { Tab::Geral };
        }
    }

    /// Menu lateral (NVIDIA/Intel): ícone + rótulo, item ativo com barra de destaque.
    fn nav_left(&mut self, ui: &mut Ui, compact: bool) {
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
            let ic = if compact { rect.center() } else { rect.left_center() + vec2(16.0, 0.0) };
            let al = if compact { egui::Align2::CENTER_CENTER } else { egui::Align2::LEFT_CENTER };
            ui.painter().text(ic, al, tab.icon(), FontId::proportional(17.0), if on { pal::cyan() } else { pal::dim() });
            if !compact {
                ui.painter().text(rect.left_center() + vec2(44.0, 0.0), egui::Align2::LEFT_CENTER, tab.label(), FontId::proportional(14.0), if on { pal::fg() } else { pal::dim() });
            }
            let resp = if compact { resp.on_hover_text(tab.label()) } else { resp };
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
                // seis medidores em células iguais (6 numa linha no largo, 3 + 3 no estreito)
                let link_col = if gbps >= 40.0 { pal::green() } else if gbps > 0.0 { pal::amber() } else { pal::red() };
                let up = bench.map(|b| b.h2d_gbs as f32).unwrap_or(0.0);
                let online = pw.map(|p| p.online).unwrap_or(false);
                let pd_col = if online { pal::green() } else { pal::amber() };
                let heat = heat(temp, pal::temp());
                let gs: [(&str, f32, f32, &str, &str, Color32, &str, Tab); 6] = [
                    ("⚡", gbps, 40.0, "Gb/s", t("Conexão", "Link"), link_col, t("Velocidade do enlace TB (chaves USB4\u{A0}20 e 21)", "Thunderbolt link speed (USB4 keys\u{A0}20 and 21)"), Tab::Enlace),
                    ("⇅", up * 8.0, 22.4, "Gb/s", t("Banda", "Bandwidth"), if up >= 2.4 { pal::green() } else { pal::amber() }, t("Banda notebook → placa (o sentido que pesa em jogo: texturas e comandos descem por ele), medida no último teste, em Gb/s como a conexão. Esperado com 40\u{A0}Gb/s: ≈ 20–22\u{A0}Gb/s", "Laptop → GPU bandwidth (the direction that matters in games: textures and commands travel down it), measured in the last test, in Gb/s like the link. Expected at 40\u{A0}Gb/s: ≈ 20–22\u{A0}Gb/s"), Tab::Fluxo),
                    ("🔌", pct, 100.0, "%", if online { t("Carregando", "Charging") } else { t("Bateria", "Battery") }, pd_col, t("Bateria do note; verde = recebendo energia PD do dock", "Laptop battery; green = receiving PD power from the dock"), Tab::Enlace),
                    ("💥", drops24, 10.0, "24\u{A0}h", t("Quedas", "Drops"), if drops24 >= 3.0 { pal::red() } else if drops24 > 0.0 { pal::amber() } else { pal::green() }, t("Quedas do dock/GPU nas últimas 24\u{A0}h", "Dock/GPU drops in the last 24\u{A0}h"), Tab::Historico),
                    ("🌡", temp, 100.0, "°C", t("Temperatura", "Temperature"), heat, t("Temperatura da placa", "GPU temperature"), Tab::Fluxo),
                    ("⇄", last.copy, 100.0, "%", t("Tráfego", "Traffic"), pal::data(), t("Motor Copy da GPU: proxy do tráfego no túnel", "GPU Copy engine: a proxy for tunnel traffic"), Tab::Fluxo),
                ];
                let labels: Vec<(&str, &str)> = gs.iter().map(|x| (x.0, x.4)).collect();
                gauge_grid(ui, &labels, |ui, i, w| {
                    let (icon, v, max, u, l, c, tip, tab) = gs[i];
                    let dp = if i == 1 && v > 0.0 { 1 } else { 0 }; // banda: 1 casa, igual ao nó do dock
                    if gauge_wd(ui, w, icon, v, max, u, l, c, dp).on_hover_text(tip).clicked() {
                        self.tab = tab;
                    }
                });
            });
    }

    fn page_options(&mut self, ui: &mut Ui, ctx: &egui::Context, th: Th) {
        let mut cfg = crate::config::get();
        let before = cfg;
        card(ui, t("🎨  Tema", "🎨  Theme"), pal::cyan(), |ui| {
            ui.horizontal_wrapped(|ui| {
                let auto_name = match self.vendor {
                    Some(v) => tf!("🔍 Automático · {}", "🔍 Automatic · {}", v.name()),
                    None => t("🔍 Automático (pela placa)", "🔍 Automatic (by GPU)").into(),
                };
                for (val, name, preview) in [(0u8, auto_name, self.vendor.unwrap_or(Th::Dark)), (1, t("☀ Claro", "☀ Light").into(), Th::Light), (2, t("🌙 Escuro", "🌙 Dark").into(), Th::Dark)] {
                    if theme_card(ui, &name, preview, cfg.theme == val).clicked() {
                        cfg.theme = val;
                    }
                }
            });
            ui.add_space(4.0);
            ui.label(egui::RichText::new(t("O automático detecta a GPU do dock (AMD, NVIDIA ou Intel) e aplica a paleta do fabricante. AMD usa abas no topo; NVIDIA e Intel, menu à esquerda.", "Automatic detects the dock's GPU (AMD, NVIDIA or Intel) and applies the vendor's palette. AMD uses tabs at the top; NVIDIA and Intel, a menu on the left.")).color(pal::dim()).size(13.5));
        });
        ui.add_space(pal::gap());
        card(ui, t("🌐  Idioma", "🌐  Language"), pal::cyan(), |ui| {
            ui.horizontal_wrapped(|ui| {
                // os nomes das línguas ficam sempre na própria língua
                for (val, name) in [(0u8, t("Automático (idioma do Windows)", "Automatic (Windows language)")), (1, "Português"), (2, "English")] {
                    if ui.selectable_value(&mut cfg.lang, val, name).clicked() {
                        // troca na hora; o config::set no fim da função grava (cfg != before)
                        crate::i18n::apply(cfg.lang);
                    }
                }
            });
        });
        ui.add_space(pal::gap());
        card(ui, t("🖥  Tela", "🖥  Display"), pal::cyan(), |ui| {
            let ppp = ctx.native_pixels_per_point().unwrap_or(1.0);
            // informação, não controle: texto apagado sem moldura
            ui.add(egui::Label::new(egui::RichText::new(tf!("Escala do Windows {:.0}%\u{A0}·\u{A0}{:.0}\u{A0}DPI   ·   Tema ativo: {}", "Windows scale {:.0}%\u{A0}·\u{A0}{:.0}\u{A0}DPI   ·   Active theme: {}", ppp * 100.0, ppp * 96.0, th.name())).color(pal::dim()).size(13.5)).wrap());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(t("🔎 Tamanho da interface", "🔎 Interface size"));
                // trilho visível no escuro (card3) e a parte preenchida até o pegador
                ui.visuals_mut().widgets.inactive.bg_fill = rail_col();
                ui.add(egui::Slider::new(&mut cfg.zoom, 0.8..=1.6).step_by(0.05).trailing_fill(true).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
                if ui.small_button(t("↺ padrão", "↺ default")).clicked() {
                    cfg.zoom = 1.0;
                }
            });
        });
        ui.add_space(pal::gap());
        card(ui, t("🔔  Comportamento", "🔔  Behavior"), pal::cyan(), |ui| {
            // caixas desmarcadas com borda visível no escuro
            check_style(ui);
            let mut auto = crate::config::autostart_enabled();
            if ui.checkbox(&mut auto, t("🚀 Iniciar com o Windows (direto na bandeja)", "🚀 Start with Windows (straight to the tray)")).changed() {
                if let Err(e) = crate::config::set_autostart(auto) {
                    self.flash = Some(tf!("Não consegui alterar a inicialização: {e}", "Couldn't change the startup setting: {e}"));
                }
            }
            ui.checkbox(&mut cfg.notify, t("🔔 Avisar na bandeja quando o dock cair", "🔔 Notify in the tray when the dock drops"));
            ui.checkbox(&mut cfg.eco, t("🍃 Animação econômica (5 qps)", "🍃 Eco animation (5 fps)"));
        });
        ui.add_space(pal::gap());
        card(ui, t("🧰  Ferramentas", "🧰  Tools"), pal::cyan(), |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button(t("📏 Teste de banda", "📏 Bandwidth test")).clicked() {
                    self.start_bench(ctx);
                    self.tab = Tab::Fluxo;
                }
                if ui.button(t("📄 Exportar histórico (CSV)", "📄 Export history (CSV)")).clicked() {
                    match crate::config::export_csv() {
                        Ok(p) => {
                            crate::config::open_folder(p.parent().unwrap_or(&p));
                            self.flash = Some(tf!("Exportado: {}", "Exported: {}", p.display()));
                        }
                        Err(e) => self.flash = Some(tf!("Falha ao exportar: {e}", "Export failed: {e}")),
                    }
                }
                if ui.button(t("📂 Pasta de dados", "📂 Data folder")).clicked() {
                    crate::config::open_folder(&crate::util::data_dir());
                }
            });
            if let Some(f) = &self.flash {
                ui.label(egui::RichText::new(f).color(pal::fg()).size(13.5));
            }
        });
        if cfg != before {
            crate::config::set(cfg);
        }
    }
}

/// Trilho de slider com contraste em todos os temas.
fn rail_col() -> Color32 {
    if pal::current().is_light() { pal::grid() } else { pal::card3().lerp_to_gamma(pal::fg(), 0.08) }
}

/// Caixas de seleção: borda `dim` de 1,5 px (desmarcada não some no escuro); no hover, a borda clareia.
fn check_style(ui: &mut Ui) {
    let w = &mut ui.visuals_mut().widgets;
    w.inactive.bg_stroke = Stroke::new(1.5, pal::dim());
    w.inactive.bg_fill = pal::card2();
    w.hovered.bg_stroke = Stroke::new(1.5, pal::fg());
    w.active.bg_stroke = Stroke::new(1.5, pal::fg());
    w.inactive.fg_stroke.color = pal::fg();
}

/// Miniatura clicável de um tema (pré-visualização com as cores dele).
fn theme_card(ui: &mut Ui, name: &str, th: Th, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(170.0, 112.0), Sense::click());
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
    p.text(rect.left_bottom() + vec2(10.0, -18.0), egui::Align2::LEFT_CENTER, name, FontId::proportional(14.0), fg); // ~8 px de respiro embaixo
    let _ = dim;
    let k = ui.ctx().animate_bool_with_time(resp.id, selected || resp.hovered(), 0.15);
    p.rect_stroke(rect, CornerRadius::same(pal::radius()), Stroke::new(1.0 + 1.5 * k, pal::border()), StrokeKind::Inside);
    if selected {
        // seleção neutra e por fora da miniatura (contraste com o fundo da página, não com a prévia)
        let cr = CornerRadius::same(pal::radius().saturating_add(3));
        ui.painter().rect_stroke(rect.expand(4.0), cr, Stroke::new(2.0, pal::sel()), StrokeKind::Inside);
    }
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
    resp.on_hover_text(tf!("GPU {} detectada no dock", "{} GPU detected on the dock", v.name()));
}

fn logo(ui: &mut Ui, ok: bool) {
    let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
    let p = ui.painter_at(r);
    p.rect_filled(r, CornerRadius::same(7), pal::card2());
    let a = r.left_bottom() + vec2(8.0, -9.0);
    let b = r.right_top() + vec2(-8.0, 9.0);
    p.line_segment([a, b], Stroke::new(2.0, pal::dim()));
    p.circle_filled(a, 4.5, pal::dim());
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
                ui.add(egui::Label::new(egui::RichText::new(text).color(color).size(14.0)).extend());
            });
        })
        .response;
    ui.interact(r.rect, ui.id().with(("pill", text)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Grade de células de largura igual: quantas colunas couberem com `min_w` cada (no máximo `n`),
/// linhas equilibradas (5 itens em 3 colunas viram 3 + 2, nunca 4 + 1) e células esticadas
/// para preencher a largura. `item(ui, i, w)` desenha a célula i com largura w.
fn even_grid(ui: &mut Ui, n: usize, min_w: f32, gap: f32, mut item: impl FnMut(&mut Ui, usize, f32)) {
    if n == 0 {
        return;
    }
    let avail = ui.available_width();
    let fit = (((avail + gap) / (min_w + gap)).floor() as usize).clamp(1, n);
    let rows = n.div_ceil(fit);
    let k = n.div_ceil(rows);
    let w = ((avail - gap * (k as f32 - 1.0)) / k as f32).floor().max(1.0);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing = vec2(gap, gap);
        for r in 0..rows {
            // a última linha, se tiver menos células, estica as suas para não deixar buraco
            let m = ((r + 1) * k).min(n) - r * k;
            let wr = if m < k { ((avail - gap * (m as f32 - 1.0)) / m as f32).floor() } else { w };
            ui.horizontal(|ui| {
                for i in r * k..r * k + m {
                    item(ui, i, wr);
                }
            });
        }
    });
}

/// Um bloco número-grande: ícone + rótulo em cima, valor e unidade embaixo.
struct TileItem {
    icon: &'static str,
    value: String,
    unit: &'static str,
    label: String,
    color: Color32,
    tip: Option<String>,
}

fn ti(icon: &'static str, value: impl Into<String>, unit: &'static str, label: impl Into<String>, color: Color32) -> TileItem {
    TileItem { icon, value: value.into(), unit, label: label.into(), color, tip: None }
}

impl TileItem {
    fn tip(mut self, s: impl Into<String>) -> Self {
        self.tip = Some(s.into());
        self
    }
}

const TPX: f32 = 10.0; // respiro lateral do bloco
const TPY: f32 = 7.0; // respiro vertical do bloco

/// Valor + unidade no tamanho `size` (unidade proporcional).
fn tile_value(p: &egui::Painter, x: &TileItem, size: f32) -> (std::sync::Arc<egui::Galley>, std::sync::Arc<egui::Galley>) {
    let vg = p.layout_no_wrap(x.value.clone(), FontId::proportional(size), x.color);
    let ug = p.layout_no_wrap(x.unit.to_string(), FontId::proportional((size * 0.62).max(11.0)), pal::dim());
    (vg, ug)
}

fn tile_value_w(p: &egui::Painter, x: &TileItem, size: f32) -> f32 {
    let (vg, ug) = tile_value(p, x, size);
    vg.size().x + 3.0 + ug.size().x
}

/// Linha de blocos com largura e altura iguais. A largura mínima vem do bloco mais largo
/// (rótulo inteiro numa linha, valor a ≥ 16 pt); o valor usa o MESMO tamanho em toda a linha,
/// o maior (até 21 pt) que cabe em todos. Nada corta nem quebra no meio. Devolve o bloco clicado.
fn tile_row(ui: &mut Ui, id: &str, items: &[TileItem]) -> Option<usize> {
    tile_row_x(ui, id, items, 0.0)
}

/// `tile_row` com `extra` px a mais de altura (sobra do cartão; o valor continua no pé do bloco).
fn tile_row_x(ui: &mut Ui, id: &str, items: &[TileItem], extra: f32) -> Option<usize> {
    let p = ui.painter().clone();
    let lab = |x: &TileItem| format!("{}\u{A0}{}", x.icon, x.label); // ícone grudado na 1ª palavra
    // quantas colunas: o máximo em que todo bloco cabe com o valor a ≥ 16 pt e o rótulo em
    // até 2 linhas, quebrando só entre palavras (menos linhas de blocos = cartão mais baixo)
    let n = items.len().max(1);
    let avail = ui.available_width();
    let cell = |k: usize| ((avail - 10.0 * (k as f32 - 1.0)) / k as f32).floor();
    let ok = |k: usize| {
        let room = cell(k) - 2.0 * TPX;
        room >= 76.0
            && items.iter().all(|x| {
                let words_fit = lab(x).split(' ').all(|wd| p.layout_no_wrap(wd.to_string(), FontId::proportional(12.5), pal::dim()).size().x <= room);
                let rows = p.layout(lab(x), FontId::proportional(12.5), pal::dim(), room).rows.len();
                words_fit && rows <= 2 && tile_value_w(&p, x, 16.0) <= room
            })
    };
    let fit = (1..=n).rev().find(|&k| ok(k)).unwrap_or(1);
    let rows = n.div_ceil(fit);
    let k = n.div_ceil(rows);
    let cw = cell(k);
    let nat = cell(fit);
    let room = cw - 2.0 * TPX;
    let mut size: f32 = 21.0;
    while size > 13.0 && items.iter().any(|x| tile_value_w(&p, x, size) > room) {
        size -= 1.0;
    }
    let vh = size * 1.3;
    // altura comum: rótulo medido na largura real da célula (a última linha, esticada, é mais larga)
    let h = items
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let r = i / k;
            let m = ((r + 1) * k).min(n) - r * k;
            let w = if m < k { ((avail - 10.0 * (m as f32 - 1.0)) / m as f32).floor() } else { cw };
            p.layout(lab(x), FontId::proportional(12.5), pal::dim(), w - 2.0 * TPX).size().y
        })
        .fold(0.0, f32::max)
        + TPY
        + 4.0
        + vh
        + TPY
        // a sobra se divide entre as linhas: o grupo cresce exatamente `extra` (senão o cartão
        // cresce rows*extra, a medida do eq_cols infla e o layout alterna quadro a quadro)
        + extra / rows as f32;
    let mut clicked = None;
    even_grid(ui, items.len(), nat, 10.0, |ui, i, w| {
        let x = &items[i];
        let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        let resp = ui.interact(r, ui.id().with(("tile", id, i)), Sense::click());
        let lift = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.15);
        let cr = CornerRadius::same(pal::radius().min(8));
        let p = ui.painter();
        p.add(pal::elev(2, lift).as_shape(r, cr));
        p.rect_filled(r, cr, pal::card2());
        if lift > 0.01 {
            p.rect_stroke(r, cr, Stroke::new(1.0, x.color.gamma_multiply(0.7 * lift)), StrokeKind::Inside);
        }
        let lg = p.layout(lab(x), FontId::proportional(12.5), pal::dim(), w - 2.0 * TPX);
        p.galley(pos2(r.left() + TPX, r.top() + TPY), lg, pal::dim());
        // valor e unidade apoiados na mesma linha de base, no pé do bloco
        let (vg, ug) = tile_value(p, x, size);
        let base = r.bottom() - TPY - vh * 0.18;
        let vw = vg.size().x;
        p.galley(pos2(r.left() + TPX, base - vg.size().y * 0.82), vg, x.color);
        p.galley(pos2(r.left() + TPX + vw + 3.0, base - ug.size().y * 0.82), ug, pal::dim());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let resp = match &x.tip {
            Some(t) => resp.on_hover_text(t.clone()),
            None => resp,
        };
        if resp.clicked() {
            clicked = Some(i);
        }
    });
    clicked
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

fn hidden() -> [(&'static str, &'static str, &'static str); 6] {
    [
        ("🔌", t("Fonte ATX", "ATX PSU"), t("Tensões, temperatura e potência: a fonte não tem barramento de dados (PMBus) ligado ao dock.", "Voltages, temperature and power: the PSU has no data bus (PMBus) connected to the dock.")),
        ("🌀", t("Ventoinha", "Fan"), t("Sem sensor de RPM ligado a um controlador visível pelo host.", "No RPM sensor wired to a controller the host can see.")),
        ("🌡", t("Temp. JHL7440", "JHL7440 temp."), t("O controlador Titan Ridge não publica sensor térmico ao Windows.", "The Titan Ridge controller doesn't expose a thermal sensor to Windows.")),
        ("⚡", t("Contrato PD", "PD contract"), t("Tensão/corrente negociadas ficam no controlador PD do dock; o Windows só vê energia externa sim/não e a taxa de carga.", "Negotiated voltage/current stay in the dock's PD controller; Windows only sees external power yes/no and the charge rate.")),
        ("📶", t("Tráfego real", "Real traffic"), t("Nenhum contador de bytes por adaptador USB4/TB. O painel usa o motor Copy da GPU como proxy.", "No per-adapter byte counter for USB4/TB. The panel uses the GPU's Copy engine as a proxy.")),
        ("📉", t("Erros do enlace", "Link errors"), t("Contadores de erro/retreino das faixas não são publicados. Só aparecem quedas (PnP) e erros PCIe (WHEA/AER).", "Lane error/retrain counters aren't published. Only drops (PnP) and PCIe errors (WHEA/AER) show up.")),
    ]
}

fn hidden_strip(ui: &mut Ui, sel: &mut Option<Sel>) {
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(t("Não dá para medir:", "Can't be measured:")).color(pal::dim()).size(13.0)).on_hover_text(t("Coisas do dock que o Windows não enxerga. Passe o mouse em cada ícone.", "Dock things Windows can't see. Hover each icon."));
        for (icon, name, why) in hidden() {
            // medido antes de alocar, para a linha quebrar entre os ícones em vez de vazar
            let g = ui.painter().layout_no_wrap(icon.to_string(), FontId::proportional(14.0), pal::faint());
            let (r, _) = ui.allocate_exact_size(g.size() + vec2(18.0, 6.0), Sense::hover());
            ui.painter().rect(r, CornerRadius::same(pal::radius()), pal::well(), Stroke::new(1.0, pal::border()), StrokeKind::Inside);
            let rr = Rect::from_center_size(r.center(), g.size());
            ui.painter().galley(rr.min, g, pal::faint());
            // risco diagonal: "não disponível"
            ui.painter().line_segment([rr.left_bottom(), rr.right_top()], Stroke::new(1.0, pal::red().gamma_multiply(0.45)));
            if ui.interact(r, ui.id().with(("hid", name)), Sense::click()).on_hover_text(format!("{name}: {why}")).clicked() {
                *sel = Some(Sel::Info(Topic::Hidden));
            }
        }
    });
}

/// Faixas do enlace desenhadas: cada faixa é uma pista de 0–20 Gb/s (TB3).
fn lanes_view(ui: &mut Ui, lanes: u32, per_lane: Option<f32>, t: f64, extra: f32) -> egui::Response {
    let w = ui.available_width();
    // `extra` (sobra do cartão) alarga o passo entre as faixas e engrossa um pouco a pista
    let nl = lanes.max(1) as f32;
    let pitch = 18.0 + extra / nl;
    let th = 11.0 + (extra / nl * 0.25).min(5.0);
    let h = pitch * nl + 22.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let p = ui.painter_at(rect);
    // colunas medidas: rótulo "Canal N" à esquerda, valor "20/20 Gb/s" à direita, pista no meio
    let lw = p.layout_no_wrap(crate::i18n::t("Canal 8", "Lane 8").into(), FontId::proportional(12.5), pal::dim()).size().x;
    let vw = p.layout_no_wrap("20/20\u{A0}Gb/s".into(), FontId::proportional(13.0), pal::dim()).size().x;
    let x0 = rect.left() + lw + 10.0;
    let x1 = rect.right() - vw - 10.0;
    for l in 0..lanes.max(1) {
        let y = rect.top() + 4.0 + l as f32 * pitch;
        let track = Rect::from_min_max(pos2(x0, y), pos2(x1, y + th));
        p.rect_filled(track, CornerRadius::same(5), pal::well());
        p.text(pos2(rect.left(), y + th / 2.0), egui::Align2::LEFT_CENTER, tf!("Canal {}", "Lane {}", l + 1), FontId::proportional(12.5), pal::dim());
        if let Some(v) = per_lane {
            let frac = (v / 20.0).clamp(0.0, 1.0);
            let fill = Rect::from_min_max(track.min, pos2(x0 + (x1 - x0) * frac, track.bottom()));
            let col = if frac < 0.99 { pal::amber() } else { pal::green() };
            p.rect_filled(fill, CornerRadius::same(5), col.gamma_multiply(0.85));
            // brilho correndo dentro da faixa
            let sx = fill.left() + ((t as f32 * 60.0 + l as f32 * 37.0) % fill.width().max(1.0));
            p.rect_filled(Rect::from_center_size(pos2(sx, fill.center().y), vec2(14.0, th - 2.0)), CornerRadius::same(4), Color32::WHITE.gamma_multiply(0.18));
            p.text(pos2(x1 + 8.0, y + th / 2.0), egui::Align2::LEFT_CENTER, format!("{v:.0}/20\u{A0}Gb/s"), FontId::proportional(13.0), col);
        }
    }
    let total = per_lane.map(|v| v * lanes as f32);
    if let Some(t) = total {
        // linha própria, alinhada à direita sob os valores: nunca sai cortado
        p.text(pos2(rect.right(), rect.bottom() - 2.0), egui::Align2::RIGHT_BOTTOM, tf!("Total {t:.0} de 40\u{A0}Gb/s", "Total {t:.0} of 40\u{A0}Gb/s"), FontId::proportional(12.5), pal::dim());
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
fn drops_chart(ui: &mut Ui, incidents: &[&Incident], h: f32) -> egui::Response {
    use chrono::{Local, TimeZone};
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
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
        // 14 px no topo para o número da barra mais alta não sair cortado
        let hmax = rect.height() - 16.0 - 14.0;
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
    p.text(rect.left_bottom(), egui::Align2::LEFT_BOTTOM, t("14\u{A0}dias atrás", "14\u{A0}days ago"), FontId::proportional(9.5), pal::faint());
    p.text(rect.right_bottom(), egui::Align2::RIGHT_BOTTOM, t("hoje", "today"), FontId::proportional(9.5), pal::faint());
    resp.on_hover_text(t("Quedas por dia. Âmbar: o PD caiu junto (o dock inteiro apagou). Vermelho: só o enlace caiu.", "Drops per day. Amber: PD dropped too (the whole dock went dark). Red: only the link dropped."))
}

/// Medidor circular (arco de 270 graus) com valor animado.
fn gauge(ui: &mut Ui, icon: &str, value: f32, max: f32, unit: &str, label: &str, color: Color32) -> egui::Response {
    let w = gauge_nat(ui, icon, label);
    gauge_w(ui, w, icon, value, max, unit, label, color)
}

/// Largura natural de um medidor: cresce com o rótulo, para ele nunca sair cortado.
fn gauge_nat(ui: &Ui, icon: &str, label: &str) -> f32 {
    let lw = ui.painter().layout_no_wrap(format!("{icon} {label}"), FontId::proportional(12.5), pal::dim()).size().x;
    (lw + 8.0).max(96.0)
}

/// Linha de medidores com células de largura igual (ver `even_grid`).
fn gauge_grid(ui: &mut Ui, labels: &[(&str, &str)], mut item: impl FnMut(&mut Ui, usize, f32)) {
    let nat = labels.iter().map(|(i, l)| gauge_nat(ui, i, l)).fold(96.0, f32::max);
    even_grid(ui, labels.len(), nat, 8.0, |ui, i, w| item(ui, i, w));
}

/// Medidor com largura `w` (o arco fica no centro da célula); valor NaN = desconhecido ("—").
#[allow(clippy::too_many_arguments)]
fn gauge_w(ui: &mut Ui, w: f32, icon: &str, value: f32, max: f32, unit: &str, label: &str, color: Color32) -> egui::Response {
    gauge_wd(ui, w, icon, value, max, unit, label, color, 0)
}

/// `gauge_w` com `dp` casas decimais no valor (vírgula, como no resto do painel).
#[allow(clippy::too_many_arguments)]
fn gauge_wd(ui: &mut Ui, w: f32, icon: &str, value: f32, max: f32, unit: &str, label: &str, color: Color32, dp: usize) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 92.0), Sense::click());
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
    p.text(c + vec2(0.0, -3.0), egui::Align2::CENTER_CENTER, if unknown { "—".to_string() } else { dec(value as f64, dp) }, FontId::proportional(19.0), if unknown { pal::dim() } else { pal::fg() });
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

/// Estado da fonte para a UI. Tudo é estimado (a fonte não manda dados): nunca vermelho nem ✕,
/// que ficam só para queda medida. Picos rápidos acima da fonte = risco; uso pesado > 85% = atenção.
struct PsuState {
    col: Color32,
    icon: &'static str,
    pill: &'static str,
    msg: String,
}

fn psu_state(l: &crate::psu::Load, watts: u32) -> PsuState {
    use crate::psu::Verdict;
    let w = watts.max(1) as f32;
    match crate::psu::verdict(l, watts) {
        Verdict::Risk => PsuState {
            col: pal::amber(),
            icon: "⚠",
            pill: t("Risco (estimado)", "Risk (estimated)"),
            msg: if l.spike > w {
                tf!(
                    "Risco (estimado): os picos rápidos da placa (≈ {:.0}\u{A0}W) passam dos {watts}\u{A0}W da fonte. O dock pode desligar inteiro, e o notebook para de carregar junto.",
                    "Risk (estimated): the card's quick spikes (≈ {:.0}\u{A0}W) go past the PSU's {watts}\u{A0}W. The whole dock may shut off, and the laptop stops charging with it.",
                    l.spike
                )
            } else {
                tf!(
                    "Risco (estimado): no uso pesado a carga (≈ {:.0}\u{A0}W) passa dos {watts}\u{A0}W da fonte.",
                    "Risk (estimated): under heavy use the load (≈ {:.0}\u{A0}W) goes past the PSU's {watts}\u{A0}W.",
                    l.worst
                )
            },
        },
        Verdict::Tight => PsuState {
            col: pal::amber(),
            icon: "▲",
            pill: t("Pouca folga", "Low headroom"),
            msg: tf!(
                "Pouca folga: no uso pesado a fonte chega a ≈ {:.0}%. Funciona, mas com pouca margem.",
                "Low headroom: under heavy use the PSU reaches ≈ {:.0}%. It works, but with little margin.",
                l.worst / w * 100.0
            ),
        },
        Verdict::Ok => PsuState {
            col: pal::green(),
            icon: "✔",
            pill: t("Folga boa", "Good headroom"),
            msg: tf!(
                "Folga boa: no uso pesado a fonte fica em ≈ {:.0}%, e os picos rápidos (≈ {:.0}\u{A0}W) cabem nela.",
                "Good headroom: under heavy use the PSU sits at ≈ {:.0}%, and the quick spikes (≈ {:.0}\u{A0}W) fit within it.",
                l.worst / w * 100.0,
                l.spike
            ),
        },
    }
}

/// Cor de uma fração estimada da fonte: verde até 85%, âmbar acima (estimativa nunca fica vermelha).
fn load_col(f: f32) -> Color32 {
    if f > 0.85 { pal::amber() } else { pal::green() }
}

/// Campo numérico de texto, alinhado à esquerda, com a unidade à direita. Valor 0 = não informado:
/// mostra só a dica esmaecida (`hint`). Devolve o novo valor quando o texto muda.
fn num_field(ui: &mut Ui, key: &str, w: f32, v: f32, dec: usize, unit: &str, hint: &str) -> Option<f32> {
    let id = ui.id().with(key);
    let bid = id.with("buf");
    let shown = if v > 0.0 { crate::i18n::dec_sep(format!("{v:.dec$}")) } else { String::new() };
    let mut buf = ui.data(|d| d.get_temp::<String>(bid)).unwrap_or(shown);
    let r = ui.add(
        egui::TextEdit::singleline(&mut buf)
            .id(id)
            .hint_text(egui::RichText::new(hint).color(pal::faint()))
            .desired_width(f32::INFINITY)
            .min_size(vec2(w, 32.0))
            .vertical_align(egui::Align::Center),
    );
    if !buf.trim().is_empty() {
        let tw = ui.painter().layout_no_wrap(format!("{buf} "), FontId::proportional(14.0), pal::fg()).size().x;
        let ux = (r.rect.right() - 12.0).max(r.rect.left() + 8.0 + tw);
        ui.painter().text(pos2(ux, r.rect.center().y), egui::Align2::RIGHT_CENTER, unit, FontId::proportional(13.5), pal::dim());
    }
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(bid, buf.clone()));
    } else {
        ui.data_mut(|d| d.remove::<String>(bid));
    }
    if r.changed() {
        let t = buf.trim().replace(',', ".");
        let t = t.trim_end_matches(|c: char| c.is_alphabetic() || c == ' ');
        return if t.is_empty() { Some(0.0) } else { t.parse::<f32>().ok() };
    }
    None
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
/// notebook/dock/portas estimados (hachurados); uso pesado (tracejado) e picos rápidos (▼) marcados.
fn load_bar(ui: &mut Ui, l: &crate::psu::Load, w: f32, width: f32) {
    let id = ui.id().with("psu-bar");
    // cabeçalho
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(t("Carga agora", "Load now")).size(12.5).color(pal::dim()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(tf!("≈ {:.0} de {:.0}\u{A0}W", "≈ {:.0} of {:.0}\u{A0}W", l.now, w)).size(14.0).strong().color(pal::fg()));
        });
    });
    let (area, _) = ui.allocate_exact_size(vec2(width, 16.0 + 24.0 + 22.0), Sense::hover());
    let track = Rect::from_min_size(pos2(area.left(), area.top() + 16.0), vec2(width, 24.0));
    let p = ui.painter();
    let rad = pal::radius();
    // trilho com contraste nos dois temas (no claro o `well` some sobre o cartão)
    if pal::current().is_light() {
        p.rect_filled(track, CornerRadius::same(rad), pal::grid());
        p.rect_stroke(track, CornerRadius::same(rad), Stroke::new(1.0, pal::border()), StrokeKind::Outside);
    } else {
        p.rect_filled(track, CornerRadius::same(rad), pal::well());
    }
    let sx = |v: f32| track.left() + (v / w).clamp(0.0, 1.0) * track.width();
    let segs = [
        (t("🎮 Placa", "🎮 GPU"), l.gpu, pal::data(), true),
        (t("💻 Notebook pelo PD", "💻 Laptop via PD"), l.pd, pal::violet(), false),
        ("▣ Dock", l.dock, pal::dim(), false),
        (t("🖴 Portas", "🖴 Ports"), l.usb, pal::faint(), false),
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
        let tip = format!("{name}: {}{v:.0}\u{A0}W ({})", if *measured { "" } else { "≈ " }, if *measured { t("medido", "measured") } else { t("estimado", "estimated") });
        ui.interact(r, id.with(("seg", k)), Sense::hover()).on_hover_text(tip);
    }
    let p = ui.painter();
    // sombra interna na borda de cima
    p.line_segment([track.left_top(), track.right_top()], Stroke::new(1.0, Color32::from_black_alpha(60)));
    // uso pesado (sustentado): tracejado
    let wx = sx(l.worst);
    let wc = if l.worst > w * 0.85 { pal::amber() } else { pal::dim() };
    p.add(egui::Shape::dashed_line(&[pos2(wx, track.top() - 3.0), pos2(wx, track.bottom() + 3.0)], Stroke::new(2.0, wc), 4.0, 3.0));
    // folga no espaço livre (desviando do tracejado do uso pesado)
    let free = track.right() - end_x;
    if free >= 90.0 {
        let txt = tf!("folga {:.0}\u{A0}W", "{:.0}\u{A0}W free", (w - l.now).max(0.0));
        let (a, b) = if wx > end_x + 1.0 && wx < track.right() - 1.0 {
            if wx - end_x >= track.right() - wx { (end_x, wx) } else { (wx, track.right()) }
        } else {
            (end_x, track.right())
        };
        if b - a >= 90.0 {
            p.text(pos2((a + b) / 2.0, track.center().y), egui::Align2::CENTER_CENTER, txt, FontId::proportional(12.0), pal::dim());
        }
    }
    // picos rápidos: triângulo em cima do trilho (preso no fim) e rótulo à esquerda dele.
    // Estimativa: âmbar (risco estimado), nunca vermelho
    let over = l.spike > w;
    let kx = sx(l.spike).min(track.right() - 6.0);
    let kc = if over { pal::amber() } else { pal::dim() };
    p.text(pos2(kx, track.top() - 1.0), egui::Align2::CENTER_BOTTOM, "▼", FontId::proportional(12.0), kc);
    let klabel = if over { t("⚠ picos rápidos passam da fonte", "⚠ quick spikes exceed the PSU").to_string() } else { tf!("picos rápidos ≈ {:.0}\u{A0}W", "quick spikes ≈ {:.0}\u{A0}W", l.spike) };
    p.text(pos2(kx - 10.0, track.top() - 2.0), egui::Align2::RIGHT_BOTTOM, klabel, FontId::proportional(11.5), kc);
    if over {
        p.line_segment([pos2(track.right() - 1.0, track.top()), pos2(track.right() - 1.0, track.bottom())], Stroke::new(2.0, pal::amber()));
    }
    // régua e rótulo do uso pesado embaixo
    let ry = track.bottom() + 5.0;
    let f11 = FontId::proportional(11.0);
    let r0 = p.text(pos2(track.left(), ry), egui::Align2::LEFT_TOP, "0", f11.clone(), pal::faint());
    let r1 = p.text(pos2(track.right(), ry), egui::Align2::RIGHT_TOP, format!("{:.0}\u{A0}W", w), f11.clone(), pal::faint());
    let g = p.layout_no_wrap(tf!("uso pesado ≈ {:.0}\u{A0}W", "heavy use ≈ {:.0}\u{A0}W", l.worst), FontId::proportional(11.5), wc);
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
            let short = if k == 1 { t("💻 Notebook", "💻 Laptop") } else { name };
            let g = ui.painter().layout_no_wrap(format!("{short} {}{v:.0}\u{A0}W", if *measured { "" } else { "≈ " }), FontId::proportional(13.0), pal::dim());
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
    let g = p.layout(t("Informe o modelo ou a potência para ver a carga ao vivo.", "Enter the model or wattage to see the live load.").into(), FontId::proportional(14.5), pal::dim(), (r.width() - 80.0).max(60.0));
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
    // tamanho medido antes de alocar: assim o horizontal_wrapped consegue quebrar a linha entre chips
    // (um Frame não avisa a largura antes e vazava para fora do cartão)
    let g = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(13.5), tc);
    let (r, _) = ui.allocate_exact_size(g.size() + vec2(18.0, 6.0), Sense::hover());
    let cr = CornerRadius::same(pal::radius());
    ui.painter().rect(r, cr, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    ui.painter().galley(r.center() - g.size() / 2.0, g, tc);
    ui.interact(r, ui.id().with(("chip", text)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

// ---------------------------------------------------------------------------
// Cards

impl App {
    fn card_link(&mut self, ui: &mut Ui, topo: &Topology, bench: Option<&BenchResult>) {
        use crate::i18n::t as tr;
        let mut sel = self.sel;
        card_i(ui, tr("⚡  Enlace Thunderbolt", "⚡  Thunderbolt link"), pal::bolt(), &mut sel, Topic::Link, tr("Como a velocidade foi decodificada", "How the speed was decoded"), |ui, sel| {
            let Some(dr) = &topo.dock_router else {
                ui.label(egui::RichText::new(tr("⛓ enlace desconectado", "⛓ link disconnected")).color(pal::red()).size(16.0));
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
            // sobra do cartão (linha eq_cols): faixas mais espaçadas, respiro e blocos mais altos
            let lx = flex(0.5, 14.0 * lanes.max(1) as f32);
            if lanes_view(ui, lanes, per_lane, t, lx).on_hover_text(tr("Chaves USB4\u{A0}20 (velocidade) e 21 (faixas). Clique para ver a decodificação.", "USB4 keys\u{A0}20 (speed) and 21 (lanes). Click to see the decoding.")).clicked() {
                *sel = Some(Sel::Info(Topic::Link));
            }
            if speed == Some(8) {
                let low = bench.map(|b| b.h2d_gbs < 2.2).unwrap_or(false);
                let txt = if low { tr("ℹ O cabo está na metade da velocidade", "ℹ The cable is running at half speed") } else { tr("ℹ O cabo parece estar na metade da velocidade", "ℹ The cable seems to be at half speed") };
                if ui.add(egui::Label::new(egui::RichText::new(txt).color(pal::amber()).size(14.0)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    *sel = Some(Sel::Info(Topic::Link));
                }
            }
            ui.add_space(4.0 + flex(0.4, 16.0));
            let (st, stc) = match dr.dstate {
                Some(1) => (tr("Ativo", "Active"), pal::green()),
                Some(4) => (tr("☾ Em repouso", "☾ Idle"), pal::dim()),
                _ => ("?", pal::dim()),
            };
            let mut tiles = vec![(ti("💾", dr.firmware.clone().unwrap_or("?".into()), "", "Firmware", pal::fg()).tip(tr("Firmware do roteador Titan Ridge do dock", "Firmware of the dock's Titan Ridge router")), Sel::Dock)];
            if let Some(ts) = dr.last_arrival {
                tiles.push((ti("🕒", fmt_dur(now_ms() - ts), "", tr("Conectado há", "Connected for"), pal::green()).tip(tf!("desde {}", "since {}", fmt_local(ts, false))), Sel::Info(Topic::Stability)));
            }
            tiles.push((ti("🔀", st, "", tr("Estado", "State"), stc).tip(tr("Estado de energia do roteador do dock (D0 ativo, D3 economia — o USB4 dorme quando ocioso)", "Power state of the dock's router (D0 active, D3 saving — USB4 sleeps when idle)")), Sel::Dock));
            let (items, dest): (Vec<TileItem>, Vec<Sel>) = tiles.into_iter().unzip();
            if let Some(i) = tile_row_x(ui, "enlace", &items, flex(1.0, 28.0)) {
                *sel = Some(dest[i]);
            }
        });
        self.sel = sel;
    }

    fn card_power(&mut self, ui: &mut Ui, samples: &[Sample]) {
        use crate::i18n::t as tr;
        let mut sel = self.sel;
        card_i(ui, tr("🔌  Energia PD", "🔌  PD power"), pal::bolt(), &mut sel, Topic::Power, tr("O que dá e o que não dá para saber do PD", "What you can and can't know about PD"), |ui, sel| {
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
                    ui.label(egui::RichText::new(if pw.online { "🔌 PD" } else { tr("🔋 bateria", "🔋 battery") }).color(if pw.online { pal::green() } else { pal::amber() }).size(13.0));
                });
            });
            let w = if pw.charge_mw > 0 { pw.charge_mw as f64 / 1000.0 } else { -(pw.discharge_mw as f64) / 1000.0 };
            let wtxt = if w.abs() < 0.05 { "0".into() } else { crate::i18n::dec_sep(format!("{w:+.1}")) };
            let items = [
                ti("🔋", wtxt, "W", tr("Carga da bateria", "Battery charge"), if w >= 0.0 { pal::bolt() } else { pal::amber() }).tip(tr("Taxa de carga (+) ou descarga (−) da bateria — WMI root\\wmi BatteryStatus", "Battery charge (+) or discharge (−) rate — WMI root\\wmi BatteryStatus")),
                ti("〰", dec(pw.mv as f64 / 1000.0, 2), "V", tr("Tensão da bateria", "Battery voltage"), pal::fg()).tip(tr("Tensão da bateria (não é a tensão do PD)", "Battery voltage (not the PD voltage)")),
                ti("⚡", "85", "W", tr("Carregador oferece", "Charger offers"), pal::dim()).tip(tr("Valor do DROM do dock (\"Module 2 PD 85W\"). O contrato negociado não é exposto.", "Value from the dock's DROM (\"Module 2 PD 85W\"). The negotiated contract is not exposed.")),
            ];
            if tile_row(ui, "energia", &items).is_some() {
                *sel = Some(Sel::Info(Topic::Power));
            }
            let series: Vec<f32> = samples.iter().filter_map(|s| s.power).map(|p| (p.charge_mw - p.discharge_mw) as f32 / 1000.0).collect();
            chart(ui, tr("Carga da bateria (W)", "Battery charge (W)"), &[(pal::bolt(), tr("agora", "now"), series)], 10.0, " W", 64.0 + flex(1.0, 150.0));
        });
        self.sel = sel;
    }

    fn card_stability(&mut self, ui: &mut Ui, topo: &Topology) {
        use crate::i18n::t as tr;
        let mut sel = self.sel;
        // faixa do cartão na cor do veredito: vermelho só quando o problema foi medido
        let accent = match self.verdict(topo).0 {
            0 => pal::green(),
            1 => pal::amber(),
            _ => pal::red(),
        };
        let incidents = &self.incidents;
        card_i(ui, tr("🛡  Estabilidade do dock", "🛡  Dock stability"), accent, &mut sel, Topic::Stability, tr("Como as quedas são classificadas", "How drops are classified"), |ui, sel| {
            let now = now_ms();
            let drops: Vec<&Incident> = incidents.iter().filter(|i| i.is_drop && i.events.iter().any(|e| e.kind.cat() == Cat::Dock || e.kind == Kind::GpuOff)).collect();
            let count = |ms: i64| drops.iter().filter(|i| now - i.start <= ms).count();
            let c24 = count(86_400_000);
            let items: Vec<TileItem> = [("💥", c24, "24\u{A0}h", if c24 > 0 { pal::red() } else { pal::green() }), ("📅", count(7 * 86_400_000), tr("7\u{A0}dias", "7\u{A0}days"), pal::fg()), ("🗓", count(30 * 86_400_000), tr("30\u{A0}dias", "30\u{A0}days"), pal::fg())]
                .into_iter()
                .map(|(icon, v, l, c)| ti(icon, v.to_string(), "", tf!("quedas {}", "drops {}", l), c))
                .collect();
            if tile_row(ui, "quedas", &items).is_some() {
                self_open_timeline(sel);
            }
            if drops_chart(ui, &drops, 64.0 + flex(1.0, 150.0)).clicked() {
                *sel = Some(Sel::Info(Topic::Stability));
            }
            let d30: Vec<&&Incident> = drops.iter().filter(|i| now - i.start <= 30 * 86_400_000).collect();
            if !d30.is_empty() {
                let pd = d30.iter().filter(|i| i.pd_lost).count();
                ui.horizontal_wrapped(|ui| {
                    if chip(ui, true, &tf!("🔋 {}/{} com PD caindo junto", "🔋 {}/{} with PD dropping too", pd, d30.len()), pal::amber()) {
                        *sel = Some(Sel::Info(Topic::Stability));
                    }
                    if let Some(ts) = topo.dock_router.as_ref().and_then(|n| n.last_arrival) {
                        let _ = chip(ui, true, &tf!("🕒 sem cair há {}", "🕒 no drops for {}", fmt_dur(now - ts)), pal::green());
                    }
                });
            }
        });
        self.sel = sel;
    }

    fn card_flow(&mut self, ui: &mut Ui, samples: &[Sample], bench: &BenchState, ctx: &egui::Context) {
        use crate::i18n::t as tr;
        let mut sel = self.sel;
        let mut run = false;
        card_i(ui, tr("📶  Fluxo de dados no túnel", "📶  Data flow in the tunnel"), pal::cyan(), &mut sel, Topic::Flow, tr("Por que o motor Copy é o proxy do tráfego", "Why the Copy engine is the traffic proxy"), |ui, sel| {
            let last = samples.last().copied().unwrap_or_default();
            ui.horizontal(|ui| {
                if gauge(ui, "⇄", last.copy, 100.0, "%", tr("Tráfego", "Traffic"), pal::data()).on_hover_text(tr("Motor Copy: cópias DMA host↔GPU — o que mais passa pelo túnel", "Copy engine: host↔GPU DMA copies — what mostly flows through the tunnel")).clicked() {
                    *sel = Some(Sel::Info(Topic::Flow));
                }
                if gauge(ui, "△", last.d3, 100.0, "%", tr("Gráficos 3D", "3D graphics"), pal::violet()).clicked() {
                    *sel = Some(Sel::Info(Topic::Flow));
                }
                ui.vertical(|ui| {
                    ui.add_space(14.0);
                    meter_bar(ui, last.ded_mb as f32 / 16368.0, &[tr("Memória da placa (VRAM)", "Card memory (VRAM)"), "VRAM"], &gb_of(last.ded_mb as f64, 16368.0), pal::data())
                        .on_hover_text(tr("Memória dedicada da GPU em uso (PDH GPU Adapter Memory)", "Dedicated GPU memory in use (PDH GPU Adapter Memory)"));
                    ui.add_space(6.0);
                    meter_bar(ui, last.shared_mb as f32 / 8192.0, &[tr("RAM do notebook usada pela placa", "Laptop RAM used by the card"), tr("RAM do notebook", "Laptop RAM"), tr("RAM do note", "RAM")], &gb_of(last.shared_mb as f64, 8192.0), pal::violet())
                        .on_hover_text(tr("RAM do note mapeada pela GPU — cada acesso atravessa o túnel TB", "Laptop RAM mapped by the GPU — every access crosses the TB tunnel"));
                });
            });
            let copy: Vec<f32> = samples.iter().map(|s| s.copy).collect();
            let d3: Vec<f32> = samples.iter().map(|s| s.d3).collect();
            chart(ui, tr("Uso da placa (5\u{A0}min)", "Card usage (5\u{A0}min)"), &[(pal::data(), tr("Cópia de dados", "Data copy"), copy), (pal::violet(), tr("Gráficos 3D", "3D graphics"), d3)], 100.0, "%", 84.0 + flex(1.0, 170.0));
            ui.horizontal(|ui| {
                let running = matches!(bench, BenchState::Running(_));
                if ui.add_enabled(!running, egui::Button::new(tr("📏 Teste de banda", "📏 Bandwidth test"))).on_hover_text(tr("Cópias D3D12 de 1 GiB na fila COPY, host↔VRAM (~5\u{A0}s).", "D3D12 copies of 1 GiB on the COPY queue, host↔VRAM (~5\u{A0}s).")).clicked() {
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
        use crate::i18n::t as tr;
        let mut sel = self.sel;
        card_i(ui, tr("🎮  GPU no dock", "🎮  GPU in the dock"), pal::data(), &mut sel, Topic::Gpu, tr("Dados via AMD ADLX", "Data via AMD ADLX"), |ui, sel| {
            let g = samples.last().and_then(|s| s.gpu).unwrap_or_default();
            let temp = g.temp.unwrap_or(0.0) as f32;
            let hot = g.hot.unwrap_or(0.0) as f32;
            let gs = [
                ("🌡", temp, 100.0, "°C", tr("Temperatura", "Temperature"), heat(temp, pal::temp())),
                ("🔥", hot, 110.0, "°C", tr("Ponto mais quente", "Hot spot"), heat(hot - 10.0, pal::hot())),
                ("⚡", g.tbp.unwrap_or(0.0) as f32, 190.0, "W", tr("Consumo", "Power"), pal::bolt()),
                ("⏱", g.clk.unwrap_or(0) as f32, 2900.0, "MHz", tr("Velocidade", "Clock"), pal::data()), // cor de dado, não de marca
                ("🌀", g.fan.unwrap_or(0) as f32, 3300.0, "RPM", tr("Ventoinha", "Fan"), pal::violet()),
            ];
            let labels: Vec<(&str, &str)> = gs.iter().map(|x| (x.0, x.4)).collect();
            gauge_grid(ui, &labels, |ui, i, w| {
                let (icon, v, max, u, l, c) = gs[i];
                if gauge_w(ui, w, icon, v, max, u, l, c).clicked() {
                    *sel = Some(Sel::Gpu);
                }
            });
            let temp: Vec<f32> = samples.iter().filter_map(|s| s.gpu.and_then(|g| g.temp)).map(|v| v as f32).collect();
            let hot: Vec<f32> = samples.iter().filter_map(|s| s.gpu.and_then(|g| g.hot)).map(|v| v as f32).collect();
            let tbp: Vec<f32> = samples.iter().filter_map(|s| s.gpu.and_then(|g| g.tbp)).map(|v| v as f32).collect();
            chart(ui, "🌡 / ⚡", &[(pal::temp(), tr("Temperatura|°C", "Temperature|°C"), temp), (pal::hot(), tr("Ponto mais quente|°C", "Hot spot|°C"), hot), (pal::bolt(), tr("Consumo|W", "Power|W"), tbp)], 90.0, "", 70.0 + flex(1.0, 170.0));
            if let Some(s) = st {
                let vram = g.vram.unwrap_or(0) as f32;
                let frac = if s.vram_total_mb > 0 { vram / s.vram_total_mb as f32 } else { 0.0 };
                meter_bar(ui, frac, &[tr("Memória da placa (VRAM)", "Card memory (VRAM)"), "VRAM"], &gb_of(vram as f64, s.vram_total_mb as f64), pal::data());
            }
        });
        self.sel = sel;
    }

    fn card_timeline(&mut self, ui: &mut Ui, compact: bool) {
        use crate::i18n::t as tr;
        let now = now_ms();
        let mut sel = self.sel;
        let mut go_all = false;
        let title = if compact { tr("🕓  O que aconteceu", "🕓  What happened") } else { tr("🕓  Linha do tempo", "🕓  Timeline") };
        card_i(ui, title, pal::red(), &mut sel, Topic::Stability, tr("Fontes: Event Log, PnP ao vivo, Overwatch", "Sources: Event Log, live PnP, Overwatch"), |ui, _sel| {
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
                        if chip(ui, self.range == r, r.label(), pal::fg()) {
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
                // incidentes (severidade 2) em vermelho, como na lista, e por cima dos outros
                let mut evs: Vec<_> = self.events.iter().filter(|e| e.ts >= t0 && enabled.contains(&e.kind.cat())).collect();
                evs.sort_by_key(|e| e.kind.severity());
                for e in evs {
                    let x = x_of(e.ts);
                    let (y0, y1) = match e.kind.severity() {
                        2 => (rect.top() + 6.0, rect.bottom() - 14.0),
                        1 => (rect.top() + 12.0, rect.bottom() - 16.0),
                        _ => (rect.top() + 18.0, rect.bottom() - 18.0),
                    };
                    p.line_segment([pos2(x, y0), pos2(x, y1)], if e.kind.severity() == 2 { Stroke::new(2.0, pal::red()) } else { Stroke::new(1.0, cat_color(e.kind.cat())) });
                }
                p.text(rect.left_bottom() + vec2(8.0, -2.0), egui::Align2::LEFT_BOTTOM, fmt_local(t0, false), FontId::proportional(11.5), pal::faint());
                p.text(rect.right_bottom() + vec2(-8.0, -2.0), egui::Align2::RIGHT_BOTTOM, tr("agora", "now"), FontId::proportional(11.5), pal::faint());
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
                ui.label(egui::RichText::new(if compact { tr("✔ nenhum problema em 30\u{A0}dias", "✔ no problems in 30\u{A0}days") } else { tr("✔ nada neste período", "✔ nothing in this period") }).color(pal::green()));
            }
            let mut open = self.open_inc;
            for inc in all.into_iter().take(limit) {
                incident_row(ui, inc, &mut open);
            }
            self.open_inc = open;
            ui.horizontal(|ui| {
                if compact {
                    if total > limit && ui.add(egui::Button::new(link_text(tf!("Ver histórico completo ({}) →", "See full history ({}) →", total))).frame(false)).clicked() {
                        go_all = true;
                    } else if total <= limit && ui.add(egui::Button::new(link_text(tr("Ver histórico completo →", "See full history →"))).frame(false)).clicked() {
                        go_all = true;
                    }
                } else if total > limit {
                    if ui.button(tf!("mostrar mais ({} restantes)", "show more ({} left)", total - limit)).clicked() {
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
        use crate::i18n::t as tr;
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
                    pal::pcie(),
                    &|p: &egui::Painter, r: Rect| {
                        let rr = r.height().min(r.width()) * 0.42;
                        p.circle_filled(r.center(), rr, pal::pcie().gamma_multiply(0.18));
                        p.circle_stroke(r.center(), rr, Stroke::new(1.0, pal::pcie().gamma_multiply(0.6)));
                        p.text(r.center(), egui::Align2::CENTER_CENTER, icon, FontId::proportional(40.0 * (r.height() / 88.0).min(1.0)), pal::fg());
                    },
                    name,
                    tr("Como funciona", "How it works"),
                    None,
                    &mut sel,
                );
                ui.add_space(10.0);
                self.info(ui, tp, topo, samples, bench, ctx);
            }
            Some(Sel::Gpu) => {
                let g = last.gpu.unwrap_or_default();
                let node = topo.gpu.as_ref();
                let (st, sc, kind) = match node {
                    None => (tr("Não encontrada", "Not found"), pal::amber(), 3),
                    Some(n) if n.problem != 0 => (tr("Com problema", "Problem"), pal::red(), 2),
                    Some(n) if n.dstate.is_some_and(|d| d != 1) => (tr("Dormindo", "Asleep"), pal::faint(), 1),
                    _ => (tr("Funcionando", "Working"), pal::green(), 0),
                };
                let spin = (g.usage.unwrap_or(0.0) / 100.0) as f32;
                let title = bind_num(gst.map(|s| s.name.trim()).filter(|s| !s.is_empty()).unwrap_or(tr("Placa de vídeo", "Graphics card")));
                side_hero(ui, e, pal::pcie(), &|p: &egui::Painter, r: Rect| art::gpu(p, r, spin, node.is_some(), t), &title, tr("Dentro do dock, ligada pelo Thunderbolt", "Inside the dock, linked over Thunderbolt"), Some((st, sc)), &mut sel);
                side_hint(
                    ui,
                    match kind {
                        0 => tr("Tudo normal com a placa. Clique num bloco para ver os gráficos.", "All good with the card. Click a block to see the charts."),
                        1 => tr("A placa está em economia de energia. Ela acorda sozinha quando um programa usa.", "The card is in power-saving mode. It wakes up on its own when a program uses it."),
                        2 => tr("O Windows relatou um problema com a placa. Veja os detalhes técnicos no fim.", "Windows reported a problem with the card. See the technical details at the bottom."),
                        _ => tr("A placa não apareceu no Windows. Confira se o dock está ligado e o cabo encaixado.", "The card didn't show up in Windows. Check that the dock is on and the cable is plugged in."),
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
                    tl("🌡", temp.map(|v| format!("{v:.0}")), "°C", tr("Temperatura", "Temperature"), heat(temp.unwrap_or(0.0), pal::temp())).vc(heat(temp.unwrap_or(0.0), pal::temp())).sp(ser(|g| g.temp.map(|v| v as f32))),
                    tl("🔥", hot.map(|v| format!("{v:.0}")), "°C", tr("Ponto mais quente", "Hot spot"), heat(hot.unwrap_or(0.0) - 10.0, pal::hot())).vc(heat(hot.unwrap_or(0.0) - 10.0, pal::hot())).sp(ser(|g| g.hot.map(|v| v as f32))),
                    tl("⚡", g.tbp.map(|v| format!("{v:.0}")), "W", tr("Consumo da placa", "Card power"), pal::bolt()).sp(ser(|g| g.tbp.map(|v| v as f32))),
                    tl("⏱", g.clk.map(|v| v.to_string()), "MHz", tr("Velocidade", "Clock"), pal::data()).sp(ser(|g| g.clk.map(|v| v as f32))),
                    tl("🌀", fan.map(|v| v.to_string()), "RPM", if fan == Some(0) { tr("Ventoinha parada", "Fan stopped") } else { tr("Ventoinha", "Fan") }, pal::violet()).sp(ser(|g| g.fan.map(|v| v as f32))),
                    {
                        let x = tl("💾", vram, "GB", tr("Memória de vídeo", "Video memory"), pal::data());
                        match (g.vram, vram_total) {
                            (Some(v), t) if t > 0 => x.bar(v as f32 / t as f32),
                            _ => x,
                        }
                    },
                    tl("📶", g.usage.map(|v| format!("{v:.0}")), "%", tr("Uso da GPU", "GPU usage"), pal::data()).sp(ser(|g| g.usage.map(|v| v as f32))),
                ];
                tiles.push(pcie_tile(gst, node.and_then(|n| n.pcie), neutral));
                let tiles: Vec<Tile> = tiles.into_iter().map(|x| if x.tip.is_some() { x } else { x.tip(tr("Ver no Desempenho", "See in Performance")) }).collect();
                if tile_grid(ui, &tiles, sel_at).is_some() {
                    self.tab = Tab::Fluxo;
                }
                ui.add_space(12.0);
                if ui.button(egui::RichText::new(tr("📈 Ver desempenho ›", "📈 See performance ›")).size(14.0)).clicked() {
                    self.tab = Tab::Fluxo;
                }
                side_tech(ui, "gpu", |ui| {
                    if let Some(s) = gst {
                        kv(ui, "ADLX:", format!("{} · {}\u{A0}MB · {}\u{A0}x{}", s.name, s.vram_total_mb, s.bus, s.lanes));
                        kv(ui, "LUID (PDH):", s.luid.clone().unwrap_or_default());
                    }
                    node_details(ui, "GPU", topo.gpu.as_ref());
                    for n in &topo.gpu_chain {
                        node_details(ui, tr("Switch interno da placa", "Card's internal switch"), Some(n));
                    }
                });
            }
            Some(Sel::Dock) => {
                let ok = dock_ok && topo.dock_switch.is_some() && topo.dock_router.as_ref().is_some_and(|n| n.problem == 0);
                // mesmo veredito do banner: ligado agora não quer dizer estável
                let (lvl, _, cause) = self.verdict(topo);
                let now = now_ms();
                let d24 = self.incidents.iter().filter(|i| i.is_drop && now - i.start <= 86_400_000).count();
                let pill = tf!("Caiu {}× em 24\u{A0}h", "Dropped {}× in 24\u{A0}h", d24);
                let (st, sc, icon, tile_v): (&str, Color32, &str, &str) = if !ok {
                    (tr("Com problema", "Problem"), pal::red(), "✖", tr("Com problema", "Problem"))
                } else if lvl >= 2 {
                    (&pill, pal::red(), "▲", tr("Instável", "Unstable"))
                } else if lvl == 1 {
                    (&pill, pal::amber(), "▲", tr("Instável", "Unstable"))
                } else {
                    (tr("Tudo certo", "All good"), pal::green(), "✔", tr("Estável", "Stable"))
                };
                side_hero(ui, e, pal::pcie(), &|p: &egui::Painter, r: Rect| art::dock(p, r, dock_ok, t), "Dock WKG-L19", tr("Caixa externa da GPU", "External GPU box"), Some((st, sc)), &mut sel);
                let hint = if !ok {
                    tr("Algo no dock não respondeu. Veja os detalhes técnicos no fim.", "Something in the dock didn't respond. See the technical details at the bottom.").to_string()
                } else if lvl > 0 {
                    let vezes = if d24 == 1 { tr("1 vez", "once").to_string() } else { tf!("{} vezes", "{} times", d24) };
                    tf!("Ligado agora, mas caiu {} nas últimas 24\u{A0}h. {}", "Connected now, but dropped {} in the last 24\u{A0}h. {}", vezes, cause).trim_end().to_string()
                } else {
                    tr("O dock está ligado, conversando com o notebook e sem quedas nas últimas 24\u{A0}h.", "The dock is on, talking to the laptop, with no drops in the last 24\u{A0}h.").to_string()
                };
                side_hint(ui, &hint);
                let dr = topo.dock_router.as_ref();
                let active = topo.ports.iter().filter(|p| d0(&p.node)).count();
                let mut tiles = vec![
                    tl("🔧", dr.and_then(|n| n.firmware.clone()), "", "Firmware", neutral),
                    tl("🕑", dr.and_then(|n| n.last_arrival).map(|ts| fmt_dur(now_ms() - ts)), "", tr("Conectado há", "Connected for"), neutral),
                    tl(icon, Some(tile_v.to_string()), "", if ok { tr("Nas últimas 24\u{A0}h", "Last 24\u{A0}h") } else { tr("Estado", "State") }, sc).vc(sc).tip(tr("Ver as quedas no Histórico", "See the drops in History")),
                    tl("🔌", (!topo.ports.is_empty()).then(|| tf!("{} de {}", "{} of {}", active, topo.ports.len())), "", tr("Portas ativas", "Active ports"), neutral).tip(tr("Portas acordadas (D0). As outras estão dormindo para economizar.", "Awake ports (D0). The others are asleep to save power.")),
                ];
                if let Some(b) = bench {
                    tiles.push(tl("📏", Some(gbit(b.h2d_gbs)), "Gb/s", tr("Banda medida", "Measured bandwidth"), pal::data()).tip(tr("Notebook → placa, o sentido que pesa em jogo", "Laptop → card, the direction that matters in games")));
                }
                if tile_grid(ui, &tiles, sel_at) == Some(2) {
                    self.tab = Tab::Historico;
                }
                side_caption(ui, tr("Portas", "Ports"));
                ui.horizontal_wrapped(|ui| {
                    for (i, p) in topo.ports.iter().enumerate() {
                        let name = match p.role {
                            PortRole::Gpu => "🎮 GPU",
                            PortRole::UsbA => "🖴 USB",
                            PortRole::TbExtra => "⚡ Thunderbolt",
                            PortRole::Other => tr("🔌 Porta", "🔌 Port"),
                        };
                        let dot = if d0(&p.node) { pal::green() } else { pal::faint() };
                        if side_chip(ui, name, Some(dot), true).on_hover_text(if d0(&p.node) { tr("Ativa", "Active") } else { tr("Dormindo", "Asleep") }).clicked() {
                            sel = Some(Sel::Port(i));
                        }
                    }
                });
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if lvl > 0 && side_chip(ui, tr("📜 Ver as quedas", "📜 See the drops"), None, true).clicked() {
                        self.tab = Tab::Historico;
                    }
                    if side_chip(ui, tr("⚡ Ver o cabo", "⚡ See the cable"), None, true).clicked() {
                        sel = Some(Sel::Cable);
                    }
                });
                side_tech(ui, "dock", |ui| {
                    node_details(ui, tr("Roteador Thunderbolt do dock", "Dock Thunderbolt router"), topo.dock_router.as_ref());
                    usb4_table(ui, tr("Chaves USB4 do dock {5DF7E321-…}", "Dock USB4 keys {5DF7E321-…}"), &topo.dock_usb4);
                    node_details(ui, tr("Switch PCIe do dock (upstream)", "Dock PCIe switch (upstream)"), topo.dock_switch.as_ref());
                    for (i, p) in topo.ports.iter().enumerate() {
                        node_details(ui, &tf!("Porta downstream {} — {:?}", "Downstream port {} — {:?}", i + 1, p.role), Some(&p.node));
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
                    (false, _) => (tr("Sem conexão", "No connection"), pal::red()),
                    (true, Some(0x8)) => (tr("Abaixo do máximo", "Below maximum"), pal::amber()),
                    (true, Some(0x4)) => (tr("Na velocidade máxima", "At full speed"), pal::green()),
                    _ => (tr("Conectado", "Connected"), pal::green()),
                };
                side_hero(ui, e, pal::bolt(), &|p: &egui::Painter, r: Rect| art::usb_c(p, r, dock_ok), tr("Cabo Thunderbolt", "Thunderbolt cable"), tr("Liga o notebook ao dock", "Links the laptop to the dock"), Some((st, sc)), &mut sel);
                side_hint(
                    ui,
                    match (dock_ok, speed) {
                        (false, _) => tr("O notebook não está enxergando o dock. Confira os dois lados do cabo.", "The laptop can't see the dock. Check both ends of the cable."),
                        (true, Some(0x8)) => tr("O dock aceita 40\u{A0}Gb/s, mas o cabo está entregando a metade. Um cabo Thunderbolt certificado de 40\u{A0}Gb/s resolve.", "The dock supports 40\u{A0}Gb/s, but the cable is delivering half. A certified 40\u{A0}Gb/s Thunderbolt cable fixes it."),
                        _ => tr("O cabo leva os dados da GPU e traz energia para o notebook.", "The cable carries the GPU's data and brings power to the laptop."),
                    },
                );
                let pd = last.power.map(|p| p.online);
                let tiles = vec![
                    tl("⚡", gbps.map(|v| v.to_string()), "Gb/s", tr("Velocidade do cabo", "Cable speed"), pal::bolt()).vc_if(speed == Some(0x8), pal::amber()).note(tr("≈ decodificado", "≈ decoded")).tip(tr("Lido das chaves USB4\u{A0}20 e 21 do dock. Veja os detalhes técnicos.", "Read from the dock's USB4\u{A0}20 and 21 keys. See the technical details.")),
                    tl("⛓", lanes.map(|v| v.to_string()), "", tr("Canais ativos", "Active lanes"), neutral),
                    tl("📏", bench.map(|b| format!("↑{} ↓{}", gbit(b.h2d_gbs), gbit(b.d2h_gbs))), "Gb/s", tr("Banda medida", "Measured bandwidth"), pal::data()),
                    tl("🔋", pd.map(|o| if o { tr("Sim", "Yes") } else { tr("Não", "No") }.to_string()), "", tr("Carregando o notebook", "Charging the laptop"), pal::bolt()).vc_if(pd == Some(false), pal::amber()),
                ];
                tile_grid(ui, &tiles, sel_at);
                ui.add_space(12.0);
                let running = matches!(*self.bench.lock().unwrap(), BenchState::Running(_));
                if ui.add_enabled(!running, egui::Button::new(egui::RichText::new(if running { tr("📏 Medindo…", "📏 Measuring…") } else { tr("📏 Medir banda agora", "📏 Measure bandwidth now") }).size(14.0))).clicked() {
                    self.start_bench(ctx);
                }
                side_tech(ui, "cable", |ui| {
                    usb4_table(ui, tr("Chaves USB4 do dock {5DF7E321-…}", "Dock USB4 keys {5DF7E321-…}"), &topo.dock_usb4);
                    node_details(ui, tr("Roteador Thunderbolt do dock", "Dock Thunderbolt router"), topo.dock_router.as_ref());
                    usb4_table(ui, tr("Chaves USB4 do host", "Host USB4 keys"), &topo.host_usb4);
                });
            }
            Some(Sel::Host) => {
                let ctrl = topo.host_ctrl.as_ref();
                let ok = ctrl.is_some_and(|n| n.problem == 0);
                let (st, sc) = if ok { (tr("Funcionando", "Working"), pal::green()) } else { (tr("Com problema", "Problem"), pal::red()) };
                side_hero(ui, e, pal::pcie(), &|p: &egui::Painter, r: Rect| art::laptop(p, r, true, t), tr("Notebook", "Laptop"), tr("Porta Thunderbolt\u{A0}4", "Thunderbolt\u{A0}4 port"), Some((st, sc)), &mut sel);
                side_hint(ui, if ok { tr("A porta Thunderbolt do notebook está pronta e levando a GPU.", "The laptop's Thunderbolt port is ready and carrying the GPU.") } else { tr("O controlador da porta Thunderbolt não respondeu. Veja os detalhes técnicos.", "The Thunderbolt port controller didn't respond. See the technical details.") });
                let pw = last.power;
                let pct = pw.filter(|p| p.full_mwh > 0).map(|p| (p.rem_mwh as f64 * 100.0 / p.full_mwh as f64).round());
                let tiles = vec![
                    tl("🔧", topo.host_router.as_ref().and_then(|n| n.firmware.clone()), "", "Firmware", neutral),
                    tl(if ok { "✔" } else { "⚠" }, Some(if ctrl.is_some() { tr("Funcionando", "Working") } else { tr("Ausente", "Missing") }.to_string()), "", tr("Controlador USB4", "USB4 controller"), sc).vc(sc),
                    // a porta raiz do túnel não publica enlace no Windows: usa o do driver AMD (mesmo túnel)
                    tl(
                        "🔗",
                        topo.root_port.as_ref().and_then(|n| n.pcie).map(|(cs, cw, _, _)| pcie_long(cs, cw)).or_else(|| gst.filter(|s| s.lanes > 0).map(|s| format!("{}\u{A0}x{}", s.bus, s.lanes))),
                        "",
                        tr("Túnel até a GPU", "Tunnel to the GPU"),
                        neutral,
                    )
                    .tip(tr("O PCIe da GPU passa por dentro do cabo Thunderbolt. É o trecho mais estreito do caminho.", "The GPU's PCIe runs inside the Thunderbolt cable. It's the narrowest stretch of the path.")),
                    tl("🔋", pw.map(|p| if p.online { tr("Na tomada", "Plugged in") } else { tr("Na bateria", "On battery") }.to_string()), &pct.map(|v| format!("{v:.0}%")).unwrap_or_default(), tr("Energia", "Power"), pal::bolt())
                        .vc_if(pw.is_some_and(|p| !p.online), pal::amber())
                        .sp(samples.iter().filter_map(|s| s.power.filter(|p| p.full_mwh > 0).map(|p| p.rem_mwh as f32 * 100.0 / p.full_mwh as f32)).collect()),
                ];
                tile_grid(ui, &tiles, sel_at);
                ui.add_space(8.0);
                if side_chip(ui, tr("⚡ Ver o cabo", "⚡ See the cable"), None, true).clicked() {
                    sel = Some(Sel::Cable);
                }
                side_tech(ui, "host", |ui| {
                    node_details(ui, tr("Controlador USB4 do note", "Laptop USB4 controller"), topo.host_ctrl.as_ref());
                    node_details(ui, tr("Roteador raiz USB4", "USB4 root router"), topo.host_router.as_ref());
                    usb4_table(ui, tr("Chaves USB4 do host", "Host USB4 keys"), &topo.host_usb4);
                    node_details(ui, tr("Porta raiz PCIe (túnel)", "PCIe root port (tunnel)"), topo.root_port.as_ref());
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
                    PortRole::Gpu => (tr("Porta da GPU", "GPU port"), tr("Onde a placa de vídeo encaixa no dock", "Where the graphics card plugs into the dock")),
                    PortRole::UsbA => (tr("Porta USB", "USB port"), tr("As entradas USB do dock", "The dock's USB inputs")),
                    PortRole::TbExtra => (tr("Porta Thunderbolt extra", "Extra Thunderbolt port"), tr("Saída para outro aparelho Thunderbolt", "Output to another Thunderbolt device")),
                    PortRole::Other => (tr("Porta do dock", "Dock port"), tr("Saída interna do dock", "Internal dock output")),
                };
                let (st, sc) = if p.node.problem != 0 { (tr("Com problema", "Problem"), pal::red()) } else if on { (tr("Ativa", "Active"), pal::green()) } else { (tr("Dormindo", "Asleep"), pal::faint()) };
                let role = p.role.clone();
                side_hero(
                    ui,
                    e,
                    pal::pcie(),
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
                side_hint(ui, if on { tr("Ligada e em uso.", "On and in use.") } else { tr("Dormindo para economizar energia. Ela acorda quando algo for ligado nela.", "Asleep to save power. It wakes up when something is plugged in.") });
                let mut tiles = vec![
                    tl(if on { "✔" } else { "💤" }, Some(if on { tr("Ativa", "Active") } else { tr("Dormindo", "Asleep") }.to_string()), "", tr("Estado", "State"), sc).vc(sc),
                    {
                        let link = p.node.pcie;
                        let mut x = tl("🔗", link.map(|(cs, cw, _, _)| pcie_short(cs, cw)), "", tr("Enlace PCIe", "PCIe link"), neutral);
                        if let Some((cs, cw, ms, mw)) = link {
                            if cs < ms || cw < mw {
                                x = x.note(tf!("máx {}", "max {}", pcie_short(ms, mw)));
                            }
                        }
                        x
                    },
                ];
                if p.role == PortRole::UsbA {
                    tiles.push(tl("🖴", Some(topo.usb_a.len().to_string()), "", tr("Dispositivos USB", "USB devices"), neutral));
                }
                tile_grid(ui, &tiles, sel_at);
                if p.role == PortRole::UsbA && !topo.usb_a.is_empty() {
                    side_caption(ui, tr("Ligados agora", "Connected now"));
                    ui.horizontal_wrapped(|ui| {
                        for d in &topo.usb_a {
                            side_chip(ui, &format!("🖴 {}", d.name), None, false);
                        }
                    });
                }
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if p.role == PortRole::Gpu && side_chip(ui, tr("🎮 Ver a placa", "🎮 See the card"), None, true).clicked() {
                        sel = Some(Sel::Gpu);
                    }
                    if side_chip(ui, tr("🔗 Ver o dock", "🔗 See the dock"), None, true).clicked() {
                        sel = Some(Sel::Dock);
                    }
                });
                side_tech(ui, &format!("port{i}"), |ui| {
                    node_details(ui, tr("Porta downstream do switch", "Switch downstream port"), Some(&p.node));
                    node_details(ui, tr("Dispositivo na porta", "Device on the port"), p.child.as_ref());
                    if p.role == PortRole::UsbA {
                        for d in &topo.usb_a {
                            node_details(ui, tr("Dispositivo USB", "USB device"), Some(d));
                        }
                    }
                });
            }
            Some(Sel::Psu) => match self.psu_load.filter(|_| self.psu.watts > 0) {
                Some(l) => {
                    let w = self.psu.watts as f32;
                    let f = l.frac.clamp(0.0, 1.0);
                    let st = psu_state(&l, self.psu.watts);
                    let model = self.psu.model.trim().to_string();
                    let rating = self.psu.rating.clone();
                    let sub = if rating.trim().is_empty() { tf!("{}\u{A0}W · sem selo", "{}\u{A0}W · no rating", self.psu.watts) } else { format!("{}\u{A0}W · 80 PLUS {}", self.psu.watts, rating.trim()) };
                    side_hero(
                        ui,
                        e,
                        pal::bolt(),
                        &|p: &egui::Painter, r: Rect| art::psu(p, r, dock_ok, t, &rating, f),
                        if model.is_empty() { tr("Fonte do dock", "Dock PSU") } else { &model },
                        &sub,
                        Some((st.pill, st.col)),
                        &mut sel,
                    );
                    side_hint(ui, &st.msg);
                    let rail = self.psu.rail12_a;
                    let tiles = vec![
                        tl("⚡", Some(format!("{:.0}", l.now)), "W", tr("Carga agora (≈)", "Load now (≈)"), pal::bolt()).vc_if(f > 0.85, pal::amber()).bar(f).tip(tf!(
                            "≈ {:.0}\u{A0}W = placa {:.0}\u{A0}W (medido) + notebook {:.0}\u{A0}W + dock {:.0}\u{A0}W + portas {:.0}\u{A0}W (estimados)",
                            "≈ {:.0}\u{A0}W = card {:.0}\u{A0}W (measured) + laptop {:.0}\u{A0}W + dock {:.0}\u{A0}W + ports {:.0}\u{A0}W (estimated)",
                            l.now, l.gpu, l.pd, l.dock, l.usb
                        )),
                        tl("📈", Some(format!("{:.0}", l.worst)), "W", tr("Uso pesado (≈)", "Heavy use (≈)"), pal::bolt()).vc_if(l.worst > w * 0.85, pal::amber()).bar((l.worst / w).min(1.0)).tip(tr("Sustentado: placa no limite de potência + notebook carregando no máximo + portas em uso.", "Sustained: card at its power limit + laptop charging at max + ports in use.")),
                        tl("▼", Some(format!("{:.0}", l.spike)), "W", tr("Picos rápidos (≈)", "Quick spikes (≈)"), pal::bolt()).vc_if(l.spike > w, pal::amber()).bar((l.spike / w).min(1.0)).tip(tr("Transientes de milissegundos da placa (~1,6× o limite) somados ao resto.", "Millisecond transients from the card (~1.6× the limit) added to the rest.")),
                        tl("🔌", Some(format!("{:.0}", l.wall)), "W", tr("Na tomada (≈)", "At the wall (≈)"), pal::data()).bar((l.wall / w).min(1.0)).tip(tf!("Eficiência estimada {:.0}%", "Estimated efficiency {:.0}%", l.eff * 100.0)),
                        if rail > 0.0 {
                            tl("⎓", Some(dec(l.amps12 as f64, 1)), "A", tr("Linha +12\u{A0}V (≈)", "+12\u{A0}V rail (≈)"), pal::bolt()).vc_if(l.amps12 > rail * 0.85, pal::amber()).bar((l.amps12 / rail).min(1.0)).tip(tf!("Contra os {}\u{A0}A da etiqueta", "Against the {}\u{A0}A on the label", dec(rail as f64, 1)))
                        } else {
                            tl("⎓", None, "A", tr("+12\u{A0}V: falta informar", "+12\u{A0}V: not set"), pal::faint()).bar(0.0).tip(tr("Preencha a corrente da linha +12\u{A0}V (está na etiqueta da fonte).", "Fill in the +12\u{A0}V rail current (it's on the PSU label)."))
                        },
                    ];
                    tile_grid(ui, &tiles, sel_at);
                    ui.add_space(12.0);
                    if ui.button(egui::RichText::new(tr("✏ Trocar fonte ›", "✏ Change PSU ›")).size(14.0)).clicked() {
                        self.tab = Tab::Enlace;
                    }
                    side_tech(ui, "psu", |ui| {
                        ui.label(egui::RichText::new(tr("De onde vem a carga agora", "Where the load comes from now")).size(13.0).strong().color(pal::dim()));
                        ui.horizontal_wrapped(|ui| {
                            for (icon, v, tip) in [("🎮", l.gpu, tr("Placa de vídeo (medida)", "Graphics card (measured)")), ("💻", l.pd, tr("Notebook pelo PD (≈ estimado)", "Laptop via PD (≈ estimated)")), ("▣", l.dock + l.usb, tr("Dock e portas (≈ estimado)", "Dock and ports (≈ estimated)"))] {
                                egui::Frame::new()
                                    .fill(pal::card2())
                                    .stroke(Stroke::new(1.0, pal::border()))
                                    .corner_radius(CornerRadius::same(pal::radius()))
                                    .inner_margin(egui::Margin::symmetric(10, 5))
                                    .show(ui, |ui| {
                                        ui.add(egui::Label::new(egui::RichText::new(format!("{icon} {v:.0}\u{A0}W")).size(14.0).color(pal::fg())).extend());
                                    })
                                    .response
                                    .on_hover_text(tip);
                            }
                            ui.add(egui::Label::new(egui::RichText::new(format!("= ≈ {:.0}\u{A0}W", l.now)).size(14.0).strong().color(pal::fg())).extend());
                        });
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(tf!(
                            "Eficiência ≈ {:.0}% · +12\u{A0}V ≈ {}\u{A0}A · uso pesado = placa no limite ({:.0}\u{A0}W) + notebook a 85\u{A0}W + dock e portas.",
                            "Efficiency ≈ {:.0}% · +12\u{A0}V ≈ {}\u{A0}A · heavy use = card at its limit ({:.0}\u{A0}W) + laptop at 85\u{A0}W + dock and ports.",
                            l.eff * 100.0,
                            dec(l.amps12 as f64, 1),
                            crate::psu::GPU_TBP_MAX
                        )).size(12.5).color(pal::dim()));
                        ui.label(egui::RichText::new(tr("A fonte não manda dados ao PC: só a placa é medida, o resto é estimado.", "The PSU doesn't send data to the PC: only the card is measured, the rest is estimated.")).size(12.5).color(pal::faint()));
                    });
                }
                None => self.sel = None,
            },
            None => {}
        }
        if sel != before {
            self.sel = sel;
        }
    }

    /// Painéis "Como funciona": 2–4 blocos com ícone e frase curta, a explicação técnica recolhida.
    fn info(&mut self, ui: &mut Ui, t: Topic, topo: &Topology, samples: &[Sample], bench: Option<&BenchResult>, ctx: &egui::Context) {
        use crate::i18n::t as tr;
        // texto técnico (o que antes ficava aberto), agora dentro de "Detalhes técnicos"
        let para = |ui: &mut Ui, s: &str| {
            ui.label(egui::RichText::new(s).size(13.0).color(pal::dim()));
            ui.add_space(6.0);
        };
        let last = samples.last().copied().unwrap_or_default();
        let running = matches!(*self.bench.lock().unwrap(), BenchState::Running(_));
        match t {
            Topic::Link => {
                let speed = topo.dock_usb4.iter().find(|p| p.pid == 20).and_then(|p| p.val.as_u32());
                let ln = topo.dock_usb4.iter().find(|p| p.pid == 21).and_then(|p| p.val.as_u32()).unwrap_or(2).max(1);
                match speed {
                    Some(0x8) => info_block(ui, "⚡", pal::amber(), &tf!("Cabo entrega {} de 40\u{A0}Gb/s", "Cable delivers {} of 40\u{A0}Gb/s", ln * 10), tr("≈ decodificado das chaves do dock: metade do que ele aceita", "≈ decoded from the dock's keys: half of what it supports")),
                    Some(0x4) => info_block(ui, "⚡", pal::green(), &tf!("Cabo entrega {}\u{A0}Gb/s", "Cable delivers {}\u{A0}Gb/s", ln * 20), tr("≈ decodificado das chaves do dock: velocidade máxima", "≈ decoded from the dock's keys: full speed")),
                    _ => info_block(ui, "⚡", pal::faint(), tr("Velocidade do cabo desconhecida", "Cable speed unknown"), tr("O dock não publicou as chaves de velocidade", "The dock did not publish its speed keys")),
                }
                match bench {
                    Some(b) => info_block(
                        ui,
                        "📏",
                        pal::data(),
                        &tf!("Banda medida ↑{} ↓{}\u{A0}Gb/s", "Measured bandwidth ↑{} ↓{}\u{A0}Gb/s", gbit(b.h2d_gbs), gbit(b.d2h_gbs)),
                        tr("Com 40\u{A0}Gb/s o normal é ≈ 20–22\u{A0}Gb/s; ≈ 13–15 é típico de 20\u{A0}Gb/s", "At 40\u{A0}Gb/s expect ≈ 20–22\u{A0}Gb/s; ≈ 13–15 is typical of 20\u{A0}Gb/s"),
                    ),
                    None => info_block(ui, "📏", pal::faint(), tr("Banda ainda não medida", "Bandwidth not measured yet"), tr("Abaixo de ≈ 18\u{A0}Gb/s confirma o enlace reduzido", "Below ≈ 18\u{A0}Gb/s confirms the reduced link")),
                }
                info_block(ui, "🔍", pal::bolt(), tr("Como provar: troque por um cabo 40\u{A0}Gb/s certificado", "To prove it: swap in a certified 40\u{A0}Gb/s cable"), tr("Thunderbolt\u{A0}3/4 passivo, até 0,8 m. A banda deve subir para ≈ 20\u{A0}Gb/s", "Passive Thunderbolt\u{A0}3/4, up to 0.8 m. Bandwidth should rise to ≈ 20\u{A0}Gb/s"));
                ui.add_space(4.0);
                if ui.add_enabled(!running, egui::Button::new(egui::RichText::new(if running { tr("📏 Medindo…", "📏 Measuring…") } else { tr("📏 Medir banda agora", "📏 Measure bandwidth now") }).size(14.0))).clicked() {
                    self.start_bench(ctx);
                }
                side_tech(ui, "info-link", |ui| {
                    para(ui, tr("O Windows publica no roteador do dock propriedades USB4 sem documentação ({5DF7E321-…}). Comparando dock e host com os IDs de hardware, deu para decodificar várias delas.", "Windows publishes undocumented USB4 properties on the dock's router ({5DF7E321-…}). Comparing dock and host by hardware IDs, several of them could be decoded."));
                    para(ui, tr("A chave 20 vale 0x8. No spec USB4 (Lane Adapter CS1) esse campo vale 0x8 = Gen2 (10\u{A0}Gb/s por faixa) e 0x4 = Gen3 (20\u{A0}Gb/s). A chave 21 é o número de faixas. Se a chave 20 virar 0x4 com outro cabo, a decodificação fica provada.", "Key 20 reads 0x8. In the USB4 spec (Lane Adapter CS1) this field is 0x8 = Gen2 (10\u{A0}Gb/s per lane) and 0x4 = Gen3 (20\u{A0}Gb/s). Key 21 is the lane count. If key 20 turns into 0x4 with another cable, the decoding is proven."));
                    usb4_table(ui, tr("Chaves USB4 do dock", "Dock USB4 keys"), &topo.dock_usb4);
                });
            }
            Topic::Power => {
                match last.power {
                    Some(pw) => {
                        let pct = (pw.full_mwh > 0).then(|| tf!(" · bateria {:.0}%", " · battery {:.0}%", pw.rem_mwh as f64 * 100.0 / pw.full_mwh as f64));
                        if pw.online {
                            info_block(ui, "🔌", pal::green(), &tf!("Na tomada{}", "Plugged in{}", pct.unwrap_or_default()), tr("O dock está carregando o notebook pelo cabo", "The dock is charging the laptop over the cable"));
                        } else {
                            info_block(ui, "🔋", pal::amber(), &tf!("Na bateria{}", "On battery{}", pct.unwrap_or_default()), tr("O notebook não está recebendo energia do dock", "The laptop isn't getting power from the dock"));
                        }
                    }
                    None => info_block(ui, "🔌", pal::faint(), tr("Energia do notebook desconhecida", "Laptop power unknown"), tr("O Windows não informou o estado da bateria", "Windows didn't report the battery state")),
                }
                info_block(ui, "💡", pal::bolt(), tr("Energia caiu junto? O dock inteiro apagou", "Power dropped too? The whole dock went dark"), tr("É a pista mais útil numa queda: aponta para fonte ou tomada", "It's the most useful clue in a drop: it points to the PSU or the outlet"));
                info_block(ui, "🙈", pal::faint(), tr("Os watts do PD não chegam ao Windows", "PD watts never reach Windows"), tr("Tensão e corrente ficam no controlador do dock", "Voltage and current stay in the dock's controller"));
                let series: Vec<f32> = samples.iter().filter_map(|s| s.power).map(|p| p.mv as f32 / 1000.0).collect();
                chart(ui, tr("tensão da bateria (V)", "battery voltage (V)"), &[(pal::bolt(), "V", series)], 13.0, " V", 70.0);
                side_tech(ui, "info-power", |ui| {
                    para(ui, tr("Fonte: WMI root\\wmi BatteryStatus (PowerOnline, ChargeRate, DischargeRate, Voltage) — é o que o driver de bateria ACPI publica.", "Source: WMI root\\wmi BatteryStatus (PowerOnline, ChargeRate, DischargeRate, Voltage) — what the ACPI battery driver publishes."));
                    para(ui, tr("Com a bateria cheia, a taxa de carga é zero e o PD alimenta só o consumo do sistema. Nesse caso o Windows não informa quantos watts o dock entrega.", "With a full battery, the charge rate is zero and PD only feeds the system's consumption. In that case Windows doesn't report how many watts the dock delivers."));
                    para(ui, tr("A tensão/corrente negociadas no contrato PD (por ex. 20\u{A0}V × 4,25\u{A0}A = 85\u{A0}W) ficam no controlador PD do dock e não chegam ao Windows.", "The voltage/current negotiated in the PD contract (e.g. 20\u{A0}V × 4.25\u{A0}A = 85\u{A0}W) stay in the dock's PD controller and never reach Windows."));
                });
            }
            Topic::Stability => {
                let (lvl, _, _) = self.verdict(topo);
                let now = now_ms();
                let d24 = self.incidents.iter().filter(|i| i.is_drop && now - i.start <= 86_400_000).count();
                let col = match lvl {
                    0 => pal::green(),
                    1 => pal::amber(),
                    _ => pal::red(),
                };
                if d24 > 0 {
                    info_block(ui, "🛡", col, &tf!("Caiu {}× nas últimas 24\u{A0}h", "Dropped {}× in the last 24\u{A0}h", d24), tr("Clique numa queda no Histórico para ver o que veio antes", "Click a drop in History to see what came before"));
                } else {
                    info_block(ui, "🛡", col, tr("Nenhuma queda nas últimas 24\u{A0}h", "No drops in the last 24\u{A0}h"), tr("O painel registra cada queda, mesmo com a janela fechada", "The panel logs every drop, even with the window closed"));
                }
                info_block(ui, "🔋", pal::bolt(), tr("Energia caiu junto → fonte ou tomada", "Power dropped too → PSU or outlet"), tr("O dock inteiro apagou ou reiniciou", "The whole dock went dark or restarted"));
                info_block(ui, "⛓", pal::data(), tr("Só a conexão caiu → cabo ou conector", "Only the connection dropped → cable or connector"), tr("A energia continuou chegando, só o túnel caiu", "Power kept coming, only the tunnel dropped"));
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if side_chip(ui, tr("📜 Ver o histórico", "📜 See history"), None, true).clicked() {
                        self.tab = Tab::Historico;
                    }
                    if side_chip(ui, tr("📄 Exportar CSV", "📄 Export CSV"), None, true).clicked() {
                        match crate::config::export_csv() {
                            Ok(p) => {
                                crate::config::open_folder(p.parent().unwrap_or(&p));
                                self.flash = Some(tf!("Exportado: {}", "Exported: {}", p.display()));
                            }
                            Err(e) => self.flash = Some(tf!("Falha: {}", "Failed: {}", e)),
                        }
                    }
                });
                side_tech(ui, "info-stab", |ui| {
                    para(ui, tr("Uma queda é agrupada com tudo o que aconteceu até 20\u{A0}s depois (switch PCIe, roteador TB, GPU, energia).", "A drop is grouped with everything that happened up to 20\u{A0}s later (PCIe switch, TB router, GPU, power)."));
                    para(ui, tr("🔋 PD caiu junto: o note perdeu a energia externa no mesmo instante — o dock inteiro apagou ou resetou (fonte ATX, controlador do dock ou PD), não só o túnel.", "🔋 PD dropped too: the laptop lost external power at the same moment — the whole dock went dark or reset (ATX PSU, dock controller or PD), not just the tunnel."));
                    para(ui, tr("⛓ Só o enlace: a energia continuou chegando e só o túnel/PCIe caiu — cabo, conector ou o próprio controlador TB.", "⛓ Link only: power kept coming and only the tunnel/PCIe dropped — cable, connector or the TB controller itself."));
                    para(ui, tr("💥 \"Falha\" = Kernel-PnP 1011: o Windows relatou o dispositivo como falhando. ⏏ \"Removido\" = 1010: sumiu do barramento (desconexão física ou apagão).", "💥 \"Failure\" = Kernel-PnP 1011: Windows reported the device as failing. ⏏ \"Removed\" = 1010: it vanished from the bus (physical disconnect or blackout)."));
                });
            }
            Topic::Flow => {
                info_block(ui, "⇄", pal::data(), &tf!("Tráfego agora ≈ {:.0}%", "Traffic now ≈ {:.0}%", last.copy), tr("Uso do motor de cópia da GPU: quanto o cabo está ocupado", "GPU copy engine usage: how busy the cable is"));
                info_block(ui, "🚚", pal::data(), tr("O motor de cópia da GPU mostra o tráfego", "The GPU copy engine shows the traffic"), tr("O Windows não conta bytes no túnel; quando ele copia, o cabo trabalha", "Windows doesn't count bytes in the tunnel; when it copies, the cable is working"));
                info_block(ui, "💫", pal::faint(), tr("Pulsos mais rápidos = mais dados", "Faster pulses = more data"), tr("Na topologia, os pulsos do cabo seguem esse uso", "On the topology map, the cable pulses follow this usage"));
                side_tech(ui, "info-flow", |ui| {
                    para(ui, tr("Nenhum contador do Windows mede bytes no túnel Thunderbolt. O motor Copy da GPU executa as cópias DMA entre a memória do note e a VRAM; quando ele está ocupado, o túnel está ocupado.", "No Windows counter measures bytes in the Thunderbolt tunnel. The GPU's Copy engine runs the DMA copies between laptop memory and VRAM; when it's busy, the tunnel is busy."));
                    para(ui, tr("Os contadores vêm do PDH: GPU Engine (Utilization Percentage, filtrado pelo LUID da Radeon) e GPU Adapter Memory.", "The counters come from PDH: GPU Engine (Utilization Percentage, filtered by the Radeon's LUID) and GPU Adapter Memory."));
                });
            }
            Topic::Gpu => {
                info_block(ui, "🎮", pal::data(), tr("Dados lidos do driver AMD", "Data read from the AMD driver"), tr("Temperatura, consumo, ventoinha e memória, ao vivo", "Temperature, power, fan and memory, live"));
                info_block(ui, "🍃", pal::faint(), tr("Só com a janela aberta", "Only while the window is open"), tr("O driver é liberado ao fechar: a bandeja fica leve", "The driver is released on close: the tray stays light"));
                ui.add_space(4.0);
                let mut s = self.sel;
                if side_chip(ui, tr("🎮 Ver a placa", "🎮 See the card"), None, true).clicked() {
                    s = Some(Sel::Gpu);
                }
                self.sel = s;
                side_tech(ui, "info-gpu", |ui| {
                    para(ui, tr("Dados do AMD ADLX (amdadlx64.dll), carregado só com a janela aberta e liberado ao fechar.", "Data from AMD ADLX (amdadlx64.dll), loaded only while the window is open and released on close."));
                });
            }
            Topic::Hidden => {
                info_block(ui, "🚫", pal::faint(), &tf!("{} coisas que o Windows não enxerga", "{} things Windows can't see", hidden().len()), tr("Passe o mouse em cada uma para saber o porquê", "Hover each one to see why"));
                ui.add_space(2.0);
                // grade de 2 colunas, células do mesmo tamanho (nada vaza nem fica torto)
                let avail = ui.available_width();
                let cw = ((avail - 8.0) / 2.0).floor();
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    for pair in hidden().chunks(2) {
                        ui.horizontal(|ui| {
                            for (icon, name, why) in pair {
                                let (r, resp) = ui.allocate_exact_size(vec2(cw, 34.0), Sense::hover());
                                let rad = CornerRadius::same(pal::radius().min(8));
                                ui.painter().rect(r, rad, pal::well(), Stroke::new(1.0, pal::border()), StrokeKind::Inside);
                                let ic = pos2(r.left() + 18.0, r.center().y);
                                ui.painter().text(ic, egui::Align2::CENTER_CENTER, *icon, FontId::proportional(15.0), pal::faint());
                                // risco diagonal: "não disponível"
                                ui.painter().line_segment([ic + vec2(-7.0, 7.0), ic + vec2(7.0, -7.0)], Stroke::new(1.0, pal::dim().gamma_multiply(0.7)));
                                ui.painter().text(pos2(r.left() + 34.0, r.center().y), egui::Align2::LEFT_CENTER, *name, FontId::proportional(13.5), pal::fg());
                                resp.on_hover_text(why.to_string());
                            }
                        });
                    }
                });
                ui.add_space(8.0);
                info_block(ui, "✅", pal::data(), tr("O que dá para ver, o painel mostra", "What can be seen, the panel shows"), tr("Quedas, energia do notebook, GPU e a velocidade do cabo", "Drops, laptop power, GPU and cable speed"));
                side_tech(ui, "info-hidden", |ui| {
                    for (icon, name, why) in hidden() {
                        ui.label(egui::RichText::new(format!("{icon} {name}")).size(13.0).strong().color(pal::fg()));
                        para(ui, why);
                    }
                });
            }
        }
    }
}

/// Bloco de explicação: ícone num círculo tingido, frase curta em destaque e uma linha de apoio.
fn info_block(ui: &mut Ui, icon: &str, color: Color32, title: &str, sub: &str) {
    egui::Frame::new()
        .fill(pal::card2())
        .corner_radius(CornerRadius::same(pal::radius().min(8)))
        .inner_margin(egui::Margin::same(10))
        .shadow(pal::elev(2, 0.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (r, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 17.0, color.gamma_multiply(0.16));
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, icon, FontId::proportional(18.0), color);
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(egui::Label::new(egui::RichText::new(title).size(15.0).color(pal::fg())).wrap());
                    if !sub.is_empty() {
                        ui.add(egui::Label::new(egui::RichText::new(sub).size(13.0).color(pal::dim())).wrap());
                    }
                });
            });
        });
    ui.add_space(8.0);
}

// ---------------- peças do painel lateral (hero, blocos, faixas)

/// Cor de calor: a cor da série (`ok`) no normal; âmbar acima de 70 °C, vermelho acima de 85 °C.
fn heat(t: f32, ok: Color32) -> Color32 {
    if t > 85.0 { pal::red() } else if t > 70.0 { pal::amber() } else { ok }
}

/// Ícone desenhado (não é glifo): funil, o gargalo do caminho.
const FUNNEL: &str = "\u{1}funil";

/// "PCIe 4.0 x8" a partir do código de velocidade do Windows (1..5) e da largura.
fn pcie_long(code: u32, w: u32) -> String {
    format!("PCIe\u{A0}{code}.0\u{A0}x{w}")
}

/// Banda de um enlace PCIe no papel, em GB/s (por faixa: 0,25 · 0,5 · 0,985 · 1,97 · 3,94).
fn pcie_gbs(g: u32, w: u32) -> f32 {
    let lane = match g {
        1 => 0.25,
        2 => 0.5,
        3 => 0.985,
        4 => 1.969,
        5 => 3.938,
        _ => 0.0,
    };
    lane * w as f32
}

/// Geração a partir do texto do ADLX ("PCIe 3.0" → 3).
fn adlx_gen(bus: &str) -> Option<u32> {
    bus.strip_prefix("PCIe ")?.split('.').next()?.parse().ok()
}

/// Bloco do caminho PCIe da GPU, sem números que se contradizem. São dois enlaces diferentes:
/// · ADLX (driver AMD): a placa no slot do dock, que segue pelo túnel Thunderbolt até o notebook
///   (no WKG-L19, PCIe 3.0 x4). É o trecho estreito: leva o funil de gargalo.
/// · Windows (DEVPKEY no chip da GPU): o chip com o switch PCIe interno da própria placa
///   (Gen4 x8 na RX 7600 XT). Fica dentro da placa e não limita nada.
fn pcie_tile(gst: Option<&crate::collect::GpuStatic>, chip: Option<(u32, u32, u32, u32)>, neutral: Color32) -> Tile {
    let tun = gst.filter(|s| s.lanes > 0).and_then(|s| adlx_gen(&s.bus).map(|g| (g, s.lanes)));
    let inner = chip.map(|(cs, cw, _, _)| (cs, cw));
    match (tun, inner) {
        (Some((tg, tw)), inner) => {
            let tb = pcie_gbs(tg, tw);
            let bottleneck = inner.is_none_or(|(g, w)| pcie_gbs(g, w) > tb);
            let mut x = tl(if bottleneck { FUNNEL } else { "🔗" }, Some(pcie_long(tg, tw)), "", t("Túnel até o notebook", "Tunnel to the laptop"), neutral);
            if let Some((g, w)) = inner {
                x = x.note(tf!("Dentro da placa: {}", "Inside the card: {}", pcie_long(g, w)));
            }
            let mut tip = tf!(
                "Túnel até o notebook · {} (lido do driver AMD): a placa no slot do dock, que segue pelo cabo Thunderbolt até o notebook. No papel, ≈ {}\u{A0}Gb/s.",
                "Tunnel to the laptop · {} (read from the AMD driver): the card in the dock slot, which continues over the Thunderbolt cable to the laptop. On paper, ≈ {}\u{A0}Gb/s.",
                pcie_long(tg, tw),
                gbit(tb as f64)
            );
            if let Some((g, w)) = inner {
                tip.push_str(&tf!(
                    "\n\nDentro da placa · {} (lido do Windows): o chip da GPU ligado ao switch PCIe da própria placa, ≈ {}\u{A0}Gb/s.",
                    "\n\nInside the card · {} (read from Windows): the GPU chip wired to the card's own PCIe switch, ≈ {}\u{A0}Gb/s.",
                    pcie_long(g, w),
                    gbit(pcie_gbs(g, w) as f64)
                ));
            }
            if bottleneck {
                tip.push_str(t(
                    "\n\nO funil marca o gargalo: o túnel é o trecho mais estreito do caminho, é ele que limita a banda da GPU, não a placa.",
                    "\n\nThe funnel marks the bottleneck: the tunnel is the narrowest part of the path, and it is what limits the GPU's bandwidth, not the card.",
                ));
            }
            x.tip(tip)
        }
        (None, Some((g, w))) => tl("🔗", Some(pcie_long(g, w)), "", t("Dentro da placa", "Inside the card"), neutral).tip(t(
            "O chip da GPU ligado ao switch PCIe da própria placa (lido do Windows). O túnel até o notebook é mais estreito, mas o driver não informou a largura dele.",
            "The GPU chip wired to the card's own PCIe switch (read from Windows). The tunnel to the laptop is narrower, but the driver didn't report its width.",
        )),
        (None, None) => tl("🔗", None, "", t("Túnel até o notebook", "Tunnel to the laptop"), neutral),
    }
}

/// "Gen4 x8" a partir do código de velocidade e da largura.
fn pcie_short(code: u32, w: u32) -> String {
    format!("{}\u{A0}x{w}", pnp::pcie_speed_name(code).split([' ', '\u{A0}']).next().unwrap_or("?"))
}

/// Nome curto do modelo da fonte: a última palavra com número ("Corsair SF750" → "SF750").
fn short_model(m: &str) -> &str {
    let m = m.trim();
    m.split_whitespace().rev().find(|w| w.chars().any(|c| c.is_ascii_digit())).or_else(|| m.split_whitespace().last()).unwrap_or(m)
}

/// Banda em Gb/s (bits) a partir de GB/s (bytes), com 1 casa: a mesma unidade da conexão,
/// para "12,6 de 20" não virar "1,6 de 20".
fn gbit(gbs: f64) -> String {
    dec(gbs * 8.0, 1)
}

fn dec(v: f64, n: usize) -> String {
    crate::i18n::dec_sep(format!("{v:.n$}"))
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
    let rad = CornerRadius::same(pal::radius().min(8));
    p.add(pal::elev(2, hv).as_shape(r, rad));
    p.rect_filled(r, rad, pal::card2());
    if hv > 0.01 {
        p.rect_stroke(r, rad, Stroke::new(1.0, x.color.gamma_multiply(0.6 * hv)), StrokeKind::Inside);
    }
    // ícone num círculo tingido
    let ic = pos2(r.left() + TILE_PAD + 15.0, r.top() + TILE_PAD + 15.0);
    p.circle_filled(ic, 15.0, x.color.gamma_multiply(0.16));
    if x.icon == FUNNEL {
        // funil: boca larga em cima, bico estreito embaixo (duas formas convexas)
        let f = |dx: f32, dy: f32| ic + vec2(dx, dy);
        p.add(egui::Shape::convex_polygon(vec![f(-7.5, -6.0), f(7.5, -6.0), f(1.8, 1.0), f(-1.8, 1.0)], x.color, Stroke::NONE));
        p.rect_filled(Rect::from_min_max(f(-1.8, 0.5), f(1.8, 7.0)), CornerRadius::same(1), x.color);
    } else {
        p.text(ic, egui::Align2::CENTER_CENTER, x.icon, FontId::proportional(17.0), x.color);
    }
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
        // trilho com contraste também no claro (o `well` some sobre o bloco)
        p.rect_filled(b, CornerRadius::same(2), if pal::current().is_light() { pal::grid() } else { pal::well() });
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
            // linha incompleta (ex.: 5 blocos em 2 colunas): os blocos dela dividem a largura toda
            let w = if row.len() < cols { ((avail - 8.0 * (row.len() as f32 - 1.0)) / row.len() as f32).floor() } else { w };
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
    let rad = CornerRadius::same(pal::radius());
    let t = ui.input(|i| i.time);
    let fr = egui::Frame::new()
        .fill(pal::card2())
        .corner_radius(rad)
        .inner_margin(egui::Margin::same(12))
        .shadow(pal::elev(2, 0.0))
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
            if r.on_hover_text(crate::i18n::t("Fechar (Esc)", "Close (Esc)")).clicked() {
                *sel = None;
            }
        });
    // filete de 3 pt no topo, na cor do ESTADO (verde/âmbar/vermelho/cinza), seguindo os cantos
    // do cartão; sem estado (painéis "Como funciona"), a cor neutra recebida. Nunca a da marca.
    let r = fr.response.rect;
    let strip = state.map(|(_, c)| c).unwrap_or(accent);
    ui.painter().with_clip_rect(Rect::from_min_max(r.min, pos2(r.right(), r.top() + 3.0))).rect_filled(r, rad, strip);
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
        ui.painter().rect_stroke(r.rect, CornerRadius::same(pal::radius().max(2).min(14)), Stroke::new(1.0, pal::sel()), StrokeKind::Inside);
    }
    resp
}

/// Detalhes crus no fim, recolhidos.
fn side_tech(ui: &mut Ui, key: &str, add: impl FnOnce(&mut Ui)) {
    ui.add_space(14.0);
    egui::CollapsingHeader::new(egui::RichText::new(t("🔧 Detalhes técnicos", "🔧 Technical details")).size(13.5).color(pal::dim()))
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
        tf!("hoje {}", "today {}", t.format("%H:%M"))
    } else if Some(d) == today.pred_opt() {
        tf!("ontem {}", "yesterday {}", t.format("%H:%M"))
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
            // uma linha só, sem quebra: ícone, hora, título, ícones de contexto e contador. Quando falta
            // largura, ficam os 2 primeiros ícones + "…" (dica com todos) e, se ainda faltar, o título
            // encolhe com "…". O contador nunca cai sozinho para a linha de baixo.
            let mut ctxs: Vec<(&'static str, Color32, f32, String)> = Vec::new();
            let mut seen = vec![head.kind];
            for e in &inc.events {
                if !seen.contains(&e.kind) {
                    seen.push(e.kind);
                    ctxs.push((kind_icon(e.kind), cat_color(e.kind.cat()), 13.0, e.kind.label().to_string()));
                }
            }
            if inc.is_drop && inc.events.iter().any(|e| e.kind.cat() == Cat::Dock) {
                if inc.pd_lost {
                    ctxs.push(("🔋", pal::amber(), 14.0, t("PD caiu junto: o dock inteiro apagou", "PD dropped too: the whole dock went dark").into()));
                } else {
                    ctxs.push(("⛓", pal::violet(), 14.0, t("Só o enlace caiu; o PD continuou", "Only the link dropped; PD stayed up").into()));
                }
            }
            let cnt = (inc.events.len() > 1).then(|| format!("×{}", inc.events.len()));
            let sp = ui.spacing().item_spacing.x;
            let wof = |ui: &Ui, s: &str, size: f32| ui.painter().layout_no_wrap(s.to_string(), FontId::proportional(size), pal::fg()).size().x;
            let full = ui.available_width();
            let time_w = if crate::i18n::en() { 108.0 } else { 96.0 };
            let fixed = 24.0 + time_w + 2.0 * sp + cnt.as_ref().map(|c| wof(ui, c, 12.5) + sp).unwrap_or(0.0);
            let icons_w = |k: usize, more: bool| -> f32 {
                ctxs.iter().take(k).map(|c| wof(ui, c.0, c.2) + sp).sum::<f32>() + if more { wof(ui, "…", 13.0) + sp } else { 0.0 }
            };
            let title = head.kind.label();
            let tw = wof(ui, title, 15.0);
            let mut shown = ctxs.len();
            if fixed + tw + icons_w(shown, false) > full && ctxs.len() > 2 {
                shown = 2;
            }
            let more = shown < ctxs.len();
            let title_room = (full - fixed - icons_w(shown, more) - 1.0).max(40.0);
            let r = ui.horizontal(|ui| {
                ui.add_sized([24.0, 20.0], egui::Label::new(egui::RichText::new(kind_icon(head.kind)).color(color).size(15.0)));
                ui.add_sized([time_w, 18.0], egui::Label::new(egui::RichText::new(friendly_time(inc.start)).color(pal::dim()).size(14.0)));
                let tl = egui::Label::new(egui::RichText::new(title).color(color).strong()).truncate();
                if tw <= title_room {
                    ui.add(tl);
                } else {
                    ui.scope(|ui| {
                        ui.set_max_width(title_room);
                        ui.add(tl);
                    });
                }
                for c in ctxs.iter().take(shown) {
                    ui.add(egui::Label::new(egui::RichText::new(c.0).color(c.1).size(c.2)).extend()).on_hover_text(&c.3);
                }
                if more {
                    let all: Vec<String> = ctxs.iter().map(|c| format!("{} {}", c.0, c.3)).collect();
                    ui.add(egui::Label::new(egui::RichText::new("…").color(pal::dim()).size(13.0)).extend()).on_hover_text(all.join("\n"));
                }
                if let Some(c) = &cnt {
                    ui.add(egui::Label::new(egui::RichText::new(c).color(pal::faint()).size(12.5)).extend());
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
                        ui.label(egui::RichText::new(&e.msg).size(13.5)).on_hover_text(tf!("fonte: {}\n{}", "source: {}\n{}", e.src, e.dev.clone().unwrap_or_default()));
                    });
                    if let Some(s) = &e.snap {
                        ui.horizontal_wrapped(|ui| {
                            ui.add_space(40.0);
                            ui.label(egui::RichText::new(format!("📸 {s}")).size(13.0).color(pal::data()));
                        });
                    }
                }
                if inc.is_drop {
                    let before = history::get().before(inc.start, 120_000);
                    ui.horizontal(|ui| {
                        ui.add_space(22.0);
                        ui.label(egui::RichText::new(tf!("⏪ 2\u{A0}min antes: {}", "⏪ 2\u{A0}min before: {}", if before.is_empty() { t("nada no Event Log (queda sem aviso)", "nothing in the Event Log (drop with no warning)").to_string() } else { tf!("{} evento(s)", "{} event(s)", before.len()) })).strong().color(pal::amber()).size(13.5));
                    });
                    for e in before.iter().rev().take(20) {
                        ui.horizontal_wrapped(|ui| {
                            ui.add_space(40.0);
                            ui.label(egui::RichText::new(format!("−{}\u{A0}s", dec((inc.start - e.ts) as f64 / 1000.0, 1))).monospace().size(13.0).color(pal::faint()));
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
            Topic::Link => t("⚡ Enlace Thunderbolt", "⚡ Thunderbolt link"),
            Topic::Power => t("🔌 Energia PD", "🔌 PD power"),
            Topic::Stability => t("🛡 Quedas do dock", "🛡 Dock drops"),
            Topic::Flow => t("📶 Fluxo de dados", "📶 Data flow"),
            Topic::Gpu => "🎮 GPU",
            Topic::Hidden => t("🚫 Invisível ao Windows", "🚫 Hidden from Windows"),
        }
    }
}

fn node_details(ui: &mut Ui, title: &str, n: Option<&Node>) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title).strong().color(pal::fg()));
    let Some(n) = n else {
        ui.label(egui::RichText::new(t("ausente", "missing")).color(pal::red()));
        return;
    };
    egui::Grid::new(("nd", title, &n.id)).num_columns(2).spacing(vec2(10.0, 2.0)).striped(true).max_col_width(ui.available_width() * 0.62).show(ui, |ui| {
        let mut row = |k: &str, v: String| {
            ui.label(egui::RichText::new(k).color(pal::dim()).size(13.0));
            ui.label(egui::RichText::new(v).size(13.0));
            ui.end_row();
        };
        row(t("nome", "name"), n.name.clone());
        row("instance id", n.id.clone());
        if let Some(f) = &n.firmware {
            row("firmware", f.clone());
        }
        row(t("estado", "state"), tf!("{} · problema {}", "{} · problem {}", dname(n), n.problem));
        if let Some(d) = &n.driver {
            row("driver", format!("{d} ({})", n.driver_provider.clone().unwrap_or_default()));
        }
        if let Some(l) = &n.location {
            row(t("local", "location"), l.clone());
        }
        if let Some((cs, cw, ms, mw)) = n.pcie {
            row(t("enlace PCIe", "PCIe link"), tf!("{}\u{A0}x{cw} (máx {}\u{A0}x{mw})", "{}\u{A0}x{cw} (max {}\u{A0}x{mw})", pnp::pcie_speed_name(cs), pnp::pcie_speed_name(ms)));
        }
        if let Some(t) = n.last_arrival {
            row(crate::i18n::t("última chegada", "last arrival"), fmt_local(t, false));
        }
        if let Some(t) = n.last_removal {
            row(crate::i18n::t("última remoção", "last removal"), fmt_local(t, false));
        }
        if let Some(r) = n.removal_policy {
            row(t("política de remoção", "removal policy"), match r { 1 => t("esperada (hot-plug)", "expected (hot-plug)").into(), 2 => t("remoção ordenada", "orderly removal").into(), 3 => t("remoção surpresa", "surprise removal").into(), x => x.to_string() });
        }
        for (k, v) in &n.pci_err {
            let v = if *k == "Tipo de dispositivo PCIe" { v.split(' ').next().and_then(|x| x.parse().ok()).map(pnp::pci_type_name).unwrap_or("?").to_string() } else { v.clone() };
            // chaves vêm de pnp.rs em português; traduzidas só na exibição
            let k: &str = match *k {
                "AER presente" => t("AER presente", "AER present"),
                "Máscara de erros não corrigíveis" => t("Máscara de erros não corrigíveis", "Uncorrectable error mask"),
                "Severidade não corrigíveis" => t("Severidade não corrigíveis", "Uncorrectable severity"),
                "Máscara de erros corrigíveis" => t("Máscara de erros corrigíveis", "Correctable error mask"),
                "Tipo de dispositivo PCIe" => t("Tipo de dispositivo PCIe", "PCIe device type"),
                "Versão PCIe Express" => t("Versão PCIe Express", "PCIe Express version"),
                other => other,
            };
            row(k, v);
        }
    });
}

fn usb4_table(ui: &mut Ui, title: &str, props: &[crate::pnp::Usb4Prop]) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title).strong().color(pal::bolt()));
    if props.is_empty() {
        ui.label(egui::RichText::new(t("nenhuma", "none")).color(pal::dim()));
        return;
    }
    egui::Grid::new(("u4", title)).num_columns(4).spacing(vec2(8.0, 2.0)).striped(true).max_col_width(ui.available_width() * 0.42).show(ui, |ui| {
        for h in ["pid", t("valor bruto", "raw value"), t("significado", "meaning"), t("confiança", "confidence")] {
            ui.label(egui::RichText::new(h).color(pal::faint()).size(12.5));
        }
        ui.end_row();
        for p in props {
            ui.label(egui::RichText::new(p.pid.to_string()).monospace().size(13.0));
            ui.label(egui::RichText::new(format!("{} {}", p.val.type_name(), p.val.show())).monospace().size(12.5).color(pal::dim()));
            ui.label(egui::RichText::new(format!("{}\n{}", p.meaning, p.decoded)).size(13.0));
            let c = match p.confidence {
                "certo" => pal::green(),
                "forte" => pal::data(),
                "hipótese" => pal::amber(),
                _ => pal::faint(),
            };
            let conf = match p.confidence {
                "certo" => t("certo", "certain"),
                "forte" => t("forte", "strong"),
                "hipótese" => t("hipótese", "hypothesis"),
                "desconhecido" => t("desconhecido", "unknown"),
                x => x,
            };
            ui.label(egui::RichText::new(conf).color(c).size(13.0));
            ui.end_row();
        }
    });
}

fn bars(ui: &mut Ui, r: &BenchResult) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 74.0), Sense::hover());
    let p = ui.painter_at(rect);
    let max = 3.2f32;
    // valores numa coluna própria à direita: a faixa tracejada nunca passa por cima do número
    let vals = [r.h2d_gbs, r.d2h_gbs].map(|v| format!("{}\u{A0}GB/s", dec(v, 2)));
    let vw = vals.iter().map(|t| p.layout_no_wrap(t.clone(), FontId::proportional(14.0), pal::fg()).size().x).fold(0.0, f32::max);
    let x_end = rect.right() - vw - 12.0;
    let x_of = |v: f32| rect.left() + 150.0 + (x_end - rect.left() - 150.0).max(20.0) * (v / max).clamp(0.0, 1.0);
    // faixa esperada TB3 40G
    let band = Rect::from_x_y_ranges(x_of(2.5)..=x_of(2.8), rect.top()..=rect.bottom() - 14.0);
    for x in [band.left(), band.right()] {
        p.extend(egui::Shape::dashed_line(&[pos2(x, band.top()), pos2(x, band.bottom())], Stroke::new(1.0, pal::green().gamma_multiply(0.7)), 4.0, 3.0));
    }
    let band_txt = t("normal com cabo de 40\u{A0}Gb/s: 2,5–2,8\u{A0}GB/s", "normal with a 40\u{A0}Gb/s cable: 2.5–2.8\u{A0}GB/s");
    let lg = p.layout_no_wrap(band_txt.into(), FontId::proportional(12.5), pal::green()).size().x;
    let lx = band.center().x.clamp(rect.left() + lg / 2.0, rect.right() - lg / 2.0);
    p.text(pos2(lx, rect.bottom()), egui::Align2::CENTER_BOTTOM, band_txt, FontId::proportional(12.5), pal::green());
    for (i, (label, v, c)) in [(t("Notebook → placa", "Laptop → GPU"), r.h2d_gbs, pal::data()), (t("Placa → notebook", "GPU → laptop"), r.d2h_gbs, pal::violet())].iter().enumerate() {
        let y = rect.top() + 8.0 + i as f32 * 26.0;
        p.text(pos2(rect.left(), y + 8.0), egui::Align2::LEFT_CENTER, *label, FontId::proportional(13.5), pal::dim());
        let bar = Rect::from_min_max(pos2(x_of(0.0), y), pos2(x_of(*v as f32), y + 16.0));
        p.rect_filled(bar, CornerRadius::same(3), *c);
        p.text(pos2(rect.right(), y + 8.0), egui::Align2::RIGHT_CENTER, &vals[i], FontId::proportional(14.0), pal::fg());
    }
}

/// Prende com espaço não separável as palavras vizinhas de um número em nomes de modelo
/// ("AMD Radeon RX 7600 XT" quebra como "AMD Radeon" / "RX 7600 XT", nunca no meio).
fn bind_num(s: &str) -> String {
    let w: Vec<&str> = s.split(' ').collect();
    let mut out = String::with_capacity(s.len() + 4);
    for (i, t) in w.iter().enumerate() {
        if i > 0 {
            let num = |x: &str| x.chars().any(|c| c.is_ascii_digit());
            out.push(if num(w[i - 1]) || num(t) { '\u{A0}' } else { ' ' });
        }
        out.push_str(t);
    }
    out
}

fn fmt_dur(ms: i64) -> String {
    let s = ms / 1000;
    if s < 3600 {
        format!("{}\u{A0}min", s / 60)
    } else if s < 86400 {
        format!("{}\u{A0}h\u{A0}{}\u{A0}min", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}\u{A0}d\u{A0}{}\u{A0}h", s / 86400, (s % 86400) / 3600)
    }
}

// ---------------------------------------------------------------------------
// Feedback: painel que monta o relatório limpo e abre a issue no GitHub (navegador da pessoa)

use crate::feedback::{self, FbKind};

struct FbPanel {
    kind: FbKind,
    msg: String,
    on: [bool; 4],
    /// montado uma vez, quando a topologia e a primeira amostra chegam (ou no "Atualizar")
    report: Option<feedback::Report>,
    /// relatório das seções marcadas, já limpo (refeito só quando as caixas mudam)
    text: String,
    status: Option<String>,
}

impl Default for FbPanel {
    fn default() -> Self {
        Self { kind: FbKind::Problem, msg: String::new(), on: [true; 4], report: None, text: String::new(), status: None }
    }
}

impl App {
    fn feedback_panel(&mut self, ctx: &egui::Context, topo: &Topology, samples: &[Sample], gst: Option<&collect::GpuStatic>) {
        let Some(fb) = self.fb.as_mut() else { return };
        if fb.report.is_none() && topo.read_ms != 0 && !samples.is_empty() {
            let incs: Vec<feedback::Inc> = self
                .incidents
                .iter()
                .map(|i| feedback::Inc { start: i.start, end: i.end, sev: i.sev, is_drop: i.is_drop, pd_lost: i.pd_lost, kinds: i.events.iter().map(|e| e.kind).collect() })
                .collect();
            let r = feedback::build(topo, gst, samples.last(), &incs);
            fb.text = feedback::compose(&r, &fb.on);
            fb.report = Some(r);
        }
        let screen = ctx.content_rect();
        let w = (screen.width() - 48.0).clamp(320.0, 760.0);
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        // a prévia fica com a altura que sobra: o resto do painel é medido no quadro anterior
        let fixed_id = egui::Id::new("fb-fixo");
        let fixed: f32 = ctx.data(|d| d.get_temp(fixed_id)).unwrap_or(470.0);
        let ph = (screen.height() - 64.0 - 36.0 - 16.0 - fixed).clamp(60.0, 340.0);
        let mut preview_h = 0.0;
        let mut content_h = 0.0;
        egui::Modal::new(egui::Id::new("feedback"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(
                egui::Frame::new()
                    .fill(pal::card())
                    .stroke(Stroke::new(1.0, pal::border()))
                    .corner_radius(CornerRadius::same(pal::radius()))
                    .inner_margin(egui::Margin::same(18)),
            )
            .show(ctx, |ui| {
                ui.set_width(w);
                // janela baixa: o painel inteiro rola em vez de sair da tela. A altura vem da janela, não
                // do quadro anterior (o Area do Modal limita o conteúdo ao tamanho que ele já tinha)
                let hmax = (screen.height() - 100.0).max(200.0);
                content_h = ui.allocate_ui(vec2(w, hmax), |ui| egui::ScrollArea::vertical().id_salt("fb-tudo").show(ui, |ui| {
                    ui.set_width(w);
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                    ui.label(egui::RichText::new("💬  Enviar feedback").size(18.0).strong().color(pal::fg()));
                    ui.label(
                        egui::RichText::new("Vira uma issue pública no GitHub do eGPU Painel, aberta no seu navegador (precisa de uma conta no GitHub).")
                            .size(13.0)
                            .color(pal::dim()),
                    );
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        for k in FbKind::ALL {
                            if chip(ui, fb.kind == k, k.label(), pal::cyan()) {
                                fb.kind = k;
                            }
                        }
                    });
                    let hint = match fb.kind {
                        FbKind::Problem => "O que aconteceu? O que você esperava? Quando começou?",
                        FbKind::Idea => "Qual é a ideia? O que ela resolveria para você?",
                        FbKind::OtherDock => "Qual é o seu dock? O que aparece certo e o que aparece errado no painel?",
                    };
                    ui.add(egui::TextEdit::multiline(&mut fb.msg).hint_text(hint).desired_rows(4).desired_width(f32::INFINITY));
                    ui.horizontal_wrapped(|ui| {
                        check_style(ui);
                        ui.label(egui::RichText::new("Incluir:").size(13.0).strong().color(pal::dim()));
                        let mut changed = false;
                        for (i, name) in feedback::SECTIONS.iter().enumerate() {
                            changed |= ui.checkbox(&mut fb.on[i], *name).changed();
                        }
                        if changed {
                            if let Some(r) = &fb.report {
                                fb.text = feedback::compose(r, &fb.on);
                            }
                        }
                    });
                    ui.label(
                        egui::RichText::new("🔒 Nome de usuário, nome do PC e números de série são removidos. Você vê tudo antes de enviar.")
                            .size(13.0)
                            .color(pal::faint()),
                    );
                    let full = feedback::full_text(fb.kind, &fb.msg, &fb.text);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Prévia do que vai").size(13.0).strong().color(pal::dim()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("↻ Atualizar").on_hover_text("Lê de novo o dock, a placa e as quedas").clicked() {
                                fb.report = None;
                                fb.status = None;
                            }
                        });
                    });
                    preview_h = egui::Frame::new()
                        .fill(pal::well())
                        .stroke(Stroke::new(1.0, pal::border()))
                        .corner_radius(CornerRadius::same(pal::radius().min(6)))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical().max_height(ph).auto_shrink([false, true]).show(ui, |ui| {
                                if fb.report.is_none() {
                                    ui.label(egui::RichText::new("Juntando as informações…").color(pal::faint()));
                                } else {
                                    ui.add(
                                        egui::TextEdit::multiline(&mut full.as_str())
                                            .font(FontId::monospace(12.0))
                                            .text_color(pal::fg())
                                            .frame(egui::Frame::NONE)
                                            .desired_width(f32::INFINITY),
                                    );
                                }
                            });
                        })
                        .response
                        .rect
                        .height();
                    if let Some(st) = &fb.status {
                        ui.label(egui::RichText::new(st).size(13.0).color(pal::green()));
                    }
                    ui.add_space(2.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let ready = fb.report.is_some();
                        let open = ui.add_enabled(
                            ready,
                            egui::Button::new(egui::RichText::new("↗ Abrir no GitHub").size(14.0).strong().color(pal::fg()))
                                .fill(pal::cyan().gamma_multiply(0.25))
                                .stroke(Stroke::new(1.0, pal::cyan().gamma_multiply(0.8)))
                                .corner_radius(CornerRadius::same(6))
                                .min_size(vec2(0.0, 32.0)),
                        );
                        if open.on_hover_text("Copia o texto completo e abre a issue já preenchida no navegador").clicked() {
                            let hints = fb.report.as_ref().map(|r| r.hints.clone()).unwrap_or_default();
                            let (url, _cut) = feedback::issue_url(fb.kind, &fb.msg, &fb.text, &hints);
                            ui.ctx().copy_text(full.clone());
                            open_url(&url);
                            fb.status = Some("Abrimos o GitHub no navegador. Se o relatório estiver cortado, cole com Ctrl+V.".into());
                        }
                        let btn = |t: &str| egui::Button::new(egui::RichText::new(t).size(14.0)).min_size(vec2(0.0, 32.0));
                        if ui.add_enabled(ready, btn("📋 Copiar relatório")).clicked() {
                            ui.ctx().copy_text(full.clone());
                            fb.status = Some("Copiado para a área de transferência.".into());
                        }
                        if ui.add(btn("Fechar")).clicked() {
                            close = true;
                        }
                    });
                })
                .content_size
                .y)
                .inner;
            });
        let fixed_now = (content_h - preview_h).max(0.0);
        if (fixed_now - fixed).abs() > 1.0 {
            ctx.data_mut(|d| d.insert_temp(fixed_id, fixed_now));
            ctx.request_repaint();
        }
        if close {
            self.fb = None;
        }
    }
}
