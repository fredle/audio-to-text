//! The app itself. The build stage replaces the body of `App::main_ui` (and
//! adds modules alongside) with the real feature, and `App::show_scene` with
//! the made-up states its Store screenshots show. The footer with the
//! auto-update toggle and version is required on every factory app.

use eframe::egui;

use crate::shots::Shots;
use crate::theme;

const WIDTH: f32 = 480.0;
const HEIGHT: f32 = 360.0;
/// How many scenes screenshot mode captures (see `show_scene`).
const SHOT_SCENES: usize = 1;

pub fn run(args: Vec<String>) -> eframe::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title("Transcribe Audio & Video to Text")
        .with_inner_size([WIDTH, HEIGHT])
        .with_min_inner_size([360.0, 240.0]);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Transcribe Audio & Video to Text",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(&args, &cc.egui_ctx)))),
    )
}

pub struct App {
    settings: crate::settings::Settings,
    version: String,
    shots: Option<Shots>,
}

impl App {
    fn new(args: &[String], ctx: &egui::Context) -> Self {
        theme::apply(ctx);
        theme::install_fonts(ctx);
        // For checking both looks without changing Windows' setting.
        if let Some(theme) = crate::shots::theme_from_args(args) {
            ctx.set_theme(theme);
        }
        let shots = Shots::from_args(args);
        let mut app = Self {
            settings: crate::settings::load(),
            version: crate::update::current_version(),
            shots,
        };
        if app.shots.is_some() {
            // Screenshots show the app as a new user first sees it.
            ctx.set_pixels_per_point(1.0);
            app.settings = crate::settings::Settings::default();
            app.version = env!("CARGO_PKG_VERSION").to_owned();
        }
        app
    }

    /// Saves settings, except in screenshot mode.
    fn save(&self) {
        if self.shots.is_none() {
            crate::settings::save(&self.settings);
        }
    }

    /// Puts the app into made-up state number `scene` (0-based) for a Store
    /// screenshot. Fill the window with believable content; start nothing
    /// real.
    fn show_scene(&mut self, _scene: usize) {}

    fn main_ui(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui.ctx());
        ui.label(egui::RichText::new("Transcribe Audio & Video to Text").font(theme::title()).color(p.text));
        ui.label(egui::RichText::new("Turn recordings, meetings and videos into text and subtitles on your own PC, with no time limit.").color(p.muted));
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui.ctx());
        ui.horizontal(|ui| {
            let mut auto_update = self.settings.auto_update;
            let label = egui::RichText::new("Automatically update").font(theme::small()).color(p.muted);
            if theme::checkbox(ui, &mut auto_update, label)
                .on_hover_text("Checks for new versions about once a day and installs them in the background.")
                .changed()
            {
                self.settings.auto_update = auto_update;
                self.save();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("v{}", self.version))
                        .font(theme::small())
                        .color(p.muted),
                );
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui_root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui_root.ctx().clone();
        if let Some(scene) = self.shots.as_mut().and_then(|shots| shots.drive(&ctx, SHOT_SCENES)) {
            self.show_scene(scene);
        }
        let p = theme::palette(&ctx);
        egui::Panel::bottom("footer")
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(16, 8)))
            .show(ui_root, |ui| self.footer(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::same(16)))
            .show(ui_root, |ui| self.main_ui(ui));
    }
}
