//! PipeWire backend for audio device management.

mod spa;

use crate::state::{AppState, AudioNode, PwCommand};
use crossbeam_channel::Receiver;
use eframe::egui;
use libspa as spa_lib;
use parking_lot::Mutex;
use pipewire as pw;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

struct NodeWrapper {
    _proxy: pw::node::Node,
    _listener: Box<dyn pw::proxy::Listener>,
}

struct DeviceWrapper {
    proxy: pw::device::Device,
    _listener: Box<dyn pw::proxy::Listener>,
}

type NodeMap = Rc<RefCell<HashMap<u32, NodeWrapper>>>;
type DeviceMap = Rc<RefCell<HashMap<u32, DeviceWrapper>>>;

/// Main PipeWire thread entry point.
pub fn run(
    state: Arc<Mutex<AppState>>,
    rx: Receiver<PwCommand>,
    repaint_ctx: Arc<Mutex<Option<egui::Context>>>,
) {
    pw::init();

    let mainloop = pw::main_loop::MainLoop::new(None).expect("Failed to create MainLoop");
    let context = pw::context::Context::new(&mainloop).expect("Failed to create Context");
    let core = context.connect(None).expect("Failed to connect to Core");
    let registry = Rc::new(core.get_registry().expect("Failed to get Registry"));

    let nodes: NodeMap = Rc::new(RefCell::new(HashMap::new()));
    let devices: DeviceMap = Rc::new(RefCell::new(HashMap::new()));

    // Setup registry listener
    let _registry_listener = {
        let registry_clone = registry.clone();
        let state_add = state.clone();
        let repaint_add = repaint_ctx.clone();
        let nodes_add = nodes.clone();
        let devices_add = devices.clone();

        let state_remove = state.clone();
        let repaint_remove = repaint_ctx.clone();
        let nodes_remove = nodes.clone();
        let devices_remove = devices.clone();

        registry
            .add_listener_local()
            .global(move |global| {
                handle_global_add(global, &registry_clone, &state_add, &repaint_add, &nodes_add, &devices_add);
            })
            .global_remove(move |id| {
                handle_global_remove(id, &state_remove, &repaint_remove, &nodes_remove, &devices_remove);
            })
            .register()
    };

    // Setup command timer
    let timer = {
        let rx = rx.clone();
        let state = state.clone();
        let devices = devices.clone();

        mainloop.loop_().add_timer(move |_| {
            process_commands(&rx, &state, &devices);
        })
    };

    timer
        .update_timer(Some(Duration::from_millis(1)), Some(Duration::from_millis(50)))
        .into_result()
        .unwrap();

    mainloop.run();
}

// --- Global Handlers ---

fn handle_global_add(
    global: &pw::registry::GlobalObject<&pw::spa::utils::dict::DictRef>,
    registry: &Rc<pw::registry::Registry>,
    state: &Arc<Mutex<AppState>>,
    repaint: &Arc<Mutex<Option<egui::Context>>>,
    nodes: &NodeMap,
    devices: &DeviceMap,
) {
    let Some(props) = global.props else { return };

    if global.type_ == pw::types::ObjectType::Device {
        handle_device(global, props, registry, state, repaint, devices);
    } else {
        handle_node(global, props, registry, state, repaint, nodes);
    }
}

fn handle_global_remove(
    id: u32,
    state: &Arc<Mutex<AppState>>,
    repaint: &Arc<Mutex<Option<egui::Context>>>,
    nodes: &NodeMap,
    devices: &DeviceMap,
) {
    nodes.borrow_mut().remove(&id);
    devices.borrow_mut().remove(&id);

    let mut s = state.lock();
    if s.nodes.remove(&id).is_some() {
        request_repaint(repaint);
    }
}

// --- Device Handling ---

fn handle_device(
    global: &pw::registry::GlobalObject<&pw::spa::utils::dict::DictRef>,
    props: &pw::spa::utils::dict::DictRef,
    registry: &Rc<pw::registry::Registry>,
    state: &Arc<Mutex<AppState>>,
    repaint: &Arc<Mutex<Option<egui::Context>>>,
    devices: &DeviceMap,
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
            on_device_param(device_id, param_id, param, &state_clone, &repaint_clone);
        })
        .register();

    device.subscribe_params(&[spa_lib::param::ParamType::Route]);

    devices.borrow_mut().insert(
        device_id,
        DeviceWrapper {
            proxy: device,
            _listener: Box::new(listener),
        },
    );
}

