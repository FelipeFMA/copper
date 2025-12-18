//! SPA POD parsing and building utilities for PipeWire audio control.

use libspa as spa;
use libspa_sys as spa_sys;
use std::mem::MaybeUninit;

// SPA property keys
pub const SPA_PROP_VOLUME: u32 = 65539;
pub const SPA_PROP_MUTE: u32 = 65540;
pub const SPA_PROP_CHANNEL_VOLUMES: u32 = 65544;

// Route parameter keys
const ROUTE_KEY_INDEX: u32 = 1;
const ROUTE_KEY_DIRECTION: u32 = 2;
const ROUTE_KEY_DEVICE: u32 = 3;
const ROUTE_KEY_PROPS: u32 = 10;
const ROUTE_KEY_SAVE: u32 = 13;

// SPA object types
const SPA_TYPE_OBJECT_PROPS: u32 = 262146;
const SPA_TYPE_OBJECT_PARAM_PROFILE: u32 = 262152;
const SPA_TYPE_OBJECT_PARAM_ROUTE: u32 = 262153;

// Profile parameter keys
const PROFILE_KEY_INDEX: u32 = 1;
const PROFILE_KEY_NAME: u32 = 2;
const PROFILE_KEY_DESCRIPTION: u32 = 3;
const PROFILE_KEY_AVAILABLE: u32 = 5;
const PROFILE_KEY_SAVE: u32 = 7;

/// Parsed audio properties from a node or route.
#[derive(Debug, Default)]
pub struct ParsedProps {
    pub volume: Option<f32>,
    pub muted: Option<bool>,
    pub channel_count: Option<u32>,
}

/// Parsed route information from a device.
#[derive(Debug)]
pub struct ParsedRoute {
    pub route_index: u32,
    pub route_device: u32,
    pub direction: u32,
    pub volume: Option<f32>,
    pub muted: Option<bool>,
    pub channel_count: Option<u32>,
}

/// Parsed profile information from a device.
#[derive(Debug)]
pub struct ParsedProfile {
    pub index: u32,
    pub description: String,
    pub available: bool,
}

/// Read the first float value from a SPA float array, returning (value, count).
unsafe fn read_float_array_first(pod: *mut spa_sys::spa_pod) -> Option<(f32, u32)> {
    if unsafe { (*pod).type_ } != spa_sys::SPA_TYPE_Array {
        return None;
    }

    let array = pod as *mut spa_sys::spa_pod_array;
    let body = unsafe { &(*array).body };

    if (*body).child.type_ != spa_sys::SPA_TYPE_Float {
        return None;
    }

    let pod_size = unsafe { (*array).pod.size };
    let body_size = std::mem::size_of::<spa_sys::spa_pod_array_body>() as u32;

    if pod_size <= body_size {
        return None;
    }

    let count = (pod_size - body_size) / 4;
    let data_ptr = unsafe { (body as *const _ as *const u8).add(body_size as usize) };
    let value = unsafe { *(data_ptr as *const f32) };

    Some((value, count))
}

/// Parse audio properties (volume, mute, channel count) from a SPA POD object.
pub unsafe fn parse_props(pod: *mut spa_sys::spa_pod) -> ParsedProps {
    let mut result = ParsedProps::default();

    if unsafe { (*pod).type_ } != spa_sys::SPA_TYPE_Object {
        return result;
    }

    let obj = pod as *mut spa_sys::spa_pod_object;
    let body = unsafe { &(*obj).body };
    let size = unsafe { (*obj).pod.size };
    let mut iter = unsafe { spa_sys::spa_pod_prop_first(body) };

    while unsafe { spa_sys::spa_pod_prop_is_inside(body, size, iter) } {
        let key = unsafe { (*iter).key };
        let value_ptr = unsafe { &mut (*iter).value as *mut spa_sys::spa_pod };

        match key {
            SPA_PROP_CHANNEL_VOLUMES => {
                if let Some((vol, count)) = unsafe { read_float_array_first(value_ptr) } {
                    result.volume = Some(vol);
                    result.channel_count = Some(count);
                }
            }
            SPA_PROP_VOLUME if result.volume.is_none() => {
                let mut f: f32 = 0.0;
                if unsafe { spa_sys::spa_pod_get_float(value_ptr, &mut f) } >= 0 {
                    result.volume = Some(f);
                }
            }
            SPA_PROP_MUTE => {
                let mut b: bool = false;
                if unsafe { spa_sys::spa_pod_get_bool(value_ptr, &mut b) } >= 0 {
                    result.muted = Some(b);
                }
            }
            _ => {}
        }

        iter = unsafe { spa_sys::spa_pod_prop_next(iter) };
    }

    result
}

