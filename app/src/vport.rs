//! The virtual "PIONEER DDJ-SX" MIDI device that rekordbox talks to.
//!
//! * macOS (and Linux, for development): CoreMIDI / ALSA virtual endpoints
//!   are built into the OS, nothing to install.
//! * Windows has no built-in way for an app to publish a MIDI device, so
//!   RekordJog uses the free virtualMIDI driver that ships with loopMIDI.
//!   RekordJog creates the port itself through the driver; loopMIDI does
//!   not need to be running and no port needs to be created by hand.
//!   If the driver can't create the port but a loopMIDI port with the
//!   right name already exists, RekordJog falls back to sending into it.

use rekordjog_core::ddj::PORT_NAME;
use serde::Serialize;

pub type RxCallback = Box<dyn Fn(&[u8]) + Send + Sync + 'static>;

pub trait VirtualPort: Send {
    fn send(&mut self, bytes: &[u8]) -> bool;
    /// Human-readable description of how the port is provided.
    fn kind(&self) -> &'static str;
    /// Whether rekordbox's LED messages reach us.
    fn has_feedback(&self) -> bool;
}

#[allow(dead_code)] // some variants only occur on Windows
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum VportError {
    /// Windows: the virtualMIDI driver (installed with loopMIDI) is missing.
    DriverMissing,
    /// Windows: a port called PIONEER DDJ-SX already exists elsewhere.
    NameInUse,
    Other(String),
}

pub fn create(on_rx: RxCallback) -> Result<Box<dyn VirtualPort>, VportError> {
    imp::create(on_rx)
}

