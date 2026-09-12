use nkscan::device::{self, Device};

/// Returns all discovered Nikon Coolscan devices.
pub fn list_scanners() -> Vec<Device> {
    device::list()
}

/// Finds the first available Nikon Coolscan device.
pub fn first_scanner() -> Option<Device> {
    device::list().into_iter().next()
}
