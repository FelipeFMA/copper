use crate::state::{AppState, AudioNode, PwCommand};
use crossbeam_channel::Sender;
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;

pub struct CopperApp {
    state: Arc<Mutex<AppState>>,
    tx: Sender<PwCommand>,
    current_tab: Tab,
}

#[derive(PartialEq)]
enum Tab {
    Outputs,
    Inputs,
    Playback,
    Recording,
    Configuration,
}

impl CopperApp {
    pub fn new(state: Arc<Mutex<AppState>>, tx: Sender<PwCommand>) -> Self {
        Self {
            state,
            tx,
            current_tab: Tab::Outputs,
        }
    }

    fn render_node(&self, ui: &mut egui::Ui, node: &AudioNode, state: &AppState) {
        let mut frame = egui::Frame::group(ui.style());
        if node.is_default {
            frame = frame.fill(ui.visuals().selection.bg_fill.linear_multiply(0.1));
            frame = frame.stroke(egui::Stroke::new(1.0, ui.visuals().selection.bg_fill));
        }

        frame.show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&node.description).strong()).truncate(),
                    );

                    if node.is_stream {
                        let target_node = if let Some(target_id) = node.target_id {
                            state.nodes.get(&target_id)
                        } else {
                            // If no target_id, try to find the default node
                            let default_name = if node.is_sink {
                                state.default_sink_name.as_ref()
                            } else {
                                state.default_source_name.as_ref()
                            };

                            default_name.and_then(|name| {
                                state.nodes.values().find(|n| n.name == *name)
                            })
                        };

                        if let Some(target) = target_node {
                            let prefix = if node.is_sink {
                                "on"
                            } else if target.media_class == "Audio/Sink" {
                                "from Monitor of"
                            } else {
                                "from"
                            };
                            ui.label(egui::RichText::new(format!(" {} {}", prefix, target.description)).small().weak());
                        }
                    }
                });

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

                    if !node.is_stream {
                        if ui.selectable_label(is_default, "Default").clicked() {
                            let _ = self.tx.send(PwCommand::SetDefault(node.id));
                        }
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

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Copper Mixer");
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.current_tab, Tab::Outputs, "Outputs");
                ui.selectable_value(&mut self.current_tab, Tab::Inputs, "Inputs");
                ui.selectable_value(&mut self.current_tab, Tab::Playback, "Playback");
                ui.selectable_value(&mut self.current_tab, Tab::Recording, "Recording");
                ui.selectable_value(&mut self.current_tab, Tab::Configuration, "Configuration");
            });

            ui.add_space(10.0);

            let mut state = self.state.lock();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    match self.current_tab {
                        Tab::Outputs => {
                            let mut sinks: Vec<&AudioNode> = state
                                .nodes
                                .values()
                                .filter(|n| n.is_sink && !n.is_stream)
                                .collect();
                            sinks.sort_by_key(|n| n.id);

                            if sinks.is_empty() {
                                ui.label("No output devices found");
                            } else {
                                for node in sinks {
                                    self.render_node(ui, node, &state);
                                }
                            }
                        }
                        Tab::Inputs => {
                            let mut sources: Vec<&AudioNode> = state
                                .nodes
                                .values()
                                .filter(|n| !n.is_sink && !n.is_stream)
                                .collect();
                            sources.sort_by_key(|n| n.id);

                            if sources.is_empty() {
                                ui.label("No input devices found");
                            } else {
                                for node in sources {
                                    self.render_node(ui, node, &state);
                                }
                            }
                        }
                        Tab::Playback => {
                            let mut playback: Vec<&AudioNode> = state
                                .nodes
                                .values()
                                .filter(|n| n.is_stream && n.is_sink)
                                .collect();
                            playback.sort_by_key(|n| n.id);

                            if playback.is_empty() {
                                ui.label("No playback streams found");
                            } else {
                                for node in playback {
                                    self.render_node(ui, node, &state);
                                }
                            }
                        }
                        Tab::Recording => {
                            let mut recording: Vec<&AudioNode> = state
                                .nodes
                                .values()
                                .filter(|n| n.is_stream && !n.is_sink)
                                .collect();
                            recording.sort_by_key(|n| n.id);

                            if recording.is_empty() {
                                ui.label("No recording streams found");
                            } else {
                                for node in recording {
                                    self.render_node(ui, node, &state);
                                }
                            }
                        }
                        Tab::Configuration => {
                            let mut cards: Vec<&crate::state::Card> = state.cards.values().collect();
                            cards.sort_by_key(|c| c.id);

                            if cards.is_empty() {
                                ui.label("No audio cards found");
                            } else {
                                for card in cards {
                                    if state.hide_unavailable_profiles && card.profiles.iter().all(|p| !p.available) {
                                        continue;
                                    }
                                    self.render_card(ui, card, &state);
                                }
                            }
                        }
                    }
                });

            ui.separator();
            ui.horizontal(|ui| {
                ui.checkbox(&mut state.show_volume_meters, "Show volume meters");
                ui.checkbox(&mut state.hide_unavailable_profiles, "Hide unavailable card profiles");
            });
        });
    }
}

impl CopperApp {
    fn render_card(&self, ui: &mut egui::Ui, card: &crate::state::Card, state: &AppState) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&card.description).strong());
                });

                ui.horizontal(|ui| {
                    ui.label("Profile:");
                    let current_profile_name = card
                        .active_profile_index
                        .and_then(|idx| card.profiles.iter().find(|p| p.index == idx))
                        .map(|p| p.description.clone())
                        .unwrap_or_else(|| "Unknown".to_string());

                    egui::ComboBox::from_id_salt(card.id)
                        .selected_text(current_profile_name)
                        .show_ui(ui, |ui| {
                            for profile in &card.profiles {
                                if state.hide_unavailable_profiles && !profile.available {
                                    continue;
                                }

                                let mut label = profile.description.clone();
                                if !profile.available {
                                    label.push_str(" (unavailable)");
                                }

                                let is_selected = card.active_profile_index == Some(profile.index);
                                if ui.selectable_label(is_selected, label).clicked() {
                                    let _ = self.tx.send(PwCommand::SetCardProfile(card.id, profile.index));
                                }
                            }
                        });
                });
            });
        });
    }
}