/// Returns true if a MIDI port name looks like one we must never treat as
/// the controller (our own virtual device).
pub fn is_own_port(name: &str) -> bool {
    name.to_ascii_lowercase().contains("ddj-sx")
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    use midir::os::unix::{VirtualInput, VirtualOutput};
    use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};

    struct UnixPort {
        out: MidiOutputConnection,
        _inp: MidiInputConnection<()>,
    }

    impl VirtualPort for UnixPort {
        fn send(&mut self, bytes: &[u8]) -> bool {
            self.out.send(bytes).is_ok()
        }
        fn kind(&self) -> &'static str {
            if cfg!(target_os = "macos") {
                "built-in CoreMIDI virtual device"
            } else {
                "ALSA virtual device"
            }
        }
        fn has_feedback(&self) -> bool {
            true
        }
    }

    pub fn create(on_rx: RxCallback) -> Result<Box<dyn VirtualPort>, VportError> {
        let err = |e: &dyn std::fmt::Display| VportError::Other(e.to_string());
        let out = MidiOutput::new("RekordJog").map_err(|e| err(&e))?;
        let out = out.create_virtual(PORT_NAME).map_err(|e| err(&e))?;
        let inp = MidiInput::new("RekordJog").map_err(|e| err(&e))?;
        let inp = inp
            .create_virtual(PORT_NAME, move |_t, msg, _| on_rx(msg), ())
            .map_err(|e| err(&e))?;
        Ok(Box::new(UnixPort { out, _inp: inp }))
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use libloading::Library;
    use midir::{MidiOutput, MidiOutputConnection};
    use std::ffi::c_void;

    type Callback = unsafe extern "system" fn(port: *mut c_void, data: *const u8, len: u32, ctx: usize);
    type CreatePortEx2 =
        unsafe extern "system" fn(name: *const u16, cb: Option<Callback>, ctx: usize, max_sysex: u32, flags: u32) -> *mut c_void;
    type ClosePort = unsafe extern "system" fn(port: *mut c_void);
    type SendData = unsafe extern "system" fn(port: *mut c_void, data: *const u8, len: u32) -> i32;

    const TE_VM_FLAGS_PARSE_RX: u32 = 1;
    const TE_VM_FLAGS_INSTANTIATE_BOTH: u32 = 12;
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const ERROR_PATH_NOT_FOUND: i32 = 3;
    const ERROR_ALREADY_EXISTS: i32 = 183;
    const ERROR_REVISION_MISMATCH: i32 = 1306;

    struct Ctx {
        on_rx: RxCallback,
    }

    unsafe extern "system" fn rx_trampoline(_port: *mut c_void, data: *const u8, len: u32, ctx: usize) {
        if data.is_null() || len == 0 || ctx == 0 {
            return; // port shutting down
        }
        let ctx = &*(ctx as *const Ctx);
        let msg = std::slice::from_raw_parts(data, len as usize);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (ctx.on_rx)(msg)));
    }

    struct TePort {
        _lib: Library,
        port: *mut c_void,
        close: ClosePort,
        send: SendData,
        ctx: *mut Ctx,
    }

    // The driver handle is thread-safe; we only ever use it behind a mutex.
    unsafe impl Send for TePort {}

    impl VirtualPort for TePort {
        fn send(&mut self, bytes: &[u8]) -> bool {
            unsafe { (self.send)(self.port, bytes.as_ptr(), bytes.len() as u32) != 0 }
        }
        fn kind(&self) -> &'static str {
            "virtualMIDI driver (from loopMIDI)"
        }
        fn has_feedback(&self) -> bool {
            true
        }
    }

    impl Drop for TePort {
        fn drop(&mut self) {
            unsafe {
                (self.close)(self.port);
                drop(Box::from_raw(self.ctx));
            }
        }
    }

    /// Fallback: an existing loopMIDI port named PIONEER DDJ-SX. A loopback
    /// port echoes everything, so LED feedback is not used in this mode.
    struct LoopMidiPort {
        out: MidiOutputConnection,
    }

    impl VirtualPort for LoopMidiPort {
        fn send(&mut self, bytes: &[u8]) -> bool {
            self.out.send(bytes).is_ok()
        }
        fn kind(&self) -> &'static str {
            "existing loopMIDI port (basic mode, no LEDs)"
        }
        fn has_feedback(&self) -> bool {
            false
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn create_te(on_rx: RxCallback) -> Result<TePort, (VportError, RxCallback)> {
        let dll = if cfg!(target_pointer_width = "64") { "teVirtualMIDI64.dll" } else { "teVirtualMIDI32.dll" };
        let lib = match unsafe { Library::new(dll) } {
            Ok(l) => l,
            Err(_) => return Err((VportError::DriverMissing, on_rx)),
        };
        let (create, close, send) = unsafe {
            let create = lib.get::<CreatePortEx2>(b"virtualMIDICreatePortEx2\0").map(|s| *s);
            let close = lib.get::<ClosePort>(b"virtualMIDIClosePort\0").map(|s| *s);
            let send = lib.get::<SendData>(b"virtualMIDISendData\0").map(|s| *s);
            match (create, close, send) {
                (Ok(a), Ok(b), Ok(c)) => (a, b, c),
                _ => return Err((VportError::Other("virtualMIDI driver is too old; reinstall loopMIDI".into()), on_rx)),
            }
        };
        let ctx = Box::into_raw(Box::new(Ctx { on_rx }));
        let name = wide(PORT_NAME);
        let mut last = 0;
        for flags in [TE_VM_FLAGS_PARSE_RX | TE_VM_FLAGS_INSTANTIATE_BOTH, TE_VM_FLAGS_PARSE_RX] {
            let port = unsafe { create(name.as_ptr(), Some(rx_trampoline), ctx as usize, 65535, flags) };
            if !port.is_null() {
                return Ok(TePort { _lib: lib, port, close, send, ctx });
            }
            last = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if last == ERROR_ALREADY_EXISTS {
                break;
            }
        }
        let ctx = unsafe { Box::from_raw(ctx) };
        let err = match last {
            ERROR_ALREADY_EXISTS => VportError::NameInUse,
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => VportError::DriverMissing,
            ERROR_REVISION_MISMATCH => VportError::Other("virtualMIDI driver version mismatch; reinstall loopMIDI".into()),
            n => VportError::Other(format!("virtualMIDI driver error {n}")),
        };
        Err((err, ctx.on_rx))
    }

    fn open_loopmidi() -> Option<LoopMidiPort> {
        let out = MidiOutput::new("RekordJog").ok()?;
        let port = out.ports().into_iter().find(|p| out.port_name(p).map(|n| is_own_port(&n)).unwrap_or(false))?;
        let conn = out.connect(&port, "RekordJog").ok()?;
        Some(LoopMidiPort { out: conn })
    }

    pub fn create(on_rx: RxCallback) -> Result<Box<dyn VirtualPort>, VportError> {
        match create_te(on_rx) {
            Ok(p) => Ok(Box::new(p)),
            Err((e, _)) => match open_loopmidi() {
                Some(p) => Ok(Box::new(p)),
                None => Err(e),
            },
        }
    }
}
