//! Painel de feedback (egui::Modal): tipo, mensagem, seções ligáveis e a prévia exata do que vai.
//! A lógica (coleta, limpeza, URL) fica em `crate::feedback`; aqui só a tela. Módulo filho de `ui`,
//! então usa direto `pal`, `chip`, `check_style` e `open_url` de lá.

use super::{Incident, chip, check_style, open_url, pal};
use crate::collect::{GpuStatic, Sample};
use crate::feedback::{self, FbKind};
use crate::i18n::t;
use crate::pnp::{self, Topology};
use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, vec2};

/// Fica no App mesmo fechado: fechar (Esc, clique fora, "Fechar") não perde o rascunho.
pub(super) struct FbPanel {
    pub(super) open: bool,
    kind: FbKind,
    msg: String,
    on: [bool; 4],
    /// montado ao abrir, quando a topologia e a primeira amostra chegam (ou após 3 s com o que houver)
    report: Option<feedback::Report>,
    /// relatório das seções marcadas, já limpo (refeito só quando as caixas mudam)
    text: String,
    status: Option<String>,
    opened_at: Option<f64>,
    want_focus: bool,
}

/// Caixas marcadas de saída: uma ideia raramente precisa do hardware inteiro.
fn default_on(k: FbKind) -> [bool; 4] {
    if k == FbKind::Idea { [true, false, false, false] } else { [true; 4] }
}

impl FbPanel {
    pub(super) fn new(open: bool) -> Self {
        let kind = FbKind::Problem;
        Self { open, kind, msg: String::new(), on: default_on(kind), report: None, text: String::new(), status: None, opened_at: None, want_focus: true }
    }

    /// Abre de novo com o rascunho de antes e um relatório lido agora.
    pub(super) fn reopen(&mut self) {
        self.open = true;
        self.report = None;
        self.status = None;
        self.opened_at = None;
        self.want_focus = true;
    }
}

