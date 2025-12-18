use crate::state::{AppState, AudioNode, PwCommand};
use crossbeam_channel::Sender;
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;

pub struct CopperApp {
    state: Arc<Mutex<AppState>>,
    tx: Sender<PwCommand>,
}

impl CopperApp {
    pub fn new(state: Arc<Mutex<AppState>>, tx: Sender<PwCommand>) -> Self {
        Self { state, tx }
    }

    fn render_node(&self, ui: &mut egui::Ui, node: &AudioNode) {
        let mut frame = egui::Frame::group(ui.style());
        if node.is_default {
            frame = frame.fill(ui.visuals().selection.bg_fill.linear_multiply(0.1));
            frame = frame.stroke(egui::Stroke::new(1.0, ui.visuals().selection.bg_fill));
        }

        frame.show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.vertical(|ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(&node.description).strong()).truncate(),
                );
                ui.add(
                    egui::Label::new(egui::RichText::new(&node.name).small().weak()).truncate(),
                );

                ui.horizontal(|ui| {
                    let mut volume_percent = node.volume * 100.0;
                    let muted = node.muted;
                    let is_default = node.is_default;

                    if ui.selectable_label(muted, "Mute").clicked() {
                        let _ = self.tx.send(PwCommand::SetMute(node.id, !muted));
                    }

                    if ui.selectable_label(is_default, "Default").clicked() {
                        let _ = self.tx.send(PwCommand::SetDefault(node.id));
                    }

                    if is_default {
                        ui.label(egui::RichText::new("(Default)").small().strong().color(ui.visuals().selection.bg_fill));
                    }

                    let slider = egui::Slider::new(&mut volume_percent, 0.0..=100.0)
                        .show_value(true)
                        .text("Vol")
                        .suffix("%")
                        .fixed_decimals(0);

                    if ui.add(slider).changed() {
                        let _ = self.tx.send(PwCommand::SetVolume(node.id, volume_percent / 100.0));
                    }
                });
            });
        });
    }
}

impl eframe::App for CopperApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Q)) {
            let _ = self.tx.send(PwCommand::Quit);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        let state = self.state.lock();

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.heading("Copper Mixer");
                    ui.add_space(10.0);

                let mut sinks: Vec<&AudioNode> = state.nodes.values().filter(|n| n.is_sink).collect();
                let mut sources: Vec<&AudioNode> = state.nodes.values().filter(|n| !n.is_sink).collect();

                sinks.sort_by_key(|n| n.id);
                sources.sort_by_key(|n| n.id);

                if !sinks.is_empty() {
                    ui.label(egui::RichText::new("Outputs").strong());
                    for node in sinks {
                        self.render_node(ui, node);
                    }
                    ui.add_space(10.0);
                }

                if !sources.is_empty() {
                    ui.label(egui::RichText::new("Inputs").strong());
                    for node in sources {
                        self.render_node(ui, node);
                    }
                }
            });
        });
    }
}


