use eframe::egui;
use pipewire as pw;
use libspa as spa;
use libspa_sys as spa_sys;
use std::sync::Arc;
use parking_lot::Mutex;
use std::collections::HashMap;
use crossbeam_channel::{Sender, Receiver, unbounded};
use std::rc::Rc;
use std::cell::RefCell;
use std::mem::MaybeUninit;
use std::time::Duration;

// --- Data Structures ---

#[derive(Clone, Debug)]
struct AudioNode {
    id: u32,
    name: String,
    description: String,
    volume: f32,
    muted: bool,
    is_sink: bool,
    channel_count: u32,
}

struct AppState {
    nodes: HashMap<u32, AudioNode>,
}

enum PwCommand {
    SetVolume(u32, f32),
    SetMute(u32, bool),
    Quit,
}

struct NodeWrapper {
    proxy: pw::node::Node,
    _listener: Box<dyn pw::proxy::Listener>,
}

// --- Main ---

fn main() -> Result<(), eframe::Error> {
    env_logger::init();

    let state = Arc::new(Mutex::new(AppState {
        nodes: HashMap::new(),
    }));

    let (tx_cmd, rx_cmd) = unbounded::<PwCommand>();
    let repaint_ctx = Arc::new(Mutex::new(None::<egui::Context>));

    let thread_state = state.clone();
    let thread_repaint_ctx = repaint_ctx.clone();

    std::thread::spawn(move || {
        pw_thread(thread_state, rx_cmd, thread_repaint_ctx);
    });

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

// --- GUI ---

struct CopperApp {
    state: Arc<Mutex<AppState>>,
    tx: Sender<PwCommand>,
}

impl CopperApp {
    fn new(state: Arc<Mutex<AppState>>, tx: Sender<PwCommand>) -> Self {
        Self { state, tx }
    }

    fn render_node(&self, ui: &mut egui::Ui, node: &AudioNode) {
        ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            ui.vertical(|ui| {
                ui.add(egui::Label::new(egui::RichText::new(&node.description).strong()).truncate());
                ui.add(egui::Label::new(egui::RichText::new(&node.name).small().weak()).truncate());
                
                ui.horizontal(|ui| {
                    let mut volume_percent = node.volume * 100.0;
                    let muted = node.muted;

                    if ui.selectable_label(muted, "Mute").clicked() {
                        let _ = self.tx.send(PwCommand::SetMute(node.id, !muted));
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
            ui.heading("Copper Mixer");
            ui.add_space(10.0);

            let mut sinks: Vec<&AudioNode> = state.nodes.values().filter(|n| n.is_sink).collect();
            let mut sources: Vec<&AudioNode> = state.nodes.values().filter(|n| !n.is_sink).collect();
            
            sinks.sort_by_key(|n| n.id);
            sources.sort_by_key(|n| n.id);

            if !sinks.is_empty() {
                ui.label(egui::RichText::new("Outputs").strong().color(egui::Color32::from_rgb(212, 115, 49)));
                for node in sinks {
                    self.render_node(ui, node);
                }
                ui.add_space(10.0);
            }

            if !sources.is_empty() {
                ui.label(egui::RichText::new("Inputs").strong().color(egui::Color32::from_rgb(212, 115, 49)));
                for node in sources {
                    self.render_node(ui, node);
                }
            }
        });
    }
}

fn setup_custom_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(egui::Color32::from_gray(240));
    visuals.panel_fill = egui::Color32::from_rgb(20, 20, 25);
    
    let copper = egui::Color32::from_rgb(212, 115, 49);
    visuals.selection.bg_fill = copper;
    visuals.widgets.active.bg_fill = copper;
    visuals.widgets.open.bg_fill = copper;
    visuals.widgets.hovered.bg_fill = copper;
    visuals.widgets.hovered.weak_bg_fill = copper;
    
    ctx.set_visuals(visuals);
}

// --- PipeWire Backend ---

fn pw_thread(state: Arc<Mutex<AppState>>, rx: Receiver<PwCommand>, repaint_ctx: Arc<Mutex<Option<egui::Context>>>) {
    pw::init();

    let mainloop = pw::main_loop::MainLoop::new(None).expect("Failed to create MainLoop");
    let context = pw::context::Context::new(&mainloop).expect("Failed to create Context");
    let core = context.connect(None).expect("Failed to connect to Core");
    let registry = Rc::new(core.get_registry().expect("Failed to get Registry"));

    let nodes: Rc<RefCell<HashMap<u32, NodeWrapper>>> = Rc::new(RefCell::new(HashMap::new()));

    let nodes_clone = nodes.clone();
    let state_clone = state.clone();
    let repaint_clone = repaint_ctx.clone();

    let registry_clone = registry.clone();
    let state_global = state_clone.clone();
    let repaint_global = repaint_clone.clone();
    let nodes_global = nodes_clone.clone();

    let state_remove = state_clone.clone();
    let repaint_remove = repaint_clone.clone();
    let nodes_remove = nodes_clone.clone();

    let _registry_listener = registry
        .add_listener_local()
        .global(move |global| {
            if let Some(props) = global.props {
                let media_class = props.get("media.class").unwrap_or("");
                let is_sink = media_class == "Audio/Sink";
                let is_source = media_class == "Audio/Source";

                if is_sink || is_source {
                    let id = global.id;
                    let name = props.get("node.name").unwrap_or("Unknown").to_string();
                    let description = props.get("node.description").unwrap_or(&name).to_string();

                    {
                        let mut s = state_global.lock();
                        s.nodes.insert(id, AudioNode {
                            id,
                            name,
                            description,
                            volume: 1.0,
                            muted: false,
                            is_sink,
                            channel_count: 0,
                        });
                    }
                    if let Some(ctx) = repaint_global.lock().as_ref() {
                        ctx.request_repaint();
                    }

                    let node: pw::node::Node = registry_clone.bind(global).expect("Failed to bind node");
                    
                    let s_clone = state_global.clone();
                    let r_clone = repaint_global.clone();
                    let id_clone = id;
                    
                    let listener = node
                        .add_listener_local()
                        .param(move |_seq, _id, _index, _next, param| {
                            println!("Param event for node {}, param_id: {:?}", id_clone, _id);
                            if let Some(param) = param {
                                let pod = param.as_raw_ptr();
                                unsafe {
                                    println!("  Param Type: {}", (*pod).type_);
                                    if (*pod).type_ == spa_sys::SPA_TYPE_Object {
                                        let obj = pod as *mut spa_sys::spa_pod_object;
                                        let body = &(*obj).body;
                                        let size = (*obj).pod.size;
                                        
                                        let mut iter = spa_sys::spa_pod_prop_first(body);
                                        
                                        let mut vol = None;
                                        let mut muted = None;
                                        let mut channels = None;

                                        while spa_sys::spa_pod_prop_is_inside(body, size, iter) {
                                            let key = (*iter).key;
                                            let value_ptr = &mut (*iter).value as *mut spa_sys::spa_pod;
                                            let type_ = (*value_ptr).type_;
                                            
                                            // println!("Node {} Param Key: {}, Type: {}", id_clone, key, type_);

                                            // SPA_PROP_volume = 0x10003 (65539)
                                            // SPA_PROP_mute = 0x10004 (65540)
                                            // SPA_PROP_channelVolumes = 0x10008 (65544)

                                            if key == 65544 { // Channel Volumes
                                                if type_ == spa_sys::SPA_TYPE_Array {
                                                    let array = value_ptr as *mut spa_sys::spa_pod_array;
                                                    let body_ptr = &(*array).body;
                                                    let child_type = (*body_ptr).child.type_;
                                                    
                                                    if child_type == spa_sys::SPA_TYPE_Float {
                                                        let pod_size = (*array).pod.size;
                                                        let body_size = std::mem::size_of::<spa_sys::spa_pod_array_body>() as u32;
                                                        // println!("  Array: pod_size={}, body_size={}", pod_size, body_size);
                                                        if pod_size > body_size {
                                                            let count = (pod_size - body_size) / 4;
                                                            channels = Some(count);
                                                            println!("  Detected {} channels for node {}", count, id_clone);
                                                            
                                                            let data_ptr = (body_ptr as *const _ as *const u8).add(body_size as usize);
                                                            let f = *(data_ptr as *const f32);
                                                            vol = Some(f);
                                                        }
                                                    }
                                                }
                                            } else if key == 65539 && vol.is_none() { // Volume (fallback)
                                                let mut f: f32 = 0.0;
                                                if spa_sys::spa_pod_get_float(value_ptr, &mut f) >= 0 {
                                                    vol = Some(f);
                                                }
                                            } else if key == 65540 { // Mute
                                                let mut b: bool = false;
                                                if spa_sys::spa_pod_get_bool(value_ptr, &mut b) >= 0 {
                                                    muted = Some(b);
                                                }
                                            }
                                            
                                            iter = spa_sys::spa_pod_prop_next(iter);
                                        }

                                        if vol.is_some() || muted.is_some() || channels.is_some() {
                                            let mut s = s_clone.lock();
                                            if let Some(n) = s.nodes.get_mut(&id_clone) {
                                                if let Some(v) = vol { 
                                                    // Convert linear to cubic for UI
                                                    n.volume = v.cbrt(); 
                                                }
                                                if let Some(m) = muted { n.muted = m; }
                                                if let Some(c) = channels { n.channel_count = c; }
                                            }
                                            if let Some(ctx) = r_clone.lock().as_ref() {
                                                ctx.request_repaint();
                                            }
                                        }
                                    }
                                }
                            }
                        })
                        .register();

                    // Subscribe to Props (id 2) and Route (id 4)
                    node.subscribe_params(&[spa::param::ParamType::Props, spa::param::ParamType::Route]);

                    nodes_global.borrow_mut().insert(id, NodeWrapper {
                        proxy: node,
                        _listener: Box::new(listener),
                    });
                }
            }
        })
        .global_remove(move |id| {
            nodes_remove.borrow_mut().remove(&id);
            let mut s = state_remove.lock();
            if s.nodes.remove(&id).is_some() {
                if let Some(ctx) = repaint_remove.lock().as_ref() {
                    ctx.request_repaint();
                }
            }
        })
        .register();

    let nodes_cmd = nodes.clone();
    let state_cmd = state.clone();
    
    let timer = mainloop.loop_().add_timer(move |_count| {
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                PwCommand::Quit => {
                    // loop_.quit();
                    std::process::exit(0);
                }
                PwCommand::SetVolume(id, vol) => {
                    // Use wpctl to set volume - this properly notifies WirePlumber and all listeners (like Waybar)
                    let vol_percent = (vol * 100.0).round() as u32;
                    let _ = std::process::Command::new("wpctl")
                        .args(["set-volume", &id.to_string(), &format!("{}%", vol_percent)])
                        .spawn();
                }
                PwCommand::SetMute(id, mute) => {
                    // Use wpctl to set mute - this properly notifies WirePlumber and all listeners
                    let mute_arg = if mute { "1" } else { "0" };
                    let _ = std::process::Command::new("wpctl")
                        .args(["set-mute", &id.to_string(), mute_arg])
                        .spawn();
                }
            }
        }
    });

    timer.update_timer(Some(Duration::from_millis(1)), Some(Duration::from_millis(50))).into_result().unwrap();

    mainloop.run();
}
