//! The one-time welcome tour for Android tablets.
//!
//! It is drawn with WordCraft's own egui widgets (no new dependency): five short pages — Welcome,
//! Storage, Appearance, Editing & Start screen, Done — with progress dots, Back/Next and Skip.
//! Completing or skipping it stores [`crate::UiState::onboarding_done`] in `ui.json`, so it never
//! appears by itself again. File › Options › General can show it again (`ui.onboarding`).
//!
//! Motion (page slide/fade, filling the progress dot, the welcome mark drawing in) follows the
//! Animation effects setting: with them off, every page and dot is drawn at rest at once.

use egui::{Align, Layout, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;

use crate::theme::{self, Tokens, medium, regular, semibold};
use crate::{WordApp, icons};

/// The five sheets, in order.
pub const PAGES: usize = 5;

/// Where the tour is. Kept out of [`crate::UiState`] except for the completion flag, so reopening
/// it from Options starts at the welcome page again.
#[derive(Clone, Debug)]
pub(crate) struct Onboarding {
    /// The tour is on screen. With `ui.onboarding_done` false it shows without this too, so a
    /// first run that is killed mid-tour starts there again.
    pub active: bool,
    pub page: usize,
    /// -1 until the tour first draws; input-clock seconds since the page changed.
    page_started: f64,
    /// -1 until the welcome page first draws; input-clock seconds since the mark began drawing.
    welcome_started: f64,
    /// +1 when the page arrived from the right (Next), -1 from the left (Back).
    direction: f32,
    /// Android's file picker was asked for a document from the Storage page.
    waiting: bool,
    /// An error from the picker request, shown inline instead of a status-bar flash.
    error: Option<String>,
}

impl Default for Onboarding {
    fn default() -> Self {
        Self { active: false, page: 0, page_started: -1.0, welcome_started: -1.0, direction: 1.0, waiting: false, error: None }
    }
}

/// Whether the tour is on screen: either explicitly reopened, or this profile has not finished it.
pub fn visible(app: &WordApp) -> bool {
    app.onboarding.active || !app.ui.onboarding_done
}

/// Show the tour from the first page (Options › General, `ui.onboarding`).
pub fn open(app: &mut WordApp) {
    app.onboarding.active = true;
    app.onboarding.page = 0;
    app.onboarding.page_started = -1.0;
    app.onboarding.welcome_started = -1.0;
    app.onboarding.direction = 1.0;
    app.onboarding.waiting = false;
    app.onboarding.error = None;
}

/// Finish the tour (Done, Skip or Android Back) and remember it.
pub fn dismiss(app: &mut WordApp) {
    app.onboarding.active = false;
    app.onboarding.waiting = false;
    app.onboarding.error = None;
    app.ui.onboarding_done = true;
}

/// Move to another page, clamped; the transition starts again in the direction of travel.
fn go_to(app: &mut WordApp, page: usize, now: f64) {
    let page = page.min(PAGES - 1);
    if page != app.onboarding.page {
        app.onboarding.direction = if page > app.onboarding.page { 1.0 } else { -1.0 };
        app.onboarding.page = page;
        app.onboarding.page_started = now;
        app.onboarding.error = None;
    }
}

/// Next (the last page finishes the tour).
fn next(app: &mut WordApp, now: f64) {
    if app.onboarding.page + 1 >= PAGES {
        dismiss(app);
    } else {
        go_to(app, app.onboarding.page + 1, now);
    }
}

/// Back (the first page stays).
fn back(app: &mut WordApp, now: f64) {
    go_to(app, app.onboarding.page.saturating_sub(1), now);
}

/// The Storage page's primary action. Android's SAF picker is opened through the existing host
/// hook (`Services::open_async`); no storage implementation lives here. A document already open
/// is never replaced from the tour, so unsaved work cannot be lost here.
pub fn storage_action(app: &mut WordApp) -> bool {
    if app.has_document() {
        app.onboarding.waiting = false;
        app.onboarding.error = None;
        return true;
    }
    app.onboarding.waiting = true;
    app.onboarding.error = None;
    // `ui.openFileDialog` is the same request File › Open makes: the host's `open_async` hook
    // opens Android's picker. (A bare `file.open` would need a path.)
    match app.run("ui.openFileDialog", json!({})) {
        Ok(_) => true,
        Err(e) => {
            app.onboarding.waiting = false;
            app.onboarding.error = Some(e);
            false
        }
    }
}

/// Whether the Storage page should read as ready: a document was opened or saved.
fn storage_ready(app: &WordApp) -> bool {
    app.has_document()
}

/// Draw the tour over everything else, inside the safe area `content`.
pub fn show(app: &mut WordApp, ctx: &egui::Context, content: Rect) {
    if !visible(app) {
        return;
    }
    let now = ctx.input(|i| i.time);
    if app.onboarding.welcome_started < 0.0 {
        app.onboarding.welcome_started = now;
    }
    if app.onboarding.page_started < 0.0 {
        app.onboarding.page_started = now;
    }
    if storage_ready(app) {
        app.onboarding.waiting = false;
    }
    // Keys: Escape skips (the same as Back), arrows move between pages, Enter advances.
    let (escape, right, left, enter) = ctx.input(|i| {
        (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::ArrowRight), i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::Enter))
    });
    if escape {
        dismiss(app);
        return;
    }
    if right || enter {
        next(app, now);
    } else if left {
        back(app, now);
    }
    if !visible(app) {
        return;
    }

    let t = Tokens::get(ctx);
    let animate = app.animations_on();
    // Keep the slide, the mark and the active dot moving to their end state.
    if animate {
        let page_done = now - app.onboarding.page_started >= theme::PAGE_SLIDE_SECS;
        let mark_done = app.onboarding.page != 0 || now - app.onboarding.welcome_started >= theme::MARK_DRAW_SECS;
        if !page_done || !mark_done {
            ctx.request_repaint();
        }
    }

    let area = egui::Area::new(egui::Id::new("onboarding")).order(egui::Order::Tooltip).fixed_pos(content.min);
    area.show(ctx, |ui| {
        ui.set_min_size(content.size());
        // An opaque veil over the ribbon and page: no tap or wheel reaches the document beneath.
        let (veil, _) = ui.allocate_exact_size(content.size(), Sense::click_and_drag());
        ui.painter().rect_filled(veil, 0.0, t.canvas);
        ui.painter().rect_filled(veil, 0.0, egui::Color32::from_black_alpha(if t.dark { 90 } else { 40 }));

        // One fixed card, centred; it fits both 1200×1920 portrait and 1920×1200 landscape.
        let margin = 24.0;
        let max = (content.size() - vec2(2.0 * margin, 2.0 * margin)).max(vec2(1.0, 1.0));
        let width = max.x.min(760.0);
        let height = max.y.min(640.0);
        let card = Rect::from_center_size(content.center(), vec2(width, height));
        ui.scope_builder(egui::UiBuilder::new().max_rect(card), |ui| {
            egui::Frame::new().fill(t.ribbon).stroke(Stroke::new(1.0, t.border_strong)).corner_radius(14).inner_margin(egui::Margin::same(24)).show(
                ui,
                |ui| {
                    ui.set_min_size(card.size() - vec2(48.0, 48.0));
                    card_body(app, ui, now, animate);
                },
            );
        });
    });
}

