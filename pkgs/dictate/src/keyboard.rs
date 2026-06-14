// A persistent Wayland virtual-keyboard client. Unlike spawning `wtype` per
// edit, this holds one connection open and sends key events directly — no
// process spawn per keystroke, and arbitrary event sequences (type, backspace,
// cursor moves) in order.
//
// We upload our own XKB keymap that assigns each character/special-key its own
// keycode at a single level, so "typing" is just pressing the keycode for the
// desired keysym — no layout or modifier juggling.

use std::collections::HashMap;
use std::os::fd::AsFd;

use anyhow::{Result, anyhow};
use rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    protocol::{wl_registry, wl_seat},
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};

#[derive(Default)]
struct Globals {
    manager: Option<ZwpVirtualKeyboardManagerV1>,
    seat: Option<wl_seat::WlSeat>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Globals {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "zwp_virtual_keyboard_manager_v1" => {
                    state.manager = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "wl_seat" => {
                    state.seat = Some(registry.bind(name, version.min(7), qh, ()));
                }
                _ => {}
            }
        }
    }
}

// We don't act on events from these — empty handlers.
macro_rules! ignore_events {
    ($t:ty) => {
        impl Dispatch<$t, ()> for Globals {
            fn event(
                _: &mut Self,
                _: &$t,
                _: <$t as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    };
}
ignore_events!(wl_seat::WlSeat);
ignore_events!(ZwpVirtualKeyboardManagerV1);
ignore_events!(ZwpVirtualKeyboardV1);

pub struct Keyboard {
    conn: Connection,
    vkbd: ZwpVirtualKeyboardV1,
    codes: HashMap<String, u32>, // keysym name -> evdev keycode (XKB keycode - 8)
    time: u32,                   // strictly-increasing event timestamp
}

impl Keyboard {
    pub fn new() -> Result<Self> {
        let conn = Connection::connect_to_env()?;
        let mut queue = conn.new_event_queue::<Globals>();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let mut globals = Globals::default();
        queue.roundtrip(&mut globals)?;

        let manager = globals
            .manager
            .ok_or_else(|| anyhow!("compositor has no zwp_virtual_keyboard_manager_v1"))?;
        let seat = globals.seat.ok_or_else(|| anyhow!("no wl_seat"))?;
        let vkbd = manager.create_virtual_keyboard(&seat, &qh, ());

        let mut kb = Keyboard {
            conn,
            vkbd,
            codes: HashMap::new(),
            time: 0,
        };
        // Pre-assign the keys we use most so we rarely rebuild the keymap:
        // the cursor/edit keys plus all printable ASCII.
        for name in ["BackSpace", "Left", "Right", "Delete", "Home", "End"] {
            kb.assign(name.to_string());
        }
        for c in ' '..='~' {
            kb.assign(format!("U{:04X}", c as u32));
        }
        kb.upload_keymap()?;
        Ok(kb)
    }

    fn assign(&mut self, sym: String) -> u32 {
        let next = self.codes.len() as u32 + 1; // 1-based; keycode 0 is reserved
        *self.codes.entry(sym).or_insert(next)
    }

    fn build_keymap(&self) -> String {
        let mut entries: Vec<(&String, u32)> = self.codes.iter().map(|(s, &c)| (s, c)).collect();
        entries.sort_by_key(|&(_, c)| c);
        let mut keycodes = String::new();
        let mut symbols = String::new();
        let mut max = 8;
        for (sym, evdev) in &entries {
            let xkb = evdev + 8;
            max = max.max(xkb);
            keycodes.push_str(&format!("    <K{xkb}> = {xkb};\n"));
            symbols.push_str(&format!("    key <K{xkb}> {{ [ {sym} ] }};\n"));
        }
        format!(
            "xkb_keymap {{\n  xkb_keycodes \"dictate\" {{\n    minimum = 8;\n    maximum = {max};\n{keycodes}  }};\n  xkb_types \"dictate\" {{ include \"complete\" }};\n  xkb_compatibility \"dictate\" {{ include \"complete\" }};\n  xkb_symbols \"dictate\" {{\n{symbols}  }};\n}};\n"
        )
    }

    fn upload_keymap(&self) -> Result<()> {
        let mut data = self.build_keymap().into_bytes();
        data.push(0); // keymap is read as a NUL-terminated string
        let fd = memfd_create("dictate-keymap", MemfdFlags::empty())?;
        let mut off = 0;
        while off < data.len() {
            off += rustix::io::write(&fd, &data[off..])?;
        }
        self.vkbd.keymap(1 /* XKB_V1 */, fd.as_fd(), data.len() as u32);
        self.conn.flush()?;
        Ok(())
    }

    // Press and release the key for `sym`, uploading a new keymap first if we
    // haven't seen this keysym before.
    fn tap(&mut self, sym: &str) -> Result<()> {
        let code = if let Some(&c) = self.codes.get(sym) {
            c
        } else {
            let c = self.assign(sym.to_string());
            self.upload_keymap()?;
            c
        };
        self.vkbd.key(self.time, code, 1);
        self.vkbd.key(self.time + 1, code, 0);
        self.time += 2;
        Ok(())
    }

    pub fn type_text(&mut self, text: &str) -> Result<()> {
        for c in text.chars() {
            self.tap(&format!("U{:04X}", c as u32))?;
        }
        self.conn.flush()?;
        Ok(())
    }

    // Press a named special key (e.g. "BackSpace", "Left", "Right"), `n` times.
    pub fn press(&mut self, name: &str, n: usize) -> Result<()> {
        for _ in 0..n {
            self.tap(name)?;
        }
        self.conn.flush()?;
        Ok(())
    }

    // Re-anchor the cursor to the end of the line before an edit. The field is
    // a single line of transcript, so End lands on the true end regardless of
    // where the previous reconcile left the cursor — drift can't compound.
    pub fn to_end(&mut self) -> Result<()> {
        self.press("End", 1)
    }

    // Carry out a sequence of cursor ops (the diff applied to the field).
    pub fn apply(&mut self, ops: &[crate::Op]) -> Result<()> {
        for op in ops {
            match op {
                crate::Op::Left(n) => self.press("Left", *n)?,
                crate::Op::Right(n) => self.press("Right", *n)?,
                crate::Op::Back(n) => self.press("BackSpace", *n)?,
                crate::Op::Type(t) => self.type_text(t)?,
            }
        }
        Ok(())
    }
}
