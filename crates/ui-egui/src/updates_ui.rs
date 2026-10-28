//! Update status for WABTY Studio. No release source is configured for this distribution.

use crate::SoundApp;

#[derive(Default)]
pub(crate) struct Runtime {
    pub(crate) manual_window_open: bool,
}

pub(crate) fn check(app: &mut SoundApp) {
    app.updates_rt.manual_window_open = true;
}

pub(crate) fn show(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.updates_rt.manual_window_open {
        return;
    }
    let mut open = true;
    let mut close = false;
    egui::Window::new("Check for Updates")
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label("Automatic updates are not configured for WABTY Studio.");
            ui.label(format!("Installed version: {}", env!("CARGO_PKG_VERSION")));
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close = ui.button("Close").clicked();
            });
        });
    if !open || close {
        app.updates_rt.manual_window_open = false;
    }
}