/// Parse route information from a SPA Route parameter POD.
pub unsafe fn parse_route(pod: *const spa_sys::spa_pod) -> Option<ParsedRoute> {
    if unsafe { (*pod).type_ } != spa_sys::SPA_TYPE_Object {
        return None;
    }

    let obj = pod as *mut spa_sys::spa_pod_object;
    let body = unsafe { &(*obj).body };
    let size = unsafe { (*obj).pod.size };
    let mut iter = unsafe { spa_sys::spa_pod_prop_first(body) };

    let mut route_index = None;
    let mut route_device = None;
    let mut direction = None;
    let mut volume = None;
    let mut muted = None;
    let mut channel_count = None;

    while unsafe { spa_sys::spa_pod_prop_is_inside(body, size, iter) } {
        let key = unsafe { (*iter).key };
        let value_ptr = unsafe { &mut (*iter).value as *mut spa_sys::spa_pod };

        match key {
            ROUTE_KEY_INDEX => {
                let mut i: i32 = 0;
                if unsafe { spa_sys::spa_pod_get_int(value_ptr, &mut i) } >= 0 {
                    route_index = Some(i as u32);
                }
            }
            ROUTE_KEY_DIRECTION => {
                let mut i: u32 = 0;
                if unsafe { spa_sys::spa_pod_get_id(value_ptr, &mut i) } >= 0 {
                    direction = Some(i);
                }
            }
            ROUTE_KEY_DEVICE => {
                let mut i: i32 = 0;
                if unsafe { spa_sys::spa_pod_get_int(value_ptr, &mut i) } >= 0 {
                    route_device = Some(i as u32);
                }
            }
            ROUTE_KEY_PROPS => {
                let props = unsafe { parse_props(value_ptr) };
                volume = props.volume;
                muted = props.muted;
                channel_count = props.channel_count;
            }
            _ => {}
        }

        iter = unsafe { spa_sys::spa_pod_prop_next(iter) };
    }

    Some(ParsedRoute {
        route_index: route_index?,
        route_device: route_device?,
        direction: direction?,
        volume,
        muted,
        channel_count,
    })
}

/// Parse profile information from a SPA Profile parameter POD.
pub unsafe fn parse_profile(pod: *const spa_sys::spa_pod) -> Option<ParsedProfile> {
    if unsafe { (*pod).type_ } != spa_sys::SPA_TYPE_Object {
        return None;
    }

    let obj = pod as *mut spa_sys::spa_pod_object;
    let body = unsafe { &(*obj).body };
    let size = unsafe { (*obj).pod.size };
    let mut iter = unsafe { spa_sys::spa_pod_prop_first(body) };

    let mut index = None;
    let mut description = None;
    let mut available = true;

    while unsafe { spa_sys::spa_pod_prop_is_inside(body, size, iter) } {
        let key = unsafe { (*iter).key };
        let value_ptr = unsafe { &mut (*iter).value as *mut spa_sys::spa_pod };

        match key {
            PROFILE_KEY_INDEX => {
                let mut i: i32 = 0;
                if unsafe { spa_sys::spa_pod_get_int(value_ptr, &mut i) } >= 0 {
                    index = Some(i as u32);
                }
            }
            PROFILE_KEY_NAME => {}
            PROFILE_KEY_DESCRIPTION => {
                let mut s: *const std::os::raw::c_char = std::ptr::null();
                if unsafe { spa_sys::spa_pod_get_string(value_ptr, &mut s) } >= 0 {
                    description = Some(unsafe { std::ffi::CStr::from_ptr(s).to_string_lossy().into_owned() });
                }
            }
            PROFILE_KEY_AVAILABLE => {
                let mut i: u32 = 0;
                if unsafe { spa_sys::spa_pod_get_id(value_ptr, &mut i) } >= 0 {
                    // 0 = No, 1 = Yes, 2 = Unknown
                    available = i != 0;
                }
            }
            _ => {}
        }

        iter = unsafe { spa_sys::spa_pod_prop_next(iter) };
    }

    Some(ParsedProfile {
        index: index?,
        description: description.unwrap_or_default(),
        available,
    })
}