pub(super) fn show(ctx: &egui::Context, fb: &mut FbPanel, topo: &Topology, samples: &[Sample], gst: Option<&GpuStatic>, incidents: &[Incident]) {
    if !fb.open {
        return;
    }
    let now = ctx.input(|i| i.time);
    let t0 = *fb.opened_at.get_or_insert(now);
    let live = topo.read_ms != 0 && !samples.is_empty();
    if fb.report.is_none() && (live || now - t0 > 3.0) {
        // sem coleta ao vivo depois de 3 s: monta com o que houver (a topologia é lida aqui se faltar)
        let fresh;
        let topo = if topo.read_ms != 0 {
            topo
        } else {
            fresh = pnp::read_topology();
            &fresh
        };
        let incs: Vec<feedback::Inc> = incidents
            .iter()
            .map(|i| feedback::Inc { start: i.start, end: i.end, sev: i.sev, is_drop: i.is_drop, pd_lost: i.pd_lost, kinds: i.events.iter().map(|e| e.kind).collect() })
            .collect();
        let r = feedback::build(topo, gst, samples.last(), &incs);
        fb.text = feedback::compose(&r, &fb.on);
        fb.report = Some(r);
    }
    if fb.report.is_none() {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    // a mensagem passa pela mesma limpeza do relatório; prévia, link e área de transferência usam esta
    let hw = fb.report.as_ref().and_then(|r| r.hw.clone());
    let mut msg = feedback::sanitize(&fb.msg, hw.as_deref());
    let mut full = feedback::full_text(fb.kind, &msg, &fb.text);

    let screen = ctx.content_rect();
    let w = (screen.width() - 48.0).clamp(320.0, 760.0);
    // Esc com o campo de texto em foco só tira o foco dele (o egui não consome a tecla): não fecha o painel
    let typing = ctx.memory(|m| m.focused().is_some());
    let mut close = false;
    let resp = egui::Modal::new(egui::Id::new("feedback"))
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
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            // janela baixa: só o conteúdo rola; aviso e botões ficam sempre à vista embaixo. A altura vem da
            // janela (o Area do Modal limitaria o conteúdo ao tamanho que ele já tinha)
            let hmax = (screen.height() - 100.0 - 84.0).max(160.0);
            ui.allocate_ui(vec2(w, hmax), |ui| {
                egui::ScrollArea::vertical().id_salt("fb-tudo").show(ui, |ui| {
                    ui.set_width(w);
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                    ui.label(egui::RichText::new(t("💬  Enviar feedback", "💬  Send feedback")).size(18.0).strong().color(pal::fg()));
                    ui.label(
                        egui::RichText::new(t(
                            "Vira uma issue pública no GitHub do eGPU Painel, aberta no seu navegador (precisa de uma conta no GitHub).",
                            "It becomes a public issue on the eGPU Painel GitHub, opened in your browser (you need a GitHub account).",
                        ))
                        .size(13.0)
                        .color(pal::dim()),
                    );
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        for k in FbKind::ALL {
                            if chip(ui, fb.kind == k, k.label(), pal::cyan()) && fb.kind != k {
                                fb.kind = k;
                                fb.on = default_on(k);
                                if let Some(r) = &fb.report {
                                    fb.text = feedback::compose(r, &fb.on);
                                }
                                full = feedback::full_text(fb.kind, &msg, &fb.text);
                            }
                        }
                    });
                    let hint = match fb.kind {
                        FbKind::Problem => t("O que aconteceu? O que você esperava? Quando começou?", "What happened? What did you expect? When did it start?"),
                        FbKind::Idea => t("Qual é a ideia? O que ela resolveria para você?", "What is the idea? What would it solve for you?"),
                        FbKind::OtherDock => t(
                            "Qual é o seu dock? O que aparece certo e o que aparece errado no painel?",
                            "Which dock do you have? What does the panel show right, and what does it show wrong?",
                        ),
                    };
                    let te = ui.add(egui::TextEdit::multiline(&mut fb.msg).hint_text(hint).desired_rows(4).desired_width(f32::INFINITY));
                    if fb.want_focus {
                        te.request_focus();
                        fb.want_focus = false;
                    }
                    if te.changed() {
                        msg = feedback::sanitize(&fb.msg, hw.as_deref());
                        full = feedback::full_text(fb.kind, &msg, &fb.text);
                    }
                    ui.horizontal_wrapped(|ui| {
                        check_style(ui);
                        ui.label(egui::RichText::new(t("Incluir:", "Include:")).size(13.0).strong().color(pal::dim()));
                        let mut changed = false;
                        for i in 0..4 {
                            changed |= ui.checkbox(&mut fb.on[i], feedback::section_label(i)).changed();
                        }
                        if changed {
                            if let Some(r) = &fb.report {
                                fb.text = feedback::compose(r, &fb.on);
                            }
                            full = feedback::full_text(fb.kind, &msg, &fb.text);
                        }
                    });
                    ui.label(
                        egui::RichText::new(t(
                            "🔒 Nome de usuário, nome do PC e números de série são removidos, também da sua mensagem. Você vê tudo antes de enviar.",
                            "🔒 User name, PC name and serial numbers are removed, from your message too. You see everything before sending.",
                        ))
                        .size(13.0)
                        .color(pal::faint()),
                    );
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(t("Prévia do que será enviado", "Preview of what will be sent")).size(13.0).strong().color(pal::dim()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(t("↻ Atualizar", "↻ Refresh")).on_hover_text(t("Lê de novo o dock, a placa e as quedas", "Reads the dock, the card and the drops again")).clicked() {
                                fb.report = None;
                                fb.status = None;
                                fb.opened_at = None;
                            }
                        });
                    });
                    egui::Frame::new()
                        .fill(pal::well())
                        .stroke(Stroke::new(1.0, pal::border()))
                        .corner_radius(CornerRadius::same(pal::radius().min(6)))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            if fb.report.is_none() {
                                ui.label(egui::RichText::new(t("Juntando as informações…", "Gathering the information…")).color(pal::faint()));
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
                });
            });
            ui.add_space(6.0);
            if let Some(st) = &fb.status {
                ui.label(egui::RichText::new(st).size(13.0).color(pal::green()));
            }
            ui.add_space(2.0);
            // Align::Min: com Center a linha ocupa a altura que sobra e o Modal nunca encolhe
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                let ready = fb.report.is_some();
                let open = ui.add_enabled(
                    ready,
                    egui::Button::new(egui::RichText::new(t("↗ Abrir no GitHub", "↗ Open on GitHub")).size(14.0).strong().color(pal::fg()))
                        .fill(pal::cyan().gamma_multiply(0.25))
                        .stroke(Stroke::new(1.0, pal::cyan().gamma_multiply(0.8)))
                        .corner_radius(CornerRadius::same(6))
                        .min_size(vec2(0.0, 32.0)),
                );
                if open.on_hover_text(t("Abre a issue já preenchida no navegador", "Opens the pre-filled issue in your browser")).clicked() {
                    let (url, msg_cut, rep_cut) = feedback::issue_url(fb.kind, &msg, &fb.text);
                    // a área de transferência só é tocada quando algo não coube no link
                    let st = if msg_cut {
                        ui.ctx().copy_text(full.clone());
                        t(
                            "A mensagem não coube inteira no link: o texto completo foi copiado. No GitHub, troque os trechos cortados pelo texto completo com Ctrl+V.",
                            "The message did not fit in the link: the full text was copied. On GitHub, replace the cut parts with the full text using Ctrl+V.",
                        )
                    } else if rep_cut {
                        ui.ctx().copy_text(fb.text.clone());
                        t(
                            "O relatório não coube inteiro no link: ele foi copiado. No GitHub, apague o trecho cortado do campo Relatório e cole com Ctrl+V.",
                            "The report did not fit in the link: it was copied. On GitHub, delete the cut part of the Report field and paste with Ctrl+V.",
                        )
                    } else {
                        t("Abrimos o GitHub no navegador. É só revisar e enviar.", "GitHub is open in your browser. Just review and submit.")
                    };
                    fb.status = Some(st.into());
                    open_url(&url);
                }
                let btn = |s: &str| egui::Button::new(egui::RichText::new(s).size(14.0)).min_size(vec2(0.0, 32.0));
                if ui.add_enabled(ready, btn(t("📋 Copiar relatório", "📋 Copy report"))).clicked() {
                    ui.ctx().copy_text(full.clone());
                    fb.status = Some(t("Copiado para a área de transferência.", "Copied to the clipboard.").into());
                }
                if ui.add(btn(t("Fechar", "Close"))).clicked() {
                    close = true;
                }
            });
        });
    if close || resp.backdrop_response.clicked() || (!typing && resp.should_close()) {
        fb.open = false;
    }
}