fn card_body(app: &mut WordApp, ui: &mut Ui, now: f64, animate: bool) {
    let t = Tokens::get(ui.ctx());
    let full = ui.max_rect();
    let nav_h = 56.0;
    let body = Rect::from_min_max(full.min, pos2(full.max.x, (full.max.y - nav_h).max(full.min.y + 120.0)));
    let progress = theme::motion(now, app.onboarding.page_started, theme::PAGE_SLIDE_SECS, animate);
    // Slide in from the side the page came from, fading as it arrives.
    let dx = (1.0 - progress) * 26.0 * app.onboarding.direction;
    let page = app.onboarding.page;
    ui.scope_builder(egui::UiBuilder::new().max_rect(body.translate(vec2(dx, 0.0))), |ui| {
        ui.set_opacity(0.25 + 0.75 * progress);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.vertical(|ui| match page {
                0 => welcome(app, ui, now, animate),
                1 => storage(app, ui, &t),
                2 => appearance(app, ui),
                3 => editing(app, ui, &t),
                _ => done(app, ui, &t),
            });
        });
    });
    // The navigation row sits at the bottom of the card, never sliding.
    let nav_top = (body.max.y + 6.0).min(full.max.y);
    let nav = Rect::from_min_max(pos2(full.min.x, nav_top), full.max);
    ui.scope_builder(egui::UiBuilder::new().max_rect(nav), |ui| {
        nav_row(app, ui, now, &t);
    });
}