/// Build a Profile parameter POD for setting device profile.
pub fn build_profile_pod(index: u32) -> Option<Vec<u8>> {
    let mut buf = Vec::with_capacity(128);
    let mut builder = spa::pod::builder::Builder::new(&mut buf);

    unsafe {
        let mut frame: MaybeUninit<spa_sys::spa_pod_frame> = MaybeUninit::uninit();

        builder
            .push_object(&mut frame, SPA_TYPE_OBJECT_PARAM_PROFILE, spa::param::ParamType::Profile.as_raw())
            .ok()?;

        // Profile index
        builder.add_prop(PROFILE_KEY_INDEX, 0).ok()?;
        builder.add_int(index as i32).ok()?;

        // Save = true
        builder.add_prop(PROFILE_KEY_SAVE, 0).ok()?;
        builder.add_bool(true).ok()?;

        builder.pop(&mut frame.assume_init());
    }

    Some(buf)
}

/// Build a Route parameter POD for setting device volume.
pub fn build_route_volume_pod(
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

        builder
            .push_object(&mut frame, SPA_TYPE_OBJECT_PARAM_ROUTE, spa::param::ParamType::Route.as_raw())
            .ok()?;

        // Route index
        builder.add_prop(ROUTE_KEY_INDEX, 0).ok()?;
        builder.add_int(route_index as i32).ok()?;

        // Route device
        builder.add_prop(ROUTE_KEY_DEVICE, 0).ok()?;
        builder.add_int(route_device as i32).ok()?;

        // Props object
        builder.add_prop(ROUTE_KEY_PROPS, 0).ok()?;

        let mut props_frame: MaybeUninit<spa_sys::spa_pod_frame> = MaybeUninit::uninit();
        builder
            .push_object(&mut props_frame, SPA_TYPE_OBJECT_PROPS, spa::param::ParamType::Route.as_raw())
            .ok()?;

        // Channel volumes
        builder.add_prop(SPA_PROP_CHANNEL_VOLUMES, 0).ok()?;
        let floats: Vec<f32> = vec![vol_linear; channels];
        spa_sys::spa_pod_builder_array(
            builder.as_raw() as *const _ as *mut _,
            4,
            spa_sys::SPA_TYPE_Float,
            floats.len() as u32,
            floats.as_ptr() as *const std::ffi::c_void,
        );

        // Mute (optional)
        if let Some(m) = mute {
            builder.add_prop(SPA_PROP_MUTE, 0).ok()?;
            builder.add_bool(m).ok()?;
        }

        builder.pop(&mut props_frame.assume_init());

        // Save = true (persist the change)
        builder.add_prop(ROUTE_KEY_SAVE, 0).ok()?;
        builder.add_bool(true).ok()?;

        builder.pop(&mut frame.assume_init());
    }

    Some(buf)
}

/// Build a Props parameter POD for setting node volume.
pub fn build_props_volume_pod(
    channel_count: u32,
    volume: f32,
    mute: Option<bool>,
) -> Option<Vec<u8>> {
    let vol_linear = volume.powi(3);
    let channels = channel_count.max(2) as usize;

    let mut buf = Vec::with_capacity(512);
    let mut builder = spa::pod::builder::Builder::new(&mut buf);

    unsafe {
        let mut frame: MaybeUninit<spa_sys::spa_pod_frame> = MaybeUninit::uninit();

        builder
            .push_object(&mut frame, SPA_TYPE_OBJECT_PROPS, spa::param::ParamType::Props.as_raw())
            .ok()?;

        // Channel volumes
        builder.add_prop(SPA_PROP_CHANNEL_VOLUMES, 0).ok()?;
        let floats: Vec<f32> = vec![vol_linear; channels];
        spa_sys::spa_pod_builder_array(
            builder.as_raw() as *const _ as *mut _,
            4,
            spa_sys::SPA_TYPE_Float,
            floats.len() as u32,
            floats.as_ptr() as *const std::ffi::c_void,
        );

        // Mute (optional)
        if let Some(m) = mute {
            builder.add_prop(SPA_PROP_MUTE, 0).ok()?;
            builder.add_bool(m).ok()?;
        }

        builder.pop(&mut frame.assume_init());
    }

    Some(buf)
}
