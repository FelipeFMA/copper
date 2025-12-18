use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct AudioNode {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub volume: f32,
    pub muted: bool,
    pub is_sink: bool,
    pub is_stream: bool,
    pub is_default: bool,
    pub channel_count: u32,
    pub device_id: Option<u32>,
    pub route_index: Option<u32>,
    pub route_device: Option<u32>,
}

pub struct AppState {
    pub nodes: HashMap<u32, AudioNode>,
    pub default_sink_name: Option<String>,
    pub default_source_name: Option<String>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            default_sink_name: None,
            default_source_name: None,
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

pub enum PwCommand {
    SetVolume(u32, f32),
    SetMute(u32, bool),
    SetDefault(u32),
    Quit,
}
