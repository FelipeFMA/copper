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
    pub media_class: String,
    pub channel_count: u32,
    pub device_id: Option<u32>,
    pub target_id: Option<u32>,
    pub route_index: Option<u32>,
    pub route_device: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct Card {
    pub id: u32,
    pub description: String,
    pub profiles: Vec<Profile>,
    pub active_profile_index: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub index: u32,
    pub description: String,
    pub available: bool,
}

pub struct AppState {
    pub nodes: HashMap<u32, AudioNode>,
    pub cards: HashMap<u32, Card>,
    pub default_sink_name: Option<String>,
    pub default_source_name: Option<String>,
    pub show_volume_meters: bool,
    pub hide_unavailable_profiles: bool,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            cards: HashMap::new(),
            default_sink_name: None,
            default_source_name: None,
            show_volume_meters: true,
            hide_unavailable_profiles: false,
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
    SetCardProfile(u32, u32),
    Quit,
}