fn welcome(app: &mut WordApp, ui: &mut Ui, now: f64, animate: bool) {
    let t = Tokens::get(ui.ctx());
    ui.vertical_centered(|ui| {
        let draw = theme::motion(now, app.onboarding.welcome_started, theme::MARK_DRAW_SECS, animate);
        let side = 132.0 * (0.94 + 0.06 * draw);
        let (mark, _) = ui.allocate_exact_size(vec2(160.0, 150.0), Sense::hover());
        let icon = Rect::from_center_size(mark.center(), vec2(side, side));
        // The mark draws in from its top edge: a growing clip reveals the existing icon art.
        let reveal = Rect::from_min_max(mark.min, pos2(mark.max.x, mark.min.y + mark.height() * draw));
        let ink = t.icon.gamma_multiply(0.35 + 0.65 * draw);
        icons::paint(&ui.painter().with_clip_rect(reveal), icon, "watermark", ink, t.accent.gamma_multiply(draw));
        ui.add_space(8.0);
        ui.label(RichText::new(tl!("Welcome to WordCraft")).font(semibold(28.0)).color(t.text));
        ui.add_space(6.0);
        ui.label(
            RichText::new(tl!(
                "A fast, open word processor for your tablet. WordCraft is native — no browser, no account, and your documents stay yours."
            ))
            .font(regular(16.0))
            .color(t.text_dim),
        );
        ui.add_space(14.0);
        ui.label(
            RichText::new(tl!("Swipe with two fingers to move the page, pinch to zoom, and tap the page to type."))
                .font(regular(14.0))
                .color(t.text_dim),
        );
    });
}

fn storage(app: &mut WordApp, ui: &mut Ui, t: &Tokens) {
    heading(ui, t, tl!("Your documents, through Android Files"));
    paragraph(
        ui,
        t,
        tl!(
            "WordCraft opens and saves through Android's Files app (the Storage Access Framework). It asks for no broad storage permission and cannot see files you did not choose."
        ),
    );
    paragraph(
        ui,
        t,
        tl!(
            "The first time you open or save, Android asks which file or folder to use. That access is kept for the file you picked; other apps' documents stay private."
        ),
    );
    ui.add_space(10.0);
    let ready = storage_ready(app);
    if ready {
        let name = app.title_stem();
        ui.label(
            RichText::new(crate::i18n::fmt(tl!("Storage is ready — {name} is open."), &[("name", &name)])).font(medium(15.0)).color(t.accent_text),
        );
        ui.add_space(8.0);
        ui.label(RichText::new(tl!("Open and Save now write through the file you picked.")).font(regular(13.0)).color(t.text_dim));
    } else {
        if app.onboarding.waiting {
            ui.label(
                RichText::new(tl!("The picker is open, or was closed without choosing a file. You can try again below."))
                    .font(regular(14.0))
                    .color(t.text_dim),
            );
        } else {
            ui.label(
                RichText::new(tl!("No document is open yet. You can try the picker now, or continue and open one later."))
                    .font(regular(14.0))
                    .color(t.text_dim),
            );
        }
        if let Some(error) = app.onboarding.error.clone() {
            ui.label(RichText::new(error).font(regular(14.0)).color(t.red));
        }
        ui.add_space(8.0);
        // The button stays: Android reports a cancelled picker as no result at all, so there is no
        // event to clear `waiting`; asking again is the way back.
        if touch_button(ui, t, tl!("Open a document…"), true).clicked() {
            storage_action(app);
        }
        ui.add_space(6.0);
        ui.label(RichText::new(tl!("You can also use Save As the first time you save a new document.")).font(regular(13.0)).color(t.text_dim));
    }
}

