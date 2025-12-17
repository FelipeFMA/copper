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
use std::time::Duration;
use std::mem::MaybeUninit;

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
    device_id: Option<u32>,
    route_index: Option<u32>,
    route_device: Option<u32>,
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
    _proxy: pw::node::Node,
    _listener: Box<dyn pw::proxy::Listener>,
}

struct DeviceWrapper {
    proxy: pw::device::Device,
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

// --- SPA POD Parsing Helpers ---

const SPA_PROP_VOLUME: u32 = 65539;
const SPA_PROP_MUTE: u32 = 65540;
const SPA_PROP_CHANNEL_VOLUMES: u32 = 65544;

struct ParsedNodeProps {
    volume: Option<f32>,
    muted: Option<bool>,
    channel_count: Option<u32>,
}

struct ParsedRouteInfo {
    route_index: u32,
    route_device: u32,
    direction: u32,
    volume: Option<f32>,
    muted: Option<bool>,
    channel_count: Option<u32>,
}

unsafe fn read_float_array_first(pod: *mut spa_sys::spa_pod) -> Option<(f32, u32)> {
    if (*pod).type_ != spa_sys::SPA_TYPE_Array {
        return None;
    }
    
    let array = pod as *mut spa_sys::spa_pod_array;
    let body = &(*array).body;
    
    if (*body).child.type_ != spa_sys::SPA_TYPE_Float {
        return None;
    }
    
    let pod_size = (*array).pod.size;
    let body_size = std::mem::size_of::<spa_sys::spa_pod_array_body>() as u32;
    
    if pod_size <= body_size {
        return None;
    }
    
    let count = (pod_size - body_size) / 4;
    let data_ptr = (body as *const _ as *const u8).add(body_size as usize);
    let value = *(data_ptr as *const f32);
    
    Some((value, count))
}

unsafe fn parse_props_from_pod(pod: *mut spa_sys::spa_pod) -> ParsedNodeProps {
    let mut result = ParsedNodeProps {
        volume: None,
        muted: None,
        channel_count: None,
    };
    
    if (*pod).type_ != spa_sys::SPA_TYPE_Object {
        return result;
    }
    
    let obj = pod as *mut spa_sys::spa_pod_object;
    let body = &(*obj).body;
    let size = (*obj).pod.size;
    let mut iter = spa_sys::spa_pod_prop_first(body);
    
    while spa_sys::spa_pod_prop_is_inside(body, size, iter) {
        let key = (*iter).key;
        let value_ptr = &mut (*iter).value as *mut spa_sys::spa_pod;
        
        match key {
            SPA_PROP_CHANNEL_VOLUMES => {
                if let Some((vol, count)) = read_float_array_first(value_ptr) {
                    result.volume = Some(vol);
                    result.channel_count = Some(count);
                }
            }
            SPA_PROP_VOLUME if result.volume.is_none() => {
                let mut f: f32 = 0.0;
                if spa_sys::spa_pod_get_float(value_ptr, &mut f) >= 0 {
                    result.volume = Some(f);
                }
            }
            SPA_PROP_MUTE => {
                let mut b: bool = false;
                if spa_sys::spa_pod_get_bool(value_ptr, &mut b) >= 0 {
                    result.muted = Some(b);
                }
            }
            _ => {}
        }
        
        iter = spa_sys::spa_pod_prop_next(iter);
    }
    
    result
}

unsafe fn parse_route_param(pod: *const spa_sys::spa_pod) -> Option<ParsedRouteInfo> {
    if (*pod).type_ != spa_sys::SPA_TYPE_Object {
        return None;
    }
    
    let obj = pod as *mut spa_sys::spa_pod_object;
    let body = &(*obj).body;
    let size = (*obj).pod.size;
    let mut iter = spa_sys::spa_pod_prop_first(body);
    
    let mut route_index = None;
    let mut route_device = None;
    let mut direction = None;
    let mut volume = None;
    let mut muted = None;
    let mut channel_count = None;
    
    while spa_sys::spa_pod_prop_is_inside(body, size, iter) {
        let key = (*iter).key;
        let value_ptr = &mut (*iter).value as *mut spa_sys::spa_pod;
        
        match key {
            1 => { // Route index
                let mut i: i32 = 0;
                if spa_sys::spa_pod_get_int(value_ptr, &mut i) >= 0 {
                    route_index = Some(i as u32);
                }
            }
            2 => { // Direction
                let mut i: u32 = 0;
                if spa_sys::spa_pod_get_id(value_ptr, &mut i) >= 0 {
                    direction = Some(i);
                }
            }
            3 => { // Route device
                let mut i: i32 = 0;
                if spa_sys::spa_pod_get_int(value_ptr, &mut i) >= 0 {
                    route_device = Some(i as u32);
                }
            }
            10 => { // Props
                let props = parse_props_from_pod(value_ptr);
                volume = props.volume;
                muted = props.muted;
                channel_count = props.channel_count;
            }
            _ => {}
        }
        
        iter = spa_sys::spa_pod_prop_next(iter);
    }
    
    match (route_index, route_device, direction) {
        (Some(ri), Some(rd), Some(dir)) => Some(ParsedRouteInfo {
            route_index: ri,
            route_device: rd,
            direction: dir,
            volume,
            muted,
            channel_count,
        }),
        _ => None,
    }
}

// --- SPA POD Building ---

fn build_route_volume_pod(
    route_index: u32,
    route_device: u32,
    channel_count: u32,
    volume: f32,
    mute: Option<bool>,
) -> Option<Vec<u8>> {
    let vol_linear = volume.powi(3);
    let channels = channel_count.max(2) as usize;
    
    let mut buf = Vec::with_capacity(1024);
    let mut builder = spa::pod::builder::Builder::new(&mut buf);
    
    unsafe {
        let mut frame: MaybeUninit<spa_sys::spa_pod_frame> = MaybeUninit::uninit();
        
        // Route object type = 262153
        builder.push_object(&mut frame, 262153, spa::param::ParamType::Route.as_raw()).ok()?;
        
        // Route index (key 1)
        builder.add_prop(1, 0).ok()?;
        builder.add_int(route_index as i32).ok()?;
        
        // Route device (key 3)
        builder.add_prop(3, 0).ok()?;
        builder.add_int(route_device as i32).ok()?;
        
        // Props object (key 10)
        builder.add_prop(10, 0).ok()?;
        
        let mut props_frame: MaybeUninit<spa_sys::spa_pod_frame> = MaybeUninit::uninit();
        // Props object type = 262146
        builder.push_object(&mut props_frame, 262146, spa::param::ParamType::Route.as_raw()).ok()?;
        
        // channelVolumes
        builder.add_prop(SPA_PROP_CHANNEL_VOLUMES, 0).ok()?;
        let floats: Vec<f32> = vec![vol_linear; channels];
        spa_sys::spa_pod_builder_array(
            builder.as_raw() as *const _ as *mut _,
            4,
            spa_sys::SPA_TYPE_Float,
            floats.len() as u32,
            floats.as_ptr() as *const std::ffi::c_void,
        );
        
        // mute (optional)
        if let Some(m) = mute {
            builder.add_prop(SPA_PROP_MUTE, 0).ok()?;
            builder.add_bool(m).ok()?;
        }
        
        builder.pop(&mut props_frame.assume_init());
        
        // save = true (key 13)
        builder.add_prop(13, 0).ok()?;
        builder.add_bool(true).ok()?;
        
        builder.pop(&mut frame.assume_init());
    }
    
    Some(buf)
}

fn set_device_route_volume(
    device: &pw::device::Device,
    route_index: u32,
    route_device: u32,
    channel_count: u32,
    volume: f32,
    mute: Option<bool>,
) {
    let Some(buf) = build_route_volume_pod(route_index, route_device, channel_count, volume, mute) else {
        return;
    };
    
    if let Some(pod) = spa::pod::Pod::from_bytes(&buf) {
        device.set_param(spa::param::ParamType::Route, 0, pod);
    }
}

// --- PipeWire Thread ---

fn pw_thread(
    state: Arc<Mutex<AppState>>,
    rx: Receiver<PwCommand>,
    repaint_ctx: Arc<Mutex<Option<egui::Context>>>,
) {
    pw::init();

    let mainloop = pw::main_loop::MainLoop::new(None).expect("Failed to create MainLoop");
    let context = pw::context::Context::new(&mainloop).expect("Failed to create Context");
    let core = context.connect(None).expect("Failed to connect to Core");
    let registry = Rc::new(core.get_registry().expect("Failed to get Registry"));

    let nodes: Rc<RefCell<HashMap<u32, NodeWrapper>>> = Rc::new(RefCell::new(HashMap::new()));
    let devices: Rc<RefCell<HashMap<u32, DeviceWrapper>>> = Rc::new(RefCell::new(HashMap::new()));

    let _registry_listener = {
        let registry_clone = registry.clone();
        let state_global = state.clone();
        let repaint_global = repaint_ctx.clone();
        let nodes_global = nodes.clone();
        let devices_global = devices.clone();
        let state_remove = state.clone();
        let repaint_remove = repaint_ctx.clone();
        let nodes_remove = nodes.clone();
        let devices_remove = devices.clone();

        registry
            .add_listener_local()
            .global(move |global| {
                handle_global(
                    global,
                    &registry_clone,
                    &state_global,
                    &repaint_global,
                    &nodes_global,
                    &devices_global,
                );
            })
            .global_remove(move |id| {
                handle_global_remove(id, &state_remove, &repaint_remove, &nodes_remove, &devices_remove);
            })
            .register()
    };

    let timer = {
        let devices_cmd = devices.clone();
        let state_cmd = state.clone();

        mainloop.loop_().add_timer(move |_| {
            process_commands(&rx, &state_cmd, &devices_cmd);
        })
    };

    timer
        .update_timer(Some(Duration::from_millis(1)), Some(Duration::from_millis(50)))
        .into_result()
        .unwrap();

    mainloop.run();
}

fn handle_global(
    global: &pw::registry::GlobalObject<&pw::spa::utils::dict::DictRef>,
    registry: &Rc<pw::registry::Registry>,
    state: &Arc<Mutex<AppState>>,
    repaint: &Arc<Mutex<Option<egui::Context>>>,
    nodes: &Rc<RefCell<HashMap<u32, NodeWrapper>>>,
    devices: &Rc<RefCell<HashMap<u32, DeviceWrapper>>>,
) {
    let Some(props) = global.props else { return };

    if *&global.type_ == pw::types::ObjectType::Device {
        handle_device_global(global, props, registry, state, repaint, devices);
    } else {
        handle_node_global(global, props, registry, state, repaint, nodes);
    }
}

fn handle_device_global(
    global: &pw::registry::GlobalObject<&pw::spa::utils::dict::DictRef>,
    props: &pw::spa::utils::dict::DictRef,
    registry: &Rc<pw::registry::Registry>,
    state: &Arc<Mutex<AppState>>,
    repaint: &Arc<Mutex<Option<egui::Context>>>,
    devices: &Rc<RefCell<HashMap<u32, DeviceWrapper>>>,
) {
    let media_class = props.get("media.class").unwrap_or("");
    if media_class != "Audio/Device" {
        return;
    }

    let device_id = global.id;
    let device: pw::device::Device = registry.bind(global).expect("Failed to bind device");

    let state_clone = state.clone();
    let repaint_clone = repaint.clone();

    let listener = device
        .add_listener_local()
        .param(move |_seq, param_id, _index, _next, param| {
            if param_id != spa::param::ParamType::Route {
                return;
            }
            let Some(param) = param else { return };

            let Some(route_info) = (unsafe { parse_route_param(param.as_raw_ptr()) }) else {
                return;
            };

            update_node_from_route(device_id, &route_info, &state_clone, &repaint_clone);
        })
        .register();

    device.subscribe_params(&[spa::param::ParamType::Route]);

    devices.borrow_mut().insert(device_id, DeviceWrapper {
        proxy: device,
        _listener: Box::new(listener),
    });
}

fn handle_node_global(
    global: &pw::registry::GlobalObject<&pw::spa::utils::dict::DictRef>,
    props: &pw::spa::utils::dict::DictRef,
    registry: &Rc<pw::registry::Registry>,
    state: &Arc<Mutex<AppState>>,
    repaint: &Arc<Mutex<Option<egui::Context>>>,
    nodes: &Rc<RefCell<HashMap<u32, NodeWrapper>>>,
) {
    let media_class = props.get("media.class").unwrap_or("");
    if media_class != "Audio/Sink" && media_class != "Audio/Source" {
        return;
    }

    let id = global.id;
    let name = props.get("node.name").unwrap_or("Unknown").to_string();
    let description = props.get("node.description").unwrap_or(&name).to_string();
    let is_sink = media_class == "Audio/Sink";
    let device_id = props.get("device.id").and_then(|s| s.parse::<u32>().ok());

    {
        let mut s = state.lock();
        s.nodes.insert(id, AudioNode {
            id,
            name,
            description,
            volume: 1.0,
            muted: false,
            is_sink,
            channel_count: 2,
            device_id,
            route_index: None,
            route_device: None,
        });
    }

    if let Some(ctx) = repaint.lock().as_ref() {
        ctx.request_repaint();
    }

    let node: pw::node::Node = registry.bind(global).expect("Failed to bind node");

    let state_clone = state.clone();
    let repaint_clone = repaint.clone();

    let listener = node
        .add_listener_local()
        .param(move |_seq, _id, _index, _next, param| {
            let Some(param) = param else { return };
            let props = unsafe { parse_props_from_pod(param.as_raw_ptr() as *mut _) };
            update_node_props(id, &props, &state_clone, &repaint_clone);
        })
        .register();

    node.subscribe_params(&[spa::param::ParamType::Props]);

    nodes.borrow_mut().insert(id, NodeWrapper {
        _proxy: node,
        _listener: Box::new(listener),
    });
}

fn handle_global_remove(
    id: u32,
    state: &Arc<Mutex<AppState>>,
    repaint: &Arc<Mutex<Option<egui::Context>>>,
    nodes: &Rc<RefCell<HashMap<u32, NodeWrapper>>>,
    devices: &Rc<RefCell<HashMap<u32, DeviceWrapper>>>,
) {
    nodes.borrow_mut().remove(&id);
    devices.borrow_mut().remove(&id);

    let mut s = state.lock();
    if s.nodes.remove(&id).is_some() {
        if let Some(ctx) = repaint.lock().as_ref() {
            ctx.request_repaint();
        }
    }
}

fn update_node_props(
    node_id: u32,
    props: &ParsedNodeProps,
    state: &Arc<Mutex<AppState>>,
    repaint: &Arc<Mutex<Option<egui::Context>>>,
) {
    if props.volume.is_none() && props.muted.is_none() && props.channel_count.is_none() {
        return;
    }

    let mut s = state.lock();
    if let Some(node) = s.nodes.get_mut(&node_id) {
        if let Some(v) = props.volume {
            node.volume = v.cbrt();
        }
        if let Some(m) = props.muted {
            node.muted = m;
        }
        if let Some(c) = props.channel_count {
            node.channel_count = c;
        }
    }

    if let Some(ctx) = repaint.lock().as_ref() {
        ctx.request_repaint();
    }
}

fn update_node_from_route(
    device_id: u32,
    route: &ParsedRouteInfo,
    state: &Arc<Mutex<AppState>>,
    repaint: &Arc<Mutex<Option<egui::Context>>>,
) {
    let mut s = state.lock();

    for node in s.nodes.values_mut() {
        if node.device_id != Some(device_id) {
            continue;
        }

        // Direction: 0 = Input (source), 1 = Output (sink)
        let matches = (route.direction == 1 && node.is_sink) || (route.direction == 0 && !node.is_sink);
        if !matches {
            continue;
        }

        node.route_index = Some(route.route_index);
        node.route_device = Some(route.route_device);

        if let Some(v) = route.volume {
            node.volume = v.cbrt();
        }
        if let Some(m) = route.muted {
            node.muted = m;
        }
        if let Some(c) = route.channel_count {
            node.channel_count = c;
        }
    }

    if let Some(ctx) = repaint.lock().as_ref() {
        ctx.request_repaint();
    }
}

fn process_commands(
    rx: &Receiver<PwCommand>,
    state: &Arc<Mutex<AppState>>,
    devices: &Rc<RefCell<HashMap<u32, DeviceWrapper>>>,
) {
    while let Ok(cmd) = rx.try_recv() {
        match cmd {
            PwCommand::Quit => std::process::exit(0),
            PwCommand::SetVolume(node_id, vol) => {
                let info = get_node_route_info(node_id, state);
                if let Some((device_id, route_index, route_device, channel_count)) = info {
                    if let Some(wrapper) = devices.borrow().get(&device_id) {
                        set_device_route_volume(&wrapper.proxy, route_index, route_device, channel_count, vol, None);
                    }
                }
            }
            PwCommand::SetMute(node_id, mute) => {
                let info = get_node_route_info_with_volume(node_id, state);
                if let Some((device_id, route_index, route_device, channel_count, volume)) = info {
                    if let Some(wrapper) = devices.borrow().get(&device_id) {
                        set_device_route_volume(&wrapper.proxy, route_index, route_device, channel_count, volume, Some(mute));
                    }
                }
            }
        }
    }
}

fn get_node_route_info(node_id: u32, state: &Arc<Mutex<AppState>>) -> Option<(u32, u32, u32, u32)> {
    let s = state.lock();
    let node = s.nodes.get(&node_id)?;
    Some((node.device_id?, node.route_index?, node.route_device?, node.channel_count))
}

fn get_node_route_info_with_volume(node_id: u32, state: &Arc<Mutex<AppState>>) -> Option<(u32, u32, u32, u32, f32)> {
    let s = state.lock();
    let node = s.nodes.get(&node_id)?;
    Some((node.device_id?, node.route_index?, node.route_device?, node.channel_count, node.volume))
}
