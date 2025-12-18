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
    proxy: pw::node::Node,
    _listener: Box<dyn pw::proxy::Listener>,
}

struct DeviceWrapper {
    proxy: pw::device::Device,
    _listener: Box<dyn pw::proxy::Listener>,
}

struct MetadataWrapper {
    proxy: pw::metadata::Metadata,
    _listener: Box<dyn pw::proxy::Listener>,
}

type NodeMap = Rc<RefCell<HashMap<u32, NodeWrapper>>>;
type DeviceMap = Rc<RefCell<HashMap<u32, DeviceWrapper>>>;
type MetadataMap = Rc<RefCell<HashMap<u32, MetadataWrapper>>>;

/// Main PipeWire thread entry point.
pub fn run(
    state: Arc<Mutex<AppState>>,
    rx: Receiver<PwCommand>,
    repaint_ctx: Arc<Mutex<Option<egui::Context>>>,
) {
    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None).expect("Failed to create MainLoop");
    let context = pw::context::ContextRc::new(&mainloop, None).expect("Failed to create Context");
    let core = context.connect_rc(None).expect("Failed to connect to Core");
    let registry = core.get_registry_rc().expect("Failed to get Registry");

    let nodes: NodeMap = Rc::new(RefCell::new(HashMap::new()));
    let devices: DeviceMap = Rc::new(RefCell::new(HashMap::new()));
    let metadata: MetadataMap = Rc::new(RefCell::new(HashMap::new()));

    // Setup registry listener
    let _registry_listener = {
        let registry_clone = registry.clone();
        let state_add = state.clone();
        let repaint_add = repaint_ctx.clone();
        let nodes_add = nodes.clone();
        let devices_add = devices.clone();
        let metadata_add = metadata.clone();

        let state_remove = state.clone();
        let repaint_remove = repaint_ctx.clone();
        let nodes_remove = nodes.clone();
        let devices_remove = devices.clone();
        let metadata_remove = metadata.clone();

        registry
            .add_listener_local()
            .global(move |global| {
                handle_global_add(global, &registry_clone, &state_add, &repaint_add, &nodes_add, &devices_add, &metadata_add);
            })
            .global_remove(move |id| {
                handle_global_remove(id, &state_remove, &repaint_remove, &nodes_remove, &devices_remove, &metadata_remove);
            })
            .register()
    };

    // Setup command timer
    let timer = {
        let rx = rx.clone();
        let state = state.clone();
        let devices = devices.clone();
        let metadata = metadata.clone();

        mainloop.loop_().add_timer(move |_| {
            process_commands(&rx, &state, &nodes, &devices, &metadata);
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
    registry: &pw::registry::RegistryRc,
    state: &Arc<Mutex<AppState>>,
    repaint: &Arc<Mutex<Option<egui::Context>>>,
    nodes: &NodeMap,
    devices: &DeviceMap,
    metadata: &MetadataMap,
) {
    let Some(props) = global.props else { return };

    if global.type_ == pw::types::ObjectType::Device {
        handle_device(global, props, registry, state, repaint, devices);
    } else if global.type_ == pw::types::ObjectType::Metadata {
        handle_metadata(global, props, registry, state, repaint, metadata);
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
    metadata: &MetadataMap,
) {
    nodes.borrow_mut().remove(&id);
    devices.borrow_mut().remove(&id);
    metadata.borrow_mut().remove(&id);

    let mut s = state.lock();
    if s.nodes.remove(&id).is_some() {
        request_repaint(repaint);
    }
}

// --- Device Handling ---

fn handle_device(
    global: &pw::registry::GlobalObject<&pw::spa::utils::dict::DictRef>,
    props: &pw::spa::utils::dict::DictRef,
    registry: &pw::registry::RegistryRc,
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

// --- Metadata Handling ---

fn handle_metadata(
    global: &pw::registry::GlobalObject<&pw::spa::utils::dict::DictRef>,
    props: &pw::spa::utils::dict::DictRef,
    registry: &pw::registry::RegistryRc,
    state: &Arc<Mutex<AppState>>,
    repaint: &Arc<Mutex<Option<egui::Context>>>,
    metadata: &MetadataMap,
) {
    let name = props.get("metadata.name").unwrap_or("");
    if name != "default" {
        return;
    }

    let id = global.id;
    let proxy: pw::metadata::Metadata = registry.bind(global).expect("Failed to bind metadata");

    let state_clone = state.clone();
    let repaint_clone = repaint.clone();

    let listener = proxy
        .add_listener_local()
        .property(move |subject, key, _type, value| {
            if let Some(key) = key {
                on_metadata_property(subject, key, value, &state_clone, &repaint_clone);
            }
            0
        })
        .register();

    metadata.borrow_mut().insert(
        id,
        MetadataWrapper {
            proxy,
            _listener: Box::new(listener),
        },
    );
}

fn on_metadata_property(
    _subject: u32,
    key: &str,
    value: Option<&str>,
    state: &Arc<Mutex<AppState>>,
    repaint: &Arc<Mutex<Option<egui::Context>>>,
) {
    if key != "default.audio.sink" && key != "default.audio.source" {
        return;
    }

    let node_name = value.and_then(|v| {
        if v.starts_with('{') {
            // Simple JSON parsing for {"name": "..."}
            v.split("\"name\":\"")
                .nth(1)
                .and_then(|s| s.split('\"').next())
        } else {
            Some(v)
        }
    });

    let mut s = state.lock();
    let is_sink = key == "default.audio.sink";

    if is_sink {
        s.default_sink_name = node_name.map(|n| n.to_string());
    } else {
        s.default_source_name = node_name.map(|n| n.to_string());
    }

    for node in s.nodes.values_mut() {
        if node.is_sink == is_sink {
            node.is_default = Some(node.name.as_str()) == node_name;
        }
    }

    request_repaint(repaint);
}

// --- Node Handling ---

fn handle_node(
    global: &pw::registry::GlobalObject<&pw::spa::utils::dict::DictRef>,
    props: &pw::spa::utils::dict::DictRef,
    registry: &pw::registry::RegistryRc,
    state: &Arc<Mutex<AppState>>,
    repaint: &Arc<Mutex<Option<egui::Context>>>,
    nodes: &NodeMap,
) {
    let media_class = props.get("media.class").unwrap_or("");
    let is_sink = media_class == "Audio/Sink";
    let is_source = media_class == "Audio/Source";
    let is_playback = media_class == "Stream/Output/Audio";
    let is_recording = media_class == "Stream/Input/Audio";

    if !is_sink && !is_source && !is_playback && !is_recording {
        return;
    }

    let id = global.id;
    let name = props.get("node.name").unwrap_or("Unknown").to_string();
    let mut description = props.get("node.description").unwrap_or(&name).to_string();

    if is_playback || is_recording {
        if let Some(app_name) = props.get("application.name") {
            if !description.contains(app_name) {
                description = format!("{}: {}", app_name, description);
            }
        }
    }

    let device_id = props.get("device.id").and_then(|s| s.parse::<u32>().ok());

    {
        let mut s = state.lock();
        let is_default = if is_sink {
            s.default_sink_name.as_ref() == Some(&name)
        } else if is_source {
            s.default_source_name.as_ref() == Some(&name)
        } else {
            false
        };

        s.nodes.insert(
            id,
            AudioNode {
                id,
                name,
                description,
                volume: 1.0,
                muted: false,
                is_sink: is_sink || is_playback,
                is_stream: is_playback || is_recording,
                is_default,
                media_class: media_class.to_string(),
                channel_count: 2,
                device_id,
                target_id: props
                    .get("target.node")
                    .or_else(|| props.get("node.target"))
                    .and_then(|s| s.parse::<u32>().ok()),
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
            proxy: node,
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

fn process_commands(rx: &Receiver<PwCommand>, state: &Arc<Mutex<AppState>>, nodes: &NodeMap, devices: &DeviceMap, metadata: &MetadataMap) {
    while let Ok(cmd) = rx.try_recv() {
        match cmd {
            PwCommand::Quit => std::process::exit(0),
            PwCommand::SetVolume(node_id, vol) => set_volume(node_id, vol, state, nodes, devices),
            PwCommand::SetMute(node_id, mute) => set_mute(node_id, mute, state, nodes, devices),
            PwCommand::SetDefault(node_id) => set_default(node_id, state, metadata),
        }
    }
}

fn set_default(node_id: u32, state: &Arc<Mutex<AppState>>, metadata: &MetadataMap) {
    let (name, is_sink) = {
        let s = state.lock();
        let Some(node) = s.nodes.get(&node_id) else { return };
        (node.name.clone(), node.is_sink)
    };

    let metadata = metadata.borrow();
    let Some(wrapper) = metadata.values().next() else { return };

    let key = if is_sink {
        "default.audio.sink"
    } else {
        "default.audio.source"
    };

    let value = format!("{{\"name\": \"{}\"}}", name);
    wrapper.proxy.set_property(0, key, Some("Spa:String:JSON"), Some(&value));
}

fn set_volume(node_id: u32, vol: f32, state: &Arc<Mutex<AppState>>, nodes: &NodeMap, devices: &DeviceMap) {
    let (is_stream, channel_count, device_id, route_index, route_device) = {
        let s = state.lock();
        let Some(node) = s.nodes.get(&node_id) else { return };
        (node.is_stream, node.channel_count, node.device_id, node.route_index, node.route_device)
    };

    if is_stream {
        let nodes = nodes.borrow();
        let Some(wrapper) = nodes.get(&node_id) else { return };
        if let Some(buf) = spa::build_props_volume_pod(channel_count, vol, None) {
            if let Some(pod) = spa_lib::pod::Pod::from_bytes(&buf) {
                wrapper.proxy.set_param(spa_lib::param::ParamType::Props, 0, pod);
            }
        }
    } else {
        let (Some(device_id), Some(route_index), Some(route_device)) = (device_id, route_index, route_device) else { return };
        let devices = devices.borrow();
        let Some(wrapper) = devices.get(&device_id) else { return };

        if let Some(buf) = spa::build_route_volume_pod(route_index, route_device, channel_count, vol, None) {
            if let Some(pod) = spa_lib::pod::Pod::from_bytes(&buf) {
                wrapper.proxy.set_param(spa_lib::param::ParamType::Route, 0, pod);
            }
        }
    }
}

fn set_mute(node_id: u32, mute: bool, state: &Arc<Mutex<AppState>>, nodes: &NodeMap, devices: &DeviceMap) {
    let (is_stream, channel_count, volume, device_id, route_index, route_device) = {
        let s = state.lock();
        let Some(node) = s.nodes.get(&node_id) else { return };
        (node.is_stream, node.channel_count, node.volume, node.device_id, node.route_index, node.route_device)
    };

    if is_stream {
        let nodes = nodes.borrow();
        let Some(wrapper) = nodes.get(&node_id) else { return };
        if let Some(buf) = spa::build_props_volume_pod(channel_count, volume, Some(mute)) {
            if let Some(pod) = spa_lib::pod::Pod::from_bytes(&buf) {
                wrapper.proxy.set_param(spa_lib::param::ParamType::Props, 0, pod);
            }
        }
    } else {
        let (Some(device_id), Some(route_index), Some(route_device)) = (device_id, route_index, route_device) else { return };
        let devices = devices.borrow();
        let Some(wrapper) = devices.get(&device_id) else { return };

        if let Some(buf) = spa::build_route_volume_pod(route_index, route_device, channel_count, volume, Some(mute)) {
            if let Some(pod) = spa_lib::pod::Pod::from_bytes(&buf) {
                wrapper.proxy.set_param(spa_lib::param::ParamType::Route, 0, pod);
            }
        }
    }
}

// --- Helpers ---

fn request_repaint(repaint: &Arc<Mutex<Option<egui::Context>>>) {
    if let Some(ctx) = repaint.lock().as_ref() {
        ctx.request_repaint();
    }
}