fn appearance(app: &mut WordApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    heading(ui, &t, tl!("Make it yours"));
    paragraph(ui, &t, tl!("Pick how WordCraft looks. The change shows here at once, and is kept for next time."));
    ui.add_space(10.0);
    ui.label(RichText::new(tl!("Interface theme")).font(semibold(15.0)).color(t.text));
    ui.horizontal_wrapped(|ui| {
        for a in theme::Appearance::ALL {
            let on = app.ui.theme == a;
            if touch_button(ui, &t, a.label(), on).clicked() {
                let _ = app.run("ui.theme", json!({"value": a.code()}));
            }
        }
    });
    ui.add_space(16.0);
    ui.label(RichText::new(tl!("Interface size")).font(semibold(15.0)).color(t.text));
    ui.horizontal_wrapped(|ui| {
        let auto = app.ui.interface_size.is_auto();
        if touch_button(ui, &t, tl!("Auto"), auto).clicked() {
            let _ = app.run("ui.interfaceSize", json!({"value": "auto"}));
        }
        for pct in theme::InterfaceSize::PRESETS {
            let on = app.ui.interface_size.percent() == Some(pct);
            if touch_button(ui, &t, &format!("{pct}%"), on).clicked() {
                let _ = app.run("ui.interfaceSize", json!({"percent": pct}));
            }
        }
    });
    ui.add_space(10.0);
    ui.label(RichText::new(tl!("Auto follows the tablet's display scaling. Document zoom is separate.")).font(regular(13.0)).color(t.text_dim));
}

fn editing(app: &mut WordApp, ui: &mut Ui, t: &Tokens) {
    heading(ui, t, tl!("Editing & the Start screen"));
    paragraph(ui, t, tl!("These are the everyday choices. You can change any of them later in File › Options."));
    ui.add_space(8.0);
    let mut autocorrect = app.session.prefs.autocorrect.enabled;
    if touch_checkbox(ui, t, &mut autocorrect, tl!("AutoCorrect as you type")).changed() {
        let _ = app.run("tools.autocorrect", json!({"enabled": autocorrect}));
    }
    note(ui, t, tl!("Fixes capitalisation, straight quotes and common typos."));
    let mut spell = app.session.view.proofing;
    if touch_checkbox(ui, t, &mut spell, tl!("Check spelling as you type")).changed() {
        let _ = app.run("file.options", json!({"checkSpelling": spell}));
    }
    if app.has_document() {
        let mut autosave = app.autosave;
        if touch_checkbox(ui, t, &mut autosave, tl!("AutoSave my document")).changed() {
            match app.run("file.autosave", json!({"value": autosave})) {
                Ok(_) => {}
                Err(e) => app.status(e),
            }
        }
        note(ui, t, tl!("AutoSave starts once this document has a file; an unsaved new document uses Save As."));
    }
    let mut start = app.ui.start_screen;
    if touch_checkbox(ui, t, &mut start, tl!("Start on Home")).changed() {
        let _ = app.run("ui.startScreen", json!({"value": start}));
    }
    note(ui, t, tl!("Home shows Blank document, templates and recent files instead of an empty page."));
}

