//! Media on a device: the gate to a library (gate) and the libraries as
//! folders (mount). Shared by `dd` and the app.

pub mod gate;
#[cfg(not(target_os = "android"))]
pub mod mount;
