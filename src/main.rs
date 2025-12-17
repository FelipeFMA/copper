mod pipewire;
mod state;
mod ui;

use crossbeam_channel::unbounded;
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;

use state::{AppState, PwCommand};
use ui::{setup_custom_style, CopperApp};

fn main() -> Result<(), eframe::Error> {
    env_logger::init();

    let state = Arc::new(Mutex::new(AppState::new()));
    let (tx_cmd, rx_cmd) = unbounded::<PwCommand>();
    let repaint_ctx = Arc::new(Mutex::new(None::<egui::Context>));

    // Spawn PipeWire thread
    {
        let state = state.clone();
        let repaint_ctx = repaint_ctx.clone();
        std::thread::spawn(move || {
            pipewire::run(state, rx_cmd, repaint_ctx);
        });
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([400.0, 600.0])
            .with_min_inner_size([300.0, 200.0])
            .with_transparent(false),
        ..Default::default()
    };

    eframe::run_native(
        "Copper",
        options,
        Box::new(move |cc| {
            *repaint_ctx.lock() = Some(cc.egui_ctx.clone());
            setup_custom_style(&cc.egui_ctx);
            Ok(Box::new(CopperApp::new(state, tx_cmd)))
        }),
    )
}