fn done(app: &mut WordApp, ui: &mut Ui, t: &Tokens) {
    heading(ui, t, tl!("You're ready"));
    paragraph(ui, t, tl!("WordCraft opens on Home, where you can start a blank document, pick a template or open a file from Android Files."));
    paragraph(ui, t, tl!("Your ribbon and settings stay where you leave them. The tour is in File › Options › General if you want it again."));
    ui.add_space(14.0);
    let name = app.title_stem();
    ui.label(RichText::new(name).font(semibold(16.0)).color(t.text));
}

fn nav_row(app: &mut WordApp, ui: &mut Ui, now: f64, t: &Tokens) {
    let last = app.onboarding.page + 1 >= PAGES;
    ui.horizontal(|ui| {
        if touch_button(ui, t, tl!("Skip"), false).clicked() {
            dismiss(app);
            return;
        }
        dots(ui, app, now, t);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let label = if last { tl!("Done") } else { tl!("Next") };
            if touch_button(ui, t, label, true).clicked() {
                next(app, now);
            }
            if app.onboarding.page > 0 && touch_button(ui, t, tl!("Back"), false).clicked() {
                back(app, now);
            }
        });
    });
}

/// Filled dots for the pages reached; the current one grows into its fill.
fn dots(ui: &mut Ui, app: &mut WordApp, now: f64, t: &Tokens) {
    let n = PAGES;
    let (rect, _) = ui.allocate_exact_size(vec2((n as f32) * 18.0, 24.0), Sense::hover());
    let animate = app.animations_on();
    let page = app.onboarding.page;
    for i in 0..n {
        let c = pos2(rect.min.x + 9.0 + (i as f32) * 18.0, rect.center().y);
        let filled = i <= page;
        let active = i == page;
        let grow = if active { theme::motion(now, app.onboarding.page_started, 0.18, animate) } else { 1.0 };
        let radius = if active { 4.0 + 1.5 * grow } else { 4.0 };
        if filled {
            ui.painter().circle_filled(c, radius, t.accent.gamma_multiply(0.35 + 0.65 * grow));
        } else {
            ui.painter().circle_stroke(c, radius, Stroke::new(1.5, t.border_strong));
        }
    }
}

fn heading(ui: &mut Ui, t: &Tokens, text: &str) {
    ui.label(RichText::new(text).font(semibold(23.0)).color(t.text));
    ui.add_space(6.0);
}

fn paragraph(ui: &mut Ui, t: &Tokens, text: &str) {
    ui.label(RichText::new(text).font(regular(15.0)).color(t.text_dim));
    ui.add_space(8.0);
}

fn note(ui: &mut Ui, t: &Tokens, text: &str) {
    ui.label(RichText::new(text).font(regular(12.5)).color(t.text_dim).italics());
    ui.add_space(6.0);
}

/// A 44-point-tall button, so fingers hit it; `primary` fills it with the accent colour.
fn touch_button(ui: &mut Ui, t: &Tokens, label: &str, primary: bool) -> egui::Response {
    let text = RichText::new(label).font(medium(15.0));
    let text = if primary { text.color(t.on_accent) } else { text.color(t.text) };
    let button = egui::Button::new(text).min_size(vec2(96.0, 44.0)).corner_radius(7.0);
    let button = if primary {
        button.fill(t.accent).stroke(Stroke::new(1.0, t.accent))
    } else {
        button.fill(t.input).stroke(Stroke::new(1.0, t.input_border))
    };
    ui.add(button)
}

fn touch_checkbox(ui: &mut Ui, t: &Tokens, value: &mut bool, label: &str) -> egui::Response {
    let mut response = ui.add(egui::Checkbox::new(value, RichText::new(label).font(regular(15.0)).color(t.text)));
    if response.rect.height() < 40.0 {
        response.rect = response.rect.expand2(vec2(4.0, 9.0));
    }
    response
}
