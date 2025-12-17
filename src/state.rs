use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct AudioNode {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub volume: f32,
    pub muted: bool,
    pub is_sink: bool,
    pub channel_count: u32,
    pub device_id: Option<u32>,
    pub route_index: Option<u32>,
    pub route_device: Option<u32>,
}

pub struct AppState {
    pub nodes: HashMap<u32, AudioNode>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
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
    Quit,
}