fn on_device_param(
    device_id: u32,
    param_id: spa_lib::param::ParamType,
    param: Option<&spa_lib::pod::Pod>,
    state: &Arc<Mutex<AppState>>,
    repaint: &Arc<Mutex<Option<egui::Context>>>,
) {
    if param_id != spa_lib::param::ParamType::Route {
        return;
    }

    let Some(param) = param else { return };
    let Some(route) = (unsafe { spa::parse_route(param.as_raw_ptr()) }) else { return };

    update_node_from_route(device_id, &route, state);
    request_repaint(repaint);
}

fn update_node_from_route(device_id: u32, route: &spa::ParsedRoute, state: &Arc<Mutex<AppState>>) {
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
}

// --- Node Handling ---

fn handle_node(
    global: &pw::registry::GlobalObject<&pw::spa::utils::dict::DictRef>,
    props: &pw::spa::utils::dict::DictRef,
    registry: &Rc<pw::registry::Registry>,
    state: &Arc<Mutex<AppState>>,
    repaint: &Arc<Mutex<Option<egui::Context>>>,
    nodes: &NodeMap,
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
        s.nodes.insert(
            id,
            AudioNode {
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
            },
        );
    }

    request_repaint(repaint);

    let node: pw::node::Node = registry.bind(global).expect("Failed to bind node");

    let state_clone = state.clone();
    let repaint_clone = repaint.clone();

    let listener = node
        .add_listener_local()
        .param(move |_seq, _id, _index, _next, param| {
            on_node_param(id, param, &state_clone, &repaint_clone);
        })
        .register();

    node.subscribe_params(&[spa_lib::param::ParamType::Props]);

    nodes.borrow_mut().insert(
        id,
        NodeWrapper {
            _proxy: node,
            _listener: Box::new(listener),
        },
    );
}

fn on_node_param(
    node_id: u32,
    param: Option<&spa_lib::pod::Pod>,
    state: &Arc<Mutex<AppState>>,
    repaint: &Arc<Mutex<Option<egui::Context>>>,
) {
    let Some(param) = param else { return };
    let props = unsafe { spa::parse_props(param.as_raw_ptr() as *mut _) };

    if props.volume.is_none() && props.muted.is_none() && props.channel_count.is_none() {
        return;
    }

    {
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
    }

    request_repaint(repaint);
}

// --- Command Processing ---

fn process_commands(rx: &Receiver<PwCommand>, state: &Arc<Mutex<AppState>>, devices: &DeviceMap) {
    while let Ok(cmd) = rx.try_recv() {
        match cmd {
            PwCommand::Quit => std::process::exit(0),
            PwCommand::SetVolume(node_id, vol) => set_volume(node_id, vol, state, devices),
            PwCommand::SetMute(node_id, mute) => set_mute(node_id, mute, state, devices),
        }
    }
}

fn set_volume(node_id: u32, vol: f32, state: &Arc<Mutex<AppState>>, devices: &DeviceMap) {
    let info = {
        let s = state.lock();
        s.nodes.get(&node_id).and_then(|n| {
            Some((n.device_id?, n.route_index?, n.route_device?, n.channel_count))
        })
    };

    let Some((device_id, route_index, route_device, channel_count)) = info else { return };

    let devices = devices.borrow();
    let Some(wrapper) = devices.get(&device_id) else { return };

    let Some(buf) = spa::build_route_volume_pod(route_index, route_device, channel_count, vol, None) else {
        return;
    };

    if let Some(pod) = spa_lib::pod::Pod::from_bytes(&buf) {
        wrapper.proxy.set_param(spa_lib::param::ParamType::Route, 0, pod);
    }
}

fn set_mute(node_id: u32, mute: bool, state: &Arc<Mutex<AppState>>, devices: &DeviceMap) {
    let info = {
        let s = state.lock();
        s.nodes.get(&node_id).and_then(|n| {
            Some((n.device_id?, n.route_index?, n.route_device?, n.channel_count, n.volume))
        })
    };

    let Some((device_id, route_index, route_device, channel_count, volume)) = info else { return };

    let devices = devices.borrow();
    let Some(wrapper) = devices.get(&device_id) else { return };

    let Some(buf) = spa::build_route_volume_pod(route_index, route_device, channel_count, volume, Some(mute)) else {
        return;
    };

    if let Some(pod) = spa_lib::pod::Pod::from_bytes(&buf) {
        wrapper.proxy.set_param(spa_lib::param::ParamType::Route, 0, pod);
    }
}

// --- Helpers ---

fn request_repaint(repaint: &Arc<Mutex<Option<egui::Context>>>) {
    if let Some(ctx) = repaint.lock().as_ref() {
        ctx.request_repaint();
    }
}
