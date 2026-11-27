//! ZMK Studio keyboards, served to `Load` and `Board` as Vial reports, the way `FakeIo` serves
//! the memory board. Studio speaks protobuf in SOF/ESC/EOF frames over USB serial or a
//! Bluetooth GATT characteristic (zmkfirmware/zmk-studio-messages). A binding is a behavior and
//! two parameters; the ones QMK has a keycode for get that keycode, and every other one becomes a
//! custom keycode from `0x7E00`, named after its behavior, so the rest of the program sees one
//! keycode space. Each key write saves at once: ZMK stages bindings until `save_changes`, Vial
//! does not.

use super::{
    compress, decode_request, encode_reply, keycodes, keymap_bytes, Ask, Candidate, Io, LayerOp,
    Listing, Reply,
};
use crate::{Error, Result};
use btleplug::api::{
    Central as _, Characteristic, Manager as _, Peripheral as _, RetrievePeripheralsOptions,
    ScanFilter, WriteType,
};
use btleplug::platform::{Manager, Peripheral};
use futures::StreamExt as _;
use std::io::{Read, Write};
use std::sync::{LazyLock, OnceLock};
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;
use uuid::Uuid;

const SOF: u8 = 0xAB;
const ESC: u8 = 0xAC;
const EOF: u8 = 0xAD;

/// `studio.Request` subsystem fields.
const CORE: u64 = 3;
const BEHAVIORS: u64 = 4;
const KEYMAP: u64 = 5;

/// zmk/app/src/studio/uuid.h: the Studio service and its one RPC characteristic.
const SERVICE: Uuid = Uuid::from_u128(0x00000000_0196_6107_c967_c5cfb1c2482a);
const RPC: Uuid = Uuid::from_u128(0x00000001_0196_6107_c967_c5cfb1c2482a);

/// Where the definition's `customKeycodes` start, and how many fit before QMK's next range.
const CUSTOM: u16 = 0x7E00;
const CUSTOM_ROOM: usize = 0x200;
/// More bindings than this from one behavior are not offered in the key list.
const EXTRAS_PER_BEHAVIOR: usize = 32;

/// One Studio call: a subsystem request in, that subsystem's response out.
trait Rpc {
    fn call(&mut self, subsystem: u64, request: &[u8]) -> Result<Vec<u8>>;
}

/// Bytes to and from the keyboard, frames not yet cut.
trait Link {
    fn send(&mut self, bytes: &[u8]) -> Result<()>;
    /// What arrived by `until`, maybe nothing.
    fn recv(&mut self, until: Instant) -> Result<Vec<u8>>;
}

/// Studio calls over any link: numbered requests in frames, answers matched by number.
struct Framed {
    link: Box<dyn Link>,
    next: u64,
    /// How long a reply may take.
    wait: Duration,
    read: Vec<u8>,
    at: usize,
}

impl Rpc for Framed {
    fn call(&mut self, subsystem: u64, request: &[u8]) -> Result<Vec<u8>> {
        let start = Instant::now();
        let result = self.call_once(subsystem, request);
        trace(format_args!(
            "{:>6} ms  subsystem {subsystem} request {request:02x?}  {}",
            start.elapsed().as_millis(),
            match &result {
                Ok(reply) if reply.len() <= 8 => format!("{reply:02x?}"),
                Ok(reply) => format!("{} bytes", reply.len()),
                Err(err) => format!("failed: {err}"),
            }
        ));
        result
    }
}

/// `LAZYKEYMAP_TRACE=file` appends one line per Studio call: how long it took, what was
/// asked, and how it ended.
fn trace(line: std::fmt::Arguments<'_>) {
    static FILE: LazyLock<Option<std::sync::Mutex<std::fs::File>>> = LazyLock::new(|| {
        let path = std::env::var_os("LAZYKEYMAP_TRACE")?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()?;
        Some(std::sync::Mutex::new(file))
    });
    if let Some(file) = FILE.as_ref() {
        if let Ok(mut file) = file.lock() {
            let _ = writeln!(file, "{line}");
        }
    }
}

impl Framed {
    fn call_once(&mut self, subsystem: u64, request: &[u8]) -> Result<Vec<u8>> {
        let id = self.next;
        self.next += 1;
        let mut message = Vec::new();
        put_int(&mut message, 1, id);
        put_bytes(&mut message, subsystem, request);
        let mut frame = vec![SOF];
        for byte in message {
            if matches!(byte, SOF | ESC | EOF) {
                frame.push(ESC);
            }
            frame.push(byte);
        }
        frame.push(EOF);
        self.link.send(&frame)?;
        // `wait` is how long the keyboard may go quiet, not how long a reply may take: over
        // Bluetooth a keymap arrives 20-odd bytes per indication, for seconds.
        let mut deadline = Instant::now() + self.wait;
        loop {
            let response = self.frame(&mut deadline)?;
            // A notification (field 2) is not the answer.
            let Some(answer) = field(&response, 1)? else {
                continue;
            };
            if int(answer, 1)?.unwrap_or(0) != id {
                continue;
            }
            if let Some(meta) = field(answer, 2)? {
                return Err(match int(meta, 2)? {
                    Some(1) => Error::Locked,
                    _ => Error::Protocol("the keyboard refused a ZMK Studio request"),
                });
            }
            return field(answer, subsystem)?
                .map(<[u8]>::to_vec)
                .ok_or(Error::Protocol("ZMK Studio reply"));
        }
    }

    /// The next whole frame. Bytes before a start of frame are dropped. Each arrival moves
    /// `deadline` to `wait` from now.
    fn frame(&mut self, deadline: &mut Instant) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        let (mut inside, mut escaped) = (false, false);
        loop {
            if self.at == self.read.len() {
                if Instant::now() > *deadline {
                    return Err(Error::Disconnected);
                }
                self.read = self.link.recv(*deadline)?;
                if !self.read.is_empty() {
                    *deadline = Instant::now() + self.wait;
                }
                self.at = 0;
                continue;
            }
            let byte = self.read[self.at];
            self.at += 1;
            match byte {
                _ if escaped => {
                    data.push(byte);
                    escaped = false;
                }
                SOF => {
                    inside = true;
                    data.clear();
                }
                ESC if inside => escaped = true,
                EOF if inside => return Ok(data),
                _ if inside => data.push(byte),
                _ => {}
            }
        }
    }
}

/// A Studio keyboard on `link` answers device info within `wait`: its calls, and its name.
fn probe(link: Box<dyn Link>, wait: Duration) -> Option<(Framed, Option<String>)> {
    let mut rpc = Framed {
        link,
        next: 1,
        wait,
        read: Vec::new(),
        at: 0,
    };
    // core get_device_info
    let core = rpc.call(CORE, &[0x08, 0x01]).ok()?;
    let info = field(&core, 1).ok()??;
    let name = text(info, 1).filter(|name| !name.is_empty());
    rpc.wait = Duration::from_secs(3);
    Some((rpc, name))
}

fn candidate(rpc: Framed, listing: Listing) -> Candidate {
    Candidate {
        listing: Box::new(listing),
        io: Some(Box::new(Zmk::new(Box::new(rpc)))),
        guard: None,
    }
}

struct Serial(Box<dyn serialport::SerialPort>);

impl Link for Serial {
    fn send(&mut self, bytes: &[u8]) -> Result<()> {
        self.0.write_all(bytes).map_err(|_| Error::Disconnected)
    }

    fn recv(&mut self, _until: Instant) -> Result<Vec<u8>> {
        let mut chunk = [0u8; 512];
        match self.0.read(&mut chunk) {
            Ok(got) => Ok(chunk[..got].to_vec()),
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut => Ok(Vec::new()),
            Err(_) => Err(Error::Disconnected),
        }
    }
}

/// The serial port `--port` named. It is probed even without ZMK's USB id or maker.
static PORT: OnceLock<String> = OnceLock::new();

pub(in crate::session) fn add_port(path: &str) {
    let _ = PORT.set(path.to_string());
}

/// Studio keyboards on USB serial, then on Bluetooth. Opening a serial port resets an Arduino or
/// a 3D printer, so only ports with ZMK's USB id or maker, or the `--port` one, are opened.
pub(super) fn enumerate() -> Vec<Candidate> {
    let forced = PORT.get();
    let ports = serialport::available_ports().unwrap_or_default();
    let mut found = Vec::new();
    for port in &ports {
        let serialport::SerialPortType::UsbPort(usb) = &port.port_type else {
            continue;
        };
        let zmk = (usb.vid, usb.pid) == (0x1D50, 0x615E)
            || usb
                .manufacturer
                .as_deref()
                .is_some_and(|m| m.contains("ZMK"));
        // macOS lists each port twice; the call-out device does not wait for carrier.
        let named = forced == Some(&port.port_name);
        if !named && (!zmk || port.port_name.starts_with("/dev/tty.")) {
            continue;
        }
        if let Some((rpc, name)) = open_serial(&port.port_name) {
            found.push(candidate(
                rpc,
                Listing {
                    name: name
                        .or(usb.product.clone())
                        .unwrap_or_else(|| "board".to_string()),
                    maker: "ZMK Studio".to_string(),
                    usb: format!("{:04x}:{:04x}", usb.vid, usb.pid),
                    serial: usb.serial_number.clone().unwrap_or_default(),
                },
            ));
        }
    }
    if let Some(path) = forced.filter(|path| !ports.iter().any(|p| &p.port_name == *path)) {
        if let Some((rpc, name)) = open_serial(path) {
            found.push(candidate(
                rpc,
                Listing {
                    name: name.unwrap_or_else(|| "board".to_string()),
                    maker: "ZMK Studio".to_string(),
                    usb: path.clone(),
                    serial: String::new(),
                },
            ));
        }
    }
    found.extend(bluetooth());
    found
}

fn open_serial(path: &str) -> Option<(Framed, Option<String>)> {
    let port = serialport::new(path, 115_200)
        .timeout(Duration::from_millis(50))
        .open()
        .ok()?;
    let _ = port.clear(serialport::ClearBuffer::Input);
    probe(Box::new(Serial(port)), Duration::from_millis(300))
}

/// btleplug is async; one runtime thread serves every Bluetooth keyboard. `None` when it would
/// not start.
static RUNTIME: LazyLock<Option<Runtime>> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .ok()
});

/// The RPC characteristic's indications, in arrival order. btleplug hands them out through a
/// 16-slot broadcast that drops the oldest when the reader falls behind, which would cut a
/// keymap reply; a task moves each one here as it comes.
type Values = tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>;

/// The RPC characteristic, written in pieces that fit the link and answered by indications.
struct Ble {
    runtime: &'static Runtime,
    peripheral: Peripheral,
    rpc: Characteristic,
    values: Values,
}

impl Link for Ble {
    fn send(&mut self, bytes: &[u8]) -> Result<()> {
        let size = usize::from(self.peripheral.mtu().saturating_sub(3)).max(20);
        // A write the keyboard never acknowledges would otherwise wait forever.
        let sent = self.runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(3), async {
                for piece in bytes.chunks(size) {
                    self.peripheral
                        .write(&self.rpc, piece, WriteType::WithResponse)
                        .await?;
                }
                Ok::<_, btleplug::Error>(())
            })
            .await
        });
        trace(format_args!(
            "        bluetooth write {} bytes in pieces of {size}: {}",
            bytes.len(),
            match &sent {
                Ok(Ok(())) => "ok".to_string(),
                Ok(Err(err)) => format!("failed: {err}"),
                Err(_) => "no acknowledgement in 3 s".to_string(),
            }
        ));
        match sent {
            Ok(Ok(())) => Ok(()),
            _ => Err(Error::Disconnected),
        }
    }

    fn recv(&mut self, until: Instant) -> Result<Vec<u8>> {
        let wait = until.saturating_duration_since(Instant::now());
        match self
            .runtime
            .block_on(async { tokio::time::timeout(wait, self.values.recv()).await })
        {
            Err(_) => Ok(Vec::new()),
            Ok(Some(value)) => Ok(value),
            Ok(None) => Err(Error::Disconnected),
        }
    }
}

/// Bluetooth keyboards with the Studio service. Studio answers only on the output the keyboard
/// is using, so a board typing over USB is found on serial, not here.
fn bluetooth() -> Vec<Candidate> {
    let Some(runtime) = RUNTIME.as_ref() else {
        return Vec::new();
    };
    // A tokio timer is made inside the runtime, so the `async` around it.
    let Ok(links) = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(8), studio_peripherals()).await
    }) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (peripheral, rpc, values, local) in links {
        let serial = peripheral.id().to_string();
        let link = Ble {
            runtime,
            peripheral,
            rpc,
            values,
        };
        if let Some((rpc, name)) = probe(Box::new(link), Duration::from_secs(2)) {
            found.push(candidate(
                rpc,
                Listing {
                    name: name.or(local).unwrap_or_else(|| "board".to_string()),
                    maker: "ZMK Studio".to_string(),
                    usb: "bluetooth".to_string(),
                    serial,
                },
            ));
        }
    }
    found
}

/// The paired keyboards the system is connected to that have the Studio service, each
/// subscribed to its RPC characteristic. One that will not connect is left out.
async fn studio_peripherals() -> Vec<(Peripheral, Characteristic, Values, Option<String>)> {
    let Ok(manager) = Manager::new().await else {
        return Vec::new();
    };
    // macOS answers nothing here while Bluetooth is off or this terminal may not use it.
    let adapters = tokio::time::timeout(Duration::from_millis(1500), manager.adapters()).await;
    let mut out = Vec::new();
    for adapter in adapters
        .ok()
        .and_then(|found| found.ok())
        .unwrap_or_default()
    {
        let options = RetrievePeripheralsOptions {
            identifiers: None,
            services: Some(vec![SERVICE]),
        };
        let peripherals = match adapter.retrieve_peripherals(options).await {
            Ok(found) => found,
            // ponytail: no connected-device lookup (Windows); a short scan finds a board that
            // advertises the service. A paired board that stays quiet there is not found.
            Err(_) => scan(&adapter).await,
        };
        trace(format_args!(
            "bluetooth: {} connected with the Studio service",
            peripherals.len()
        ));
        for peripheral in peripherals {
            match subscribe(&peripheral).await {
                Ok((rpc, values, local)) => out.push((peripheral, rpc, values, local)),
                Err(err) => trace(format_args!(
                    "bluetooth {}: could not open the Studio service: {err}",
                    peripheral.id()
                )),
            }
        }
    }
    out
}

async fn scan(adapter: &btleplug::platform::Adapter) -> Vec<Peripheral> {
    let filter = ScanFilter {
        services: vec![SERVICE],
    };
    if adapter.start_scan(filter).await.is_err() {
        return Vec::new();
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    let _ = adapter.stop_scan().await;
    adapter.peripherals().await.unwrap_or_default()
}

async fn subscribe(
    peripheral: &Peripheral,
) -> btleplug::Result<(Characteristic, Values, Option<String>)> {
    if !peripheral.is_connected().await? {
        peripheral.connect().await?;
    }
    peripheral.discover_services().await?;
    let rpc = peripheral
        .characteristics()
        .into_iter()
        .find(|c| c.uuid == RPC)
        .ok_or(btleplug::Error::NoSuchCharacteristic)?;
    let mut stream = peripheral.notifications().await?;
    let (sender, values) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(value) = stream.next().await {
            if value.uuid == RPC && sender.send(value.value).is_err() {
                return;
            }
        }
    });
    peripheral.subscribe(&rpc).await?;
    let local = peripheral.properties().await?.and_then(|p| p.local_name);
    Ok((rpc, values, local))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Binding {
    behavior: i32,
    param1: u32,
    param2: u32,
}

struct Behavior {
    id: i32,
    name: String,
    /// Each set is the values param1 may take, then param2.
    sets: Vec<[Vec<Param>; 2]>,
}

enum Param {
    Nil,
    Constant(u32, String),
    Range(i32, i32),
    Usage,
    Layer,
}

struct Layer {
    id: u32,
    name: String,
    bindings: Vec<Binding>,
}

/// A Studio keyboard read at the first Vial report, then kept in step with each write.
struct Zmk {
    rpc: Box<dyn Rpc>,
    behaviors: Vec<Behavior>,
    layers: Vec<Layer>,
    /// Bindings with no QMK keycode. Index `n` is keycode `CUSTOM + n`.
    custom: Vec<Binding>,
    /// The Vial definition made from the active physical layout, XZ-compressed.
    compressed: Vec<u8>,
    /// Physical layout names, and which one is active.
    layouts: Vec<String>,
    layout: usize,
    /// The next piece of the first read.
    reading: Reading,
}

enum Reading {
    Lock,
    Behaviors,
    /// The ids from the list; `behaviors` holds the ones read so far.
    Details(Vec<u64>),
    Keymap,
    Layout,
    Done,
}

impl Io for Zmk {
    fn exchange(&mut self, report: &[u8; 32]) -> Result<[u8; 32]> {
        let ask = decode_request(report)?;
        let reply = self.answer(&ask)?;
        Ok(encode_reply(&ask, &reply))
    }

    fn speaks(&self, code: u16) -> bool {
        self.binding(code).is_some()
    }

    /// A failed piece starts the read over, so Enter on the card tries again from the lock.
    fn busy(&mut self) -> Result<Option<String>> {
        let step = self.read_next();
        if step.is_err() {
            self.reading = Reading::Lock;
        }
        step
    }

    fn layer_names(&self) -> Vec<String> {
        self.layers.iter().map(|l| l.name.clone()).collect()
    }

    fn keymap(&self) -> Vec<Vec<u16>> {
        self.layers
            .iter()
            .map(|layer| {
                layer
                    .bindings
                    .iter()
                    .map(|binding| self.code(*binding).unwrap_or(0))
                    .collect()
            })
            .collect()
    }

    fn edit_layers(&mut self, op: &LayerOp) -> Result<()> {
        let done = self.layer_call(op);
        // Read again from the lock, whether the call worked or not.
        self.reading = Reading::Lock;
        done
    }

    fn layouts(&self) -> Vec<String> {
        self.layouts.clone()
    }

    fn layout_at(&self) -> usize {
        self.layout
    }

    fn set_layout(&mut self, index: usize) -> Result<()> {
        if self.layouts.len() < 2 {
            return Err(Error::Protocol("this keyboard has one layout"));
        }
        if index >= self.layouts.len() {
            return Err(Error::Protocol("no such layout"));
        }
        if index == self.layout {
            return Ok(());
        }
        let done = self.layout_call(index);
        self.reading = Reading::Lock;
        done
    }

    fn intern(&mut self, text: &str) -> Option<(u16, String)> {
        let binding = self.parse_binding(text)?;
        if self.code(binding).is_none() {
            if self.custom.len() >= CUSTOM_ROOM {
                return None;
            }
            self.custom.push(binding);
        }
        let code = self.code(binding)?;
        let name = if code >= CUSTOM {
            self.name(binding)
        } else {
            String::new()
        };
        Some((code, name))
    }

    fn binding_text(&self, code: u16) -> Option<String> {
        self.text(self.binding(code)?)
    }

    fn complete(&self, typed: &str) -> Option<super::file::Completion> {
        if self.behaviors.is_empty() {
            return None;
        }
        Some(self.compose(typed))
    }
}

impl Zmk {
    fn new(rpc: Box<dyn Rpc>) -> Self {
        Self {
            rpc,
            behaviors: Vec::new(),
            layers: Vec::new(),
            custom: Vec::new(),
            compressed: Vec::new(),
            layouts: Vec::new(),
            layout: 0,
            reading: Reading::Lock,
        }
    }

    fn answer(&mut self, ask: &Ask) -> Result<Reply> {
        Ok(match *ask {
            Ask::KeyboardId => {
                // `busy` read the keyboard before this.
                // 4: dynamic-entry counts carry the Caps Word and repeat key flags, and there is no
                // persistent default layer.
                Reply::Id {
                    version: 4,
                    uid: [0; 8],
                }
            }
            Ask::Entries => Reply::Entries {
                tap_dance: 0,
                combo: 0,
                alt_repeat: u8::from(self.behavior("Key Repeat").is_some()),
                flags: u8::from(self.behavior("Caps Word").is_some()),
            },
            Ask::DefinitionSize => Reply::DefSize(
                u32::try_from(self.compressed.len())
                    .map_err(|_| Error::Protocol("layout is too large"))?,
            ),
            Ask::Definition { block } => {
                let start = (block as usize * 32).min(self.compressed.len());
                let end = (start + 32).min(self.compressed.len());
                Reply::DefBlock(self.compressed[start..end].to_vec())
            }
            Ask::LayoutOptions => Reply::LayoutOptions(0),
            Ask::LayerCount => Reply::LayerCount(
                u8::try_from(self.layers.len()).map_err(|_| Error::Protocol("layer count"))?,
            ),
            Ask::MacroCount => Reply::MacroCount(0),
            Ask::KeymapChunk { offset, len } => {
                let codes: Vec<u16> = self.keymap().concat();
                let bytes = keymap_bytes(&codes);
                let start = usize::from(offset).min(bytes.len());
                let end = (start + usize::from(len)).min(bytes.len());
                let mut chunk = bytes[start..end].to_vec();
                chunk.resize(usize::from(len), 0);
                Reply::Chunk(chunk)
            }
            Ask::SetKey {
                layer, col, code, ..
            } => {
                self.set(layer, col, code)?;
                Reply::Ack
            }
            Ask::UnlockStatus => Reply::Unlock {
                unlocked: self.unlocked()?,
                pairs: Vec::new(),
            },
            Ask::UnlockStart => Reply::Ack,
            Ask::UnlockPoll => Reply::Polled {
                unlocked: self.unlocked()?,
                left: 0,
            },
            _ => return Err(Error::Protocol("ZMK Studio edits only the keymap")),
        })
    }

    fn unlocked(&mut self) -> Result<bool> {
        // core get_lock_state; 1 is unlocked.
        let core = self.rpc.call(CORE, &[0x10, 0x01])?;
        Ok(int(&core, 2)? == Some(1))
    }

    /// The whole first read at once. Tests use it; `Load` reads piece by piece through `busy`.
    #[cfg(test)]
    fn read(&mut self) -> Result<()> {
        while self.busy()?.is_some() {}
        Ok(())
    }

    /// One piece of the first read: lock state, the behavior list, each behavior, the keymap,
    /// then the layout and the definition drawn from it. Says what the next piece reads, or
    /// `None` once the keyboard can answer.
    fn read_next(&mut self) -> Result<Option<String>> {
        let next = match std::mem::replace(&mut self.reading, Reading::Done) {
            Reading::Lock => {
                if !self.unlocked()? {
                    return Err(Error::Locked);
                }
                Reading::Behaviors
            }
            Reading::Behaviors => {
                let listed = self.rpc.call(BEHAVIORS, &[0x08, 0x01])?;
                self.behaviors.clear();
                Reading::Details(ints(field(&listed, 1)?.unwrap_or_default(), 1)?)
            }
            Reading::Details(ids) if self.behaviors.len() < ids.len() => {
                let id = ids[self.behaviors.len()];
                let mut request = Vec::new();
                put_int(&mut request, 1, id);
                let mut details = Vec::new();
                put_bytes(&mut details, 2, &request);
                let reply = self.rpc.call(BEHAVIORS, &details)?;
                let details = field(&reply, 2)?.ok_or(Error::Protocol("behavior details"))?;
                self.behaviors.push(behavior(id as i32, details)?);
                Reading::Details(ids)
            }
            Reading::Details(_) => Reading::Keymap,
            Reading::Keymap => {
                self.read_keymap()?;
                Reading::Layout
            }
            Reading::Layout => {
                self.read_layout()?;
                return Ok(None);
            }
            Reading::Done => return Ok(None),
        };
        let what = match &next {
            Reading::Details(ids) => format!("behaviors {}/{}", self.behaviors.len(), ids.len()),
            Reading::Keymap => "keymap".to_string(),
            Reading::Layout => "layout".to_string(),
            _ => "behaviors".to_string(),
        };
        self.reading = next;
        Ok(Some(what))
    }

    fn read_keymap(&mut self) -> Result<()> {
        let reply = self.rpc.call(KEYMAP, &[0x08, 0x01])?;
        let keymap = field(&reply, 1)?.ok_or(Error::Protocol("keymap"))?;
        self.layers.clear();
        for layer in all(keymap, 1)? {
            let mut bindings = Vec::new();
            for binding in all(layer, 3)? {
                bindings.push(Binding {
                    behavior: unzigzag(int(binding, 1)?.unwrap_or(0)),
                    param1: int(binding, 2)?.unwrap_or(0) as u32,
                    param2: int(binding, 3)?.unwrap_or(0) as u32,
                });
            }
            self.layers.push(Layer {
                id: int(layer, 1)?.unwrap_or(0) as u32,
                name: text(layer, 2).unwrap_or_default(),
                bindings,
            });
        }
        let keys = self
            .layers
            .iter()
            .map(|l| l.bindings.len())
            .max()
            .unwrap_or(0);
        if keys == 0 || keys > usize::from(u8::MAX) {
            return Err(Error::NoLayout);
        }
        // ZMK layers all hold one binding per key position. The Vial matrix is one row of them.
        if self.layers.iter().any(|l| l.bindings.len() != keys) {
            return Err(Error::Protocol("ZMK layers differ in length"));
        }
        Ok(())
    }

    fn read_layout(&mut self) -> Result<()> {
        let keys = self.layers[0].bindings.len();
        let reply = self.rpc.call(KEYMAP, &[0x30, 0x01])?;
        let layouts = field(&reply, 6)?.ok_or(Error::NoLayout)?;
        let active = int(layouts, 1)?.unwrap_or(0) as usize;
        let found = all(layouts, 2)?;
        self.layouts = found
            .iter()
            .enumerate()
            .map(|(index, layout)| {
                text(layout, 1)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("layout {index}"))
            })
            .collect();
        if found.is_empty() {
            return Err(Error::NoLayout);
        }
        self.layout = active.min(self.layouts.len() - 1);
        let layout = found[self.layout];
        let mut rows = Vec::new();
        for (position, key) in all(layout, 2)?.into_iter().enumerate().take(keys) {
            let at = |tag| -> Result<f64> {
                Ok(f64::from(unzigzag(int(key, tag)?.unwrap_or(0))) / 100.0)
            };
            let (rx, ry) = (at(6)?, at(7)?);
            rows.push(serde_json::json!([
                {
                    "r": at(5)?, "rx": rx, "ry": ry,
                    "x": at(3)? - rx, "y": at(4)? - ry,
                    "w": at(1)?, "h": at(2)?,
                },
                format!("0,{position}"),
            ]));
        }

        self.custom.clear();
        let mut extras = self.extras();
        let mut seen: Vec<Binding> = self
            .layers
            .iter()
            .flat_map(|l| l.bindings.clone())
            .collect();
        seen.append(&mut extras);
        for binding in seen {
            if self.code(binding).is_none() {
                if self.custom.len() == CUSTOM_ROOM {
                    return Err(Error::Protocol("too many ZMK bindings without a keycode"));
                }
                self.custom.push(binding);
            }
        }
        let custom: Vec<_> = self
            .custom
            .iter()
            .map(|binding| serde_json::json!({ "name": self.name(*binding) }))
            .collect();
        let json = serde_json::json!({
            "matrix": { "rows": 1, "cols": keys },
            "layouts": { "keymap": rows },
            "customKeycodes": custom,
        });
        self.compressed = compress(json.to_string().as_bytes())?;
        Ok(())
    }

    /// Sends one binding and saves it.
    fn set(&mut self, layer: u8, position: u8, code: u16) -> Result<()> {
        let binding = self.binding(code).ok_or(Error::Protocol(
            "this keyboard has no ZMK binding for that key",
        ))?;
        let id = self
            .layers
            .get(usize::from(layer))
            .ok_or(Error::Protocol("layer"))?
            .id;
        let mut encoded = Vec::new();
        let behavior = (binding.behavior << 1) ^ (binding.behavior >> 31);
        put_int(&mut encoded, 1, u64::from(behavior as u32));
        put_int(&mut encoded, 2, u64::from(binding.param1));
        put_int(&mut encoded, 3, u64::from(binding.param2));
        let mut request = Vec::new();
        put_int(&mut request, 1, u64::from(id));
        put_int(&mut request, 2, u64::from(position));
        put_bytes(&mut request, 3, &encoded);
        let mut set = Vec::new();
        put_bytes(&mut set, 2, &request);
        let reply = self.rpc.call(KEYMAP, &set)?;
        if int(&reply, 2)? != Some(0) {
            return Err(Error::Protocol("the keyboard refused that binding"));
        }
        self.save()?;
        let slot = self.layers[usize::from(layer)]
            .bindings
            .get_mut(usize::from(position))
            .ok_or(Error::Protocol("key position"))?;
        *slot = binding;
        Ok(())
    }

    /// `keymap.save_changes`: what Studio staged becomes what the keyboard keeps.
    fn save(&mut self) -> Result<()> {
        let reply = self.rpc.call(KEYMAP, &[0x20, 0x01])?;
        let saved = field(&reply, 4)?.ok_or(Error::Protocol("save"))?;
        if int(saved, 1)? != Some(1) {
            return Err(Error::Protocol("the keyboard could not save the keymap"));
        }
        Ok(())
    }

    /// Studio's layer calls, then a save. The next load reads everything again.
    fn layer_call(&mut self, op: &LayerOp) -> Result<()> {
        let layer = |index: u8| self.layers.get(usize::from(index)).map(|l| l.id);
        let (tag, body) = match op {
            LayerOp::Add => (9, Vec::new()),
            LayerOp::Remove(index) => {
                layer(*index).ok_or(Error::Protocol("layer"))?;
                let mut body = Vec::new();
                put_int(&mut body, 1, u64::from(*index));
                (10, body)
            }
            LayerOp::Move { from, to } => {
                let mut body = Vec::new();
                put_int(&mut body, 1, u64::from(*from));
                put_int(&mut body, 2, u64::from(*to));
                (8, body)
            }
            LayerOp::Rename(index, name) => {
                let id = layer(*index).ok_or(Error::Protocol("layer"))?;
                let mut body = Vec::new();
                put_int(&mut body, 1, u64::from(id));
                put_bytes(&mut body, 2, name.as_bytes());
                (12, body)
            }
        };
        let mut request = Vec::new();
        put_bytes(&mut request, tag, &body);
        let reply = self.rpc.call(KEYMAP, &request)?;
        // Rename answers with a bare result code; the rest with `ok` (field 1) or `err`.
        let done = if tag == 12 {
            int(&reply, 12)? == Some(0)
        } else {
            match field(&reply, tag)? {
                Some(result) => field(result, 1)?.is_some(),
                None => false,
            }
        };
        if !done {
            return Err(Error::Protocol(match op {
                LayerOp::Add => "the keyboard has no room for another layer",
                LayerOp::Remove(_) => "the keyboard would not remove that layer",
                LayerOp::Move { .. } => "the keyboard would not move that layer",
                LayerOp::Rename(..) => "the keyboard would not take that name",
            }));
        }
        self.save()
    }

    /// `keymap.set_active_physical_layout`, then a save. The next load reads the new grid.
    fn layout_call(&mut self, index: usize) -> Result<()> {
        let mut request = Vec::new();
        put_int(&mut request, 7, index as u64);
        let reply = self.rpc.call(KEYMAP, &request)?;
        let body = field(&reply, 7)?.ok_or(Error::Protocol("layout"))?;
        if int(body, 2)?.is_some() {
            return Err(Error::Protocol("the keyboard would not switch layout"));
        }
        self.layout = index;
        self.save()
    }

    fn behavior(&self, name: &str) -> Option<i32> {
        self.behaviors.iter().find(|b| b.name == name).map(|b| b.id)
    }

    fn code(&self, binding: Binding) -> Option<u16> {
        if let Some(index) = self.custom.iter().position(|b| *b == binding) {
            return Some(CUSTOM + index as u16);
        }
        // Only a keycode that turns back into this exact binding stands for it.
        self.qmk(binding)
            .filter(|code| self.native(*code) == Some(binding))
    }

    fn binding(&self, code: u16) -> Option<Binding> {
        if (CUSTOM..CUSTOM + CUSTOM_ROOM as u16).contains(&code) {
            return self.custom.get(usize::from(code - CUSTOM)).copied();
        }
        self.native(code)
    }

    /// The QMK keycode for a binding of a behavior QMK has.
    fn qmk(&self, binding: Binding) -> Option<u16> {
        let name = &self
            .behaviors
            .iter()
            .find(|b| b.id == binding.behavior)?
            .name;
        qmk_code(name, binding.param1, binding.param2, |id| {
            self.layers
                .iter()
                .position(|l| l.id == id)
                .map(|i| i as u16)
        })
    }

    /// The binding for a QMK keycode, when the keyboard has the behavior it needs.
    fn native(&self, code: u16) -> Option<Binding> {
        let (name, param1, param2) = qmk_binding(code, |index| {
            self.layers.get(usize::from(index)).map(|l| l.id)
        })?;
        self.behavior(name).map(|behavior| Binding {
            behavior,
            param1,
            param2,
        })
    }

    /// Every binding of a behavior QMK has no keycode for whose parameters are all named
    /// values or small ranges: Bluetooth profiles, outputs, lighting, Studio unlock. The key
    /// list offers these. A behavior with no parameter sets takes none.
    fn extras(&self) -> Vec<Binding> {
        let none = [[Vec::new(), Vec::new()]];
        let mut out = Vec::new();
        for behavior in &self.behaviors {
            let start = out.len();
            let sets = if behavior.sets.is_empty() {
                &none[..]
            } else {
                &behavior.sets[..]
            };
            for set in sets {
                let [Some(firsts), Some(seconds)] = [values(&set[0]), values(&set[1])] else {
                    continue;
                };
                for &param1 in &firsts {
                    for &param2 in &seconds {
                        let binding = Binding {
                            behavior: behavior.id,
                            param1,
                            param2,
                        };
                        if self.qmk(binding).is_none() && !out[start..].contains(&binding) {
                            out.push(binding);
                        }
                    }
                }
            }
            out.truncate(start + (out.len() - start).min(EXTRAS_PER_BEHAVIOR));
        }
        out
    }

    /// `Bluetooth Select Profile 1`: the behavior, then each parameter by its metadata name.
    fn name(&self, binding: Binding) -> String {
        let Some(behavior) = self.behaviors.iter().find(|b| b.id == binding.behavior) else {
            return format!("behavior {}", binding.behavior);
        };
        let mut words = vec![behavior.name.clone()];
        if behavior.sets.is_empty() && behavior.name.starts_with("mouse_") && binding.param1 != 0 {
            words.push(move_text(binding.param1));
            return words.join(" ");
        }
        for (slot, param) in [binding.param1, binding.param2].into_iter().enumerate() {
            let described = behavior.sets.iter().flat_map(|set| &set[slot]);
            let mut word = None;
            for desc in described {
                word = match desc {
                    Param::Constant(value, name) if *value == param => Some(name.clone()),
                    Param::Layer => self
                        .layers
                        .iter()
                        .position(|l| l.id == param)
                        .map(|index| index.to_string()),
                    Param::Usage => Some(usage_name(param)),
                    Param::Range(..) => Some(param.to_string()),
                    _ => continue,
                };
                if word.is_some() {
                    break;
                }
            }
            match word {
                Some(word) => words.push(word),
                None if param != 0 => words.push(param.to_string()),
                None => {}
            }
        }
        words.join(" ")
    }

    /// `&kp`, then each parameter, from the behavior list the keyboard sent.
    fn compose(&self, typed: &str) -> super::file::Completion {
        let words: Vec<&str> = typed.split_whitespace().collect();
        let (done, partial) = if typed.ends_with(' ') || words.is_empty() {
            (&words[..], "")
        } else {
            (&words[..words.len() - 1], words[words.len() - 1])
        };
        let tokens = self.behavior_tokens();
        let Some(head) = done.first() else {
            let items = tokens
                .iter()
                .map(|(id, token)| {
                    let name = self
                        .behaviors
                        .iter()
                        .find(|b| b.id == *id)
                        .map(|b| b.name.clone());
                    (format!("&{token}"), name.unwrap_or_default())
                })
                .collect();
            return (
                "behavior".to_string(),
                rank(items, &format!("&{}", partial.trim_start_matches('&'))),
                false,
            );
        };
        let label = head.trim_start_matches('&');
        let Some(behavior) = tokens
            .iter()
            .find(|(_, token)| token == label)
            .and_then(|(id, _)| self.behaviors.iter().find(|b| b.id == *id))
        else {
            return (format!("no {label} behavior"), Vec::new(), false);
        };
        if behavior.sets.is_empty() {
            // ZMK sends no parameter list for `&mmv`, `&msc` or `&ext_power`: take keymap text.
            let title = format!("&{label} ({}): optional parameters", behavior.name);
            if done.len() > 2 || done[1..].iter().any(|word| bare_param(word).is_none()) {
                return (format!("not a {label} parameter"), Vec::new(), false);
            }
            if partial.is_empty() {
                return (title, Vec::new(), true);
            }
            let named = BARE
                .iter()
                .map(|(name, _)| (name.to_string(), String::new()));
            let mut items = rank(named.collect(), partial);
            if bare_param(partial).is_some() && !items.iter().any(|(t, _)| t == partial) {
                items.insert(0, (partial.to_string(), String::new()));
            }
            return (title, items, false);
        }
        let mut sets: Vec<&[Vec<Param>; 2]> = behavior.sets.iter().collect();
        let mut filled = Vec::new();
        for word in &done[1..] {
            let Some(slot) = next_slot(&sets, &filled) else {
                return (behavior.name.clone(), Vec::new(), false);
            };
            let Some(value) = match_word(&sets, label, slot, word, &self.layers) else {
                return (format!("not a {label} parameter"), Vec::new(), false);
            };
            sets.retain(|set| accepts(&set[slot], value));
            filled.push(slot);
        }
        let Some(slot) = next_slot(&sets, &filled) else {
            return (format!("&{label}"), Vec::new(), partial.is_empty());
        };
        let (what, items) = slot_options(&sets, label, slot, &self.layers);
        let mut items = rank(items, partial);
        if what == "key" {
            if let Some(text) = modifier_expr(partial) {
                if !items.iter().any(|(token, _)| token == &text) {
                    items.insert(0, (text, String::new()));
                }
            }
        }
        if let Some(value) = range_value(&sets, slot, partial) {
            if !items.iter().any(|(token, _)| token == partial) {
                items.insert(0, (partial.to_string(), value.to_string()));
            }
        }
        let count = param_count(behavior);
        let tap = count == 2
            && behavior
                .sets
                .iter()
                .all(|set| set[1].iter().any(|p| matches!(p, Param::Usage)));
        let title = super::file::role(what, filled.len(), count, tap);
        (title, items, false)
    }

    fn parse_binding(&self, text: &str) -> Option<Binding> {
        let words: Vec<&str> = text.split_whitespace().collect();
        let (head, params) = words.split_first()?;
        let label = head.strip_prefix('&')?;
        let id = self
            .behavior_tokens()
            .into_iter()
            .find(|(_, token)| token == label)
            .map(|(id, _)| id)?;
        let behavior = self.behaviors.iter().find(|b| b.id == id)?;
        if behavior.sets.is_empty() {
            let mut chosen = [0u32; 2];
            for (slot, word) in params.iter().enumerate() {
                *chosen.get_mut(slot)? = bare_param(word)?;
            }
            return Some(Binding {
                behavior: id,
                param1: chosen[0],
                param2: chosen[1],
            });
        }
        let mut sets: Vec<&[Vec<Param>; 2]> = behavior.sets.iter().collect();
        let mut chosen = [0u32; 2];
        let mut filled = Vec::new();
        for word in params {
            let slot = next_slot(&sets, &filled)?;
            let value = match_word(&sets, label, slot, word, &self.layers)?;
            sets.retain(|set| accepts(&set[slot], value));
            if behavior.sets.is_empty() || sets.is_empty() {
                return None;
            }
            chosen[slot] = value;
            filled.push(slot);
        }
        if next_slot(&sets, &filled).is_some() {
            return None;
        }
        Some(Binding {
            behavior: id,
            param1: chosen[0],
            param2: chosen[1],
        })
    }

    /// A binding as a keymap writes it, in the tokens `parse_binding` reads back:
    /// `&mt LSHIFT A`, `&bt BT_SEL 1`, `&mo 2`, `&mmv MOVE_X(-2500)`.
    fn text(&self, binding: Binding) -> Option<String> {
        let (_, label) = self
            .behavior_tokens()
            .into_iter()
            .find(|(id, _)| *id == binding.behavior)?;
        let behavior = self.behaviors.iter().find(|b| b.id == binding.behavior)?;
        let mut words = vec![format!("&{label}")];
        if behavior.sets.is_empty() {
            // Trailing zeros are left out; a zero before a set parameter keeps its place.
            let count = if binding.param2 != 0 {
                2
            } else {
                usize::from(binding.param1 != 0)
            };
            for &param in &[binding.param1, binding.param2][..count] {
                let moved = move_text(param);
                let mouse = behavior.name.starts_with("mouse_") && param != 0;
                words.push(if mouse && bare_param(&moved) == Some(param) {
                    moved
                } else {
                    param.to_string()
                });
            }
            return Some(words.join(" "));
        }
        let mut sets: Vec<&[Vec<Param>; 2]> = behavior.sets.iter().collect();
        let mut filled = Vec::new();
        while let Some(slot) = next_slot(&sets, &filled) {
            let value = [binding.param1, binding.param2][slot];
            let (what, items) = slot_options(&sets, &label, slot, &self.layers);
            // A key by the name the key list shows, `LSHIFT`; modifier functions by the
            // shortest form; anything else by the token it is typed as.
            let key = (what == "key")
                .then(|| super::zmk_keys::preferred(value).or_else(|| super::zmk_keys::name(value)))
                .flatten();
            let word = key
                .or_else(|| {
                    items
                        .iter()
                        .find(|(token, _)| {
                            option_value(&sets, &label, slot, token, &self.layers) == Some(value)
                        })
                        .map(|(token, _)| token.clone())
                })
                .or_else(|| super::zmk_keys::name(value))
                .unwrap_or_else(|| value.to_string());
            words.push(word);
            sets.retain(|set| accepts(&set[slot], value));
            filled.push(slot);
        }
        Some(words.join(" "))
    }

    fn behavior_tokens(&self) -> Vec<(i32, String)> {
        let mut used = Vec::new();
        self.behaviors
            .iter()
            .map(|behavior| {
                let mut token = preferred(&behavior.name);
                if used.iter().any(|taken| taken == &token) {
                    token = format!("{token}{}", behavior.id);
                }
                used.push(token.clone());
                (behavior.id, token)
            })
            .collect()
    }
}

/// ZMK's names for parameters of behaviors that send no parameter list (`pointing.h` at its
/// default speeds, `ext_power.h`).
const BARE: &[(&str, u32)] = &[
    ("MOVE_UP", (-600i32 as u32) & 0xFFFF),
    ("MOVE_DOWN", 600),
    ("MOVE_LEFT", ((-600i32 as u32) & 0xFFFF) << 16),
    ("MOVE_RIGHT", 600 << 16),
    ("SCRL_UP", 10),
    ("SCRL_DOWN", (-10i32 as u32) & 0xFFFF),
    ("SCRL_LEFT", ((-10i32 as u32) & 0xFFFF) << 16),
    ("SCRL_RIGHT", 10 << 16),
    ("EP_OFF", 0),
    ("EP_ON", 1),
    ("EP_TOG", 2),
];

/// `&mmv`/`&msc`'s parameter as a keymap writes it: `MOVE_X(-2500)`, `MOVE_Y(20)`, `MOVE(x,y)`.
fn move_text(param: u32) -> String {
    let (x, y) = ((param >> 16) as u16 as i16, param as u16 as i16);
    match (x, y) {
        (0, y) => format!("MOVE_Y({y})"),
        (x, 0) => format!("MOVE_X({x})"),
        (x, y) => format!("MOVE({x},{y})"),
    }
}

/// A parameter as a keymap writes it: a number, `MOVE_X(n)`, `MOVE_Y(n)`, or a `BARE` name.
fn bare_param(word: &str) -> Option<u32> {
    if let Ok(n) = word.parse::<i64>() {
        return (i64::from(i32::MIN)..=i64::from(u32::MAX))
            .contains(&n)
            .then_some(n as u32);
    }
    let axis = |name: &str| -> Option<u32> {
        let n = word
            .strip_prefix(name)?
            .strip_prefix('(')?
            .strip_suffix(')')?;
        Some(n.trim().parse::<i32>().ok()? as u32 & 0xFFFF)
    };
    if let Some(x) = axis("MOVE_X") {
        return Some(x << 16);
    }
    if let Some(y) = axis("MOVE_Y") {
        return Some(y);
    }
    BARE.iter().find(|(name, _)| *name == word).map(|(_, v)| *v)
}

/// Display names Studio sends, and the `&` token the key list uses. The rest are slugged.
const LABELS: &[(&str, &str)] = &[
    ("Key Press", "kp"),
    ("Mod-Tap", "mt"),
    ("Layer-Tap", "lt"),
    ("Momentary Layer", "mo"),
    ("Toggle Layer", "tog"),
    ("To Layer", "to"),
    ("Sticky Layer", "sl"),
    ("Sticky Key", "sk"),
    ("Key Toggle", "kt"),
    ("Mouse Key Press", "mkp"),
    ("Bluetooth", "bt"),
    ("Output Selection", "out"),
    ("Underglow", "rgb_ug"),
    ("External Power", "ext_power"),
    ("Backlight", "bl"),
    ("Transparent", "trans"),
    ("None", "none"),
    ("Bootloader", "bootloader"),
    ("Reset", "sys_reset"),
    ("Grave/Escape", "gresc"),
    ("Caps Word", "caps_word"),
    ("Key Repeat", "key_repeat"),
    ("Studio Unlock", "studio_unlock"),
];

/// The names ZMK's dt-bindings headers give a behavior's first parameter, by value
/// (`bt.h`, `outputs.h`, `rgb.h`, `backlight.h`, `ext_power.h`, `pointing.h`), so a binding
/// composed on a Studio board reads as it would in a keymap: `&bt BT_SEL 1`.
const COMMANDS: &[(&str, &[(&str, u32)])] = &[
    (
        "bt",
        &[
            ("BT_CLR", 0),
            ("BT_NXT", 1),
            ("BT_PRV", 2),
            ("BT_SEL", 3),
            ("BT_CLR_ALL", 4),
            ("BT_DISC", 5),
        ],
    ),
    (
        "out",
        &[
            ("OUT_TOG", 0),
            ("OUT_USB", 1),
            ("OUT_BLE", 2),
            ("OUT_NONE", 3),
        ],
    ),
    (
        "rgb_ug",
        &[
            ("RGB_TOG", 0),
            ("RGB_ON", 1),
            ("RGB_OFF", 2),
            ("RGB_HUI", 3),
            ("RGB_HUD", 4),
            ("RGB_SAI", 5),
            ("RGB_SAD", 6),
            ("RGB_BRI", 7),
            ("RGB_BRD", 8),
            ("RGB_SPI", 9),
            ("RGB_SPD", 10),
            ("RGB_EFF", 11),
            ("RGB_EFR", 12),
        ],
    ),
    (
        "bl",
        &[
            ("BL_ON", 0),
            ("BL_OFF", 1),
            ("BL_TOG", 2),
            ("BL_INC", 3),
            ("BL_DEC", 4),
            ("BL_CYCLE", 5),
            ("BL_SET", 6),
        ],
    ),
    ("ext_power", &[("EP_OFF", 0), ("EP_ON", 1), ("EP_TOG", 2)]),
    (
        "mkp",
        &[
            ("LCLK", 1),
            ("RCLK", 2),
            ("MCLK", 4),
            ("MB4", 8),
            ("MB5", 16),
        ],
    ),
];

/// The token a constant parameter is typed as: ZMK's name for it when the behavior's first
/// parameter has one, else its Studio display name without spaces.
fn constant_token(label: &str, slot: usize, value: u32, name: &str) -> String {
    let known = (slot == 0)
        .then(|| COMMANDS.iter().find(|(l, _)| *l == label))
        .flatten()
        .and_then(|(_, names)| names.iter().find(|(_, v)| *v == value));
    match known {
        Some((zmk, _)) => (*zmk).to_string(),
        None if slug(name).is_empty() => value.to_string(),
        None => slug(name),
    }
}

fn preferred(name: &str) -> String {
    LABELS
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, token)| (*token).to_string())
        .unwrap_or_else(|| slug(name))
}

fn slug(name: &str) -> String {
    let token: String = name
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .collect();
    if token.is_empty() {
        "binding".to_string()
    } else {
        token
    }
}

fn rank(items: Vec<(String, String)>, partial: &str) -> Vec<(String, String)> {
    let needle = partial.to_lowercase();
    let mut hits: Vec<(u8, (String, String))> = items
        .into_iter()
        .filter_map(|(token, about)| {
            let hay = token.to_lowercase();
            let rank = if hay == needle {
                0
            } else if hay.starts_with(&needle) {
                1
            } else if hay.contains(&needle) || about.to_lowercase().contains(&needle) {
                2
            } else {
                return None;
            };
            Some((rank, (token, about)))
        })
        .collect();
    hits.sort_by_key(|(rank, _)| *rank);
    hits.into_iter().map(|(_, hit)| hit).collect()
}

fn modifier_expr(partial: &str) -> Option<String> {
    let text = partial.trim();
    (text.contains('(') && super::zmk_keys::parse(text).is_some()).then(|| text.to_string())
}

fn param_count(behavior: &Behavior) -> usize {
    (0..2)
        .filter(|slot| behavior.sets.iter().any(|set| slot_needed(set, *slot)))
        .count()
}

fn slot_needed(set: &[Vec<Param>; 2], slot: usize) -> bool {
    set[slot].iter().any(|param| !matches!(param, Param::Nil))
}

fn next_slot(sets: &[&[Vec<Param>; 2]], filled: &[usize]) -> Option<usize> {
    (0..2).find(|slot| !filled.contains(slot) && sets.iter().any(|set| slot_needed(set, *slot)))
}

fn accepts(descs: &[Param], value: u32) -> bool {
    if descs.is_empty() || descs.iter().all(|param| matches!(param, Param::Nil)) {
        return value == 0;
    }
    descs.iter().any(|param| match param {
        Param::Constant(known, _) => *known == value,
        Param::Range(min, max) => in_range(value, *min, *max),
        Param::Usage => super::zmk_keys::name(value).is_some(),
        Param::Layer => true,
        Param::Nil => false,
    })
}

fn in_range(value: u32, min: i32, max: i32) -> bool {
    let value = i64::from(value);
    value >= i64::from(min) && value <= i64::from(max)
}

fn match_word(
    sets: &[&[Vec<Param>; 2]],
    label: &str,
    slot: usize,
    word: &str,
    layers: &[Layer],
) -> Option<u32> {
    let (what, items) = slot_options(sets, label, slot, layers);
    if let Some((token, _)) = items.iter().find(|(token, _)| token == word) {
        return option_value(sets, label, slot, token, layers);
    }
    if what == "key" {
        if let Some(value) = super::zmk_keys::parse(word) {
            return Some(value);
        }
    }
    range_value(sets, slot, word)
}

fn option_value(
    sets: &[&[Vec<Param>; 2]],
    label: &str,
    slot: usize,
    token: &str,
    layers: &[Layer],
) -> Option<u32> {
    for set in sets {
        for desc in &set[slot] {
            let value = match desc {
                Param::Constant(value, name)
                    if constant_token(label, slot, *value, name) == token
                        || value.to_string() == token =>
                {
                    Some(*value)
                }
                Param::Range(min, max)
                    if *min >= 0
                        && token
                            .parse::<i32>()
                            .ok()
                            .is_some_and(|n| n >= *min && n <= *max) =>
                {
                    token.parse().ok()
                }
                Param::Usage => super::zmk_keys::parse(token),
                Param::Layer => layers
                    .iter()
                    .enumerate()
                    .find_map(|(index, layer)| (index.to_string() == token).then_some(layer.id)),
                _ => None,
            };
            if value.is_some() {
                return value;
            }
        }
    }
    None
}

fn range_value(sets: &[&[Vec<Param>; 2]], slot: usize, word: &str) -> Option<u32> {
    let value = word.parse::<i64>().ok()?;
    sets.iter()
        .flat_map(|set| &set[slot])
        .any(|desc| matches!(desc, Param::Range(min, max) if value >= i64::from(*min) && value <= i64::from(*max)))
        .then_some(value as u32)
}

fn slot_options(
    sets: &[&[Vec<Param>; 2]],
    label: &str,
    slot: usize,
    layers: &[Layer],
) -> (&'static str, Vec<(String, String)>) {
    let mut constants = Vec::new();
    let mut ranges = Vec::new();
    let (mut usage, mut layer) = (false, false);
    for set in sets {
        for desc in &set[slot] {
            match desc {
                Param::Constant(value, name) => {
                    let token = constant_token(label, slot, *value, name);
                    if !constants.iter().any(|(known, _)| known == &token) {
                        constants.push((token, name.clone()));
                    }
                }
                Param::Range(min, max) if *min >= 0 && max - min < 32 => {
                    for n in *min..=*max {
                        let token = n.to_string();
                        if !ranges.iter().any(|(known, _)| known == &token) {
                            ranges.push((token, String::new()));
                        }
                    }
                }
                Param::Usage => usage = true,
                Param::Layer => layer = true,
                _ => {}
            }
        }
    }
    let mut items = constants;
    if layer {
        for (index, layer) in layers.iter().enumerate() {
            items.push((index.to_string(), layer.name.clone()));
        }
    }
    items.extend(ranges);
    if usage {
        items.extend(super::zmk_keys::listed().iter().cloned());
    }
    let what = if usage && !layer {
        "key"
    } else if layer && !usage {
        "layer"
    } else {
        "value"
    };
    (what, items)
}

/// The QMK keycode for a binding of the behavior whose `display-name` is `name`. `layer` turns
/// a layer parameter into the layer's index: Studio passes layer ids, a keymap file indexes.
pub(super) fn qmk_code(
    name: &str,
    p1: u32,
    p2: u32,
    layer: impl Fn(u32) -> Option<u16>,
) -> Option<u16> {
    Some(match name {
        "None" => 0x0000,
        "Transparent" => 0x0001,
        "Bootloader" => 0x7C00,
        "Reset" => 0x7C01,
        "Grave/Escape" => 0x7C16,
        "Caps Word" => 0x7C73,
        "Key Repeat" => 0x7C79,
        "Key Press" => {
            let mods = p1 >> 24;
            let (page, id) = ((p1 >> 16) & 0xFF, p1 & 0xFFFF);
            if mods == 0 {
                match page {
                    0x07 => basic(id)?,
                    _ => MEDIA.iter().find(|m| (m.1, m.2) == (page, id))?.0,
                }
            } else if page == 0x07 {
                u16::from(mod5(mods as u8)?) << 8 | basic(id)?
            } else {
                return None;
            }
        }
        "Mouse Key Press" if p1.is_power_of_two() && p1 <= 0x10 => {
            0x00D1 + p1.trailing_zeros() as u16
        }
        "Mod-Tap" => 0x2000 | u16::from(mod5(modifiers(p1)?)?) << 8 | basic_usage(p2)?,
        "Layer-Tap" => 0x4000 | layer(p1).filter(|l| *l < 16)? << 8 | basic_usage(p2)?,
        "To Layer" => 0x5200 | layer(p1).filter(|l| *l < 32)?,
        "Momentary Layer" => 0x5220 | layer(p1).filter(|l| *l < 32)?,
        "Toggle Layer" => 0x5260 | layer(p1).filter(|l| *l < 32)?,
        "Sticky Layer" => 0x5280 | layer(p1).filter(|l| *l < 32)?,
        "Sticky Key" => 0x52A0 | u16::from(mod5(modifiers(p1)?)?),
        _ => return None,
    })
}

/// The behavior (by `display-name`) and parameters a QMK keycode stands for. `layer` turns a
/// layer index into the parameter the keyboard takes.
pub(super) fn qmk_binding(
    code: u16,
    layer: impl Fn(u16) -> Option<u32>,
) -> Option<(&'static str, u32, u32)> {
    let low = code & 0xFF;
    Some(match code {
        0x0000 => ("None", 0, 0),
        0x0001 => ("Transparent", 0, 0),
        0x7C00 => ("Bootloader", 0, 0),
        0x7C01 => ("Reset", 0, 0),
        0x7C16 => ("Grave/Escape", 0, 0),
        0x7C73 => ("Caps Word", 0, 0),
        0x7C79 => ("Key Repeat", 0, 0),
        0x0004..=0x00A4 | 0x00E0..=0x00E7 => ("Key Press", usage(code), 0),
        0x00D1..=0x00D5 => ("Mouse Key Press", 1 << (code - 0x00D1), 0),
        0x0100..=0x1FFF if basic(u32::from(low)).is_some() => {
            let mods = mods8((code >> 8) as u8 & 0x1F);
            if mods == 0 {
                return None;
            }
            ("Key Press", u32::from(mods) << 24 | usage(low), 0)
        }
        0x2000..=0x3FFF if basic(u32::from(low)).is_some() => (
            "Mod-Tap",
            modifier_key((code >> 8) as u8 & 0x1F)?,
            usage(low),
        ),
        0x4000..=0x4FFF if basic(u32::from(low)).is_some() => {
            ("Layer-Tap", layer(code >> 8 & 0x0F)?, usage(low))
        }
        0x5200..=0x521F => ("To Layer", layer(code & 0x1F)?, 0),
        0x5220..=0x523F => ("Momentary Layer", layer(code & 0x1F)?, 0),
        0x5260..=0x527F => ("Toggle Layer", layer(code & 0x1F)?, 0),
        0x5280..=0x529F => ("Sticky Layer", layer(code & 0x1F)?, 0),
        0x52A0..=0x52BF => ("Sticky Key", modifier_key(code as u8 & 0x1F)?, 0),
        _ => {
            let (_, page, id) = MEDIA.iter().find(|m| m.0 == code)?;
            ("Key Press", page << 16 | id, 0)
        }
    })
}

/// QMK system and consumer keycodes and the HID page and usage they send (QMK `report.h`).
const MEDIA: &[(u16, u32, u32)] = &[
    (0x00A5, 0x01, 0x81),
    (0x00A6, 0x01, 0x82),
    (0x00A7, 0x01, 0x83),
    (0x00A8, 0x0C, 0xE2),
    (0x00A9, 0x0C, 0xE9),
    (0x00AA, 0x0C, 0xEA),
    (0x00AB, 0x0C, 0xB5),
    (0x00AC, 0x0C, 0xB6),
    (0x00AD, 0x0C, 0xB7),
    (0x00AE, 0x0C, 0xCD),
    (0x00AF, 0x0C, 0x183),
    (0x00B0, 0x0C, 0xB8),
    (0x00B1, 0x0C, 0x18A),
    (0x00B2, 0x0C, 0x192),
    (0x00B3, 0x0C, 0x194),
    (0x00B4, 0x0C, 0x221),
    (0x00B5, 0x0C, 0x223),
    (0x00B6, 0x0C, 0x224),
    (0x00B7, 0x0C, 0x225),
    (0x00B8, 0x0C, 0x226),
    (0x00B9, 0x0C, 0x227),
    (0x00BA, 0x0C, 0x22A),
    (0x00BB, 0x0C, 0xB3),
    (0x00BC, 0x0C, 0xB4),
    (0x00BD, 0x0C, 0x6F),
    (0x00BE, 0x0C, 0x70),
    (0x00BF, 0x0C, 0x19F),
    (0x00C0, 0x0C, 0x1CB),
];

/// A QMK basic keycode is its HID keyboard usage.
fn basic(id: u32) -> Option<u16> {
    matches!(id, 0x04..=0xA4 | 0xE0..=0xE7).then_some(id as u16)
}

/// A keyboard-page usage with no modifiers in it.
fn basic_usage(param: u32) -> Option<u16> {
    if param >> 16 != 0x07 {
        return None;
    }
    basic(param & 0xFFFF)
}

fn usage(code: u16) -> u32 {
    0x07 << 16 | u32::from(code)
}

/// QMK's five mod bits (Ctrl, Shift, Alt, GUI, then right-hand) as HID's eight.
fn mods8(mod5: u8) -> u8 {
    if mod5 & 0x10 == 0 {
        mod5 & 0x0F
    } else {
        (mod5 & 0x0F) << 4
    }
}

/// HID's eight mod bits as QMK's five, when they are all on one hand.
fn mod5(mods: u8) -> Option<u8> {
    match (mods & 0x0F, mods >> 4) {
        (0, 0) => None,
        (left, 0) => Some(left),
        (0, right) => Some(right | 0x10),
        _ => None,
    }
}

/// `&mt LSHIFT` and `&sk LC(LSHIFT)` take a modifier key, with more modifiers in its top byte.
fn modifiers(param: u32) -> Option<u8> {
    let id = param & 0xFFFF;
    if (param >> 16) & 0xFF != 0x07 || !(0xE0..=0xE7).contains(&id) {
        return None;
    }
    Some(1 << (id - 0xE0) | (param >> 24) as u8)
}

/// The inverse of `modifiers`: the lowest modifier as the key, the rest on top.
fn modifier_key(mod5: u8) -> Option<u32> {
    let mods = mods8(mod5);
    if mods == 0 {
        return None;
    }
    let low = mods.trailing_zeros();
    Some(u32::from(mods & !(1 << low)) << 24 | usage(0xE0 + low as u16))
}

/// The key's legend for a plain keyboard usage, else the number.
fn usage_name(param: u32) -> String {
    basic_usage(param)
        .and_then(|code| {
            [keycodes::BASIC, keycodes::ISO_JIS]
                .iter()
                .flat_map(|table| table.iter())
                .find(|row| row.2 == code)
        })
        .map_or_else(|| format!("{param:#x}"), |row| row.1.to_string())
}

/// Every value a parameter may take, when they are few and need no keyboard or layer.
fn values(params: &[Param]) -> Option<Vec<u32>> {
    if params.is_empty() {
        return Some(vec![0]);
    }
    let mut out = Vec::new();
    for param in params {
        match param {
            Param::Nil => out.push(0),
            Param::Constant(value, _) => out.push(*value),
            Param::Range(min, max) if max >= min && max - min < EXTRAS_PER_BEHAVIOR as i32 => {
                out.extend((*min..=*max).map(|v| v as u32));
            }
            _ => return None,
        }
    }
    Some(out)
}

fn behavior(id: i32, details: &[u8]) -> Result<Behavior> {
    let mut sets = Vec::new();
    for set in all(details, 3)? {
        let mut pair: [Vec<Param>; 2] = [Vec::new(), Vec::new()];
        for (slot, tag) in [(0, 1), (1, 2)] {
            for desc in all(set, tag)? {
                let name = text(desc, 1).unwrap_or_default();
                pair[slot].push(if let Some(value) = int(desc, 3)? {
                    Param::Constant(value as u32, name)
                } else if let Some(range) = field(desc, 4)? {
                    Param::Range(
                        int(range, 1)?.unwrap_or(0) as i32,
                        int(range, 2)?.unwrap_or(0) as i32,
                    )
                } else if field(desc, 5)?.is_some() {
                    Param::Usage
                } else if field(desc, 6)?.is_some() {
                    Param::Layer
                } else {
                    Param::Nil
                });
            }
        }
        sets.push(pair);
    }
    Ok(Behavior {
        id,
        name: text(details, 2).unwrap_or_default(),
        sets,
    })
}

// Protobuf, the little of it Studio needs: varints and length-delimited fields.

enum Value<'a> {
    Int(u64),
    Bytes(&'a [u8]),
}

fn varint(bytes: &[u8], at: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*at).ok_or(Error::Protocol("ZMK Studio reply"))?;
        *at += 1;
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(Error::Protocol("ZMK Studio reply"))
}

/// Every field in `message`, in order.
fn fields(message: &[u8]) -> Result<Vec<(u64, Value<'_>)>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < message.len() {
        let key = varint(message, &mut at)?;
        let value = match key & 7 {
            0 => Value::Int(varint(message, &mut at)?),
            2 => {
                let len = varint(message, &mut at)? as usize;
                let end = at
                    .checked_add(len)
                    .filter(|end| *end <= message.len())
                    .ok_or(Error::Protocol("ZMK Studio reply"))?;
                let bytes = &message[at..end];
                at = end;
                Value::Bytes(bytes)
            }
            1 => {
                at += 8;
                continue;
            }
            5 => {
                at += 4;
                continue;
            }
            _ => return Err(Error::Protocol("ZMK Studio reply")),
        };
        out.push((key >> 3, value));
    }
    Ok(out)
}

/// The last length-delimited `tag`.
fn field(message: &[u8], tag: u64) -> Result<Option<&[u8]>> {
    Ok(all(message, tag)?.pop())
}

fn all(message: &[u8], tag: u64) -> Result<Vec<&[u8]>> {
    Ok(fields(message)?
        .into_iter()
        .filter_map(|(t, v)| match v {
            Value::Bytes(bytes) if t == tag => Some(bytes),
            _ => None,
        })
        .collect())
}

/// The last varint `tag`.
fn int(message: &[u8], tag: u64) -> Result<Option<u64>> {
    Ok(ints(message, tag)?.pop())
}

/// A repeated varint, packed or not.
fn ints(message: &[u8], tag: u64) -> Result<Vec<u64>> {
    let mut out = Vec::new();
    for (t, value) in fields(message)? {
        match value {
            _ if t != tag => {}
            Value::Int(v) => out.push(v),
            Value::Bytes(packed) => {
                let mut at = 0;
                while at < packed.len() {
                    out.push(varint(packed, &mut at)?);
                }
            }
        }
    }
    Ok(out)
}

fn text(message: &[u8], tag: u64) -> Option<String> {
    let bytes = field(message, tag).ok()??;
    String::from_utf8(bytes.to_vec()).ok()
}

fn unzigzag(value: u64) -> i32 {
    ((value >> 1) as i64 ^ -((value & 1) as i64)) as i32
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn put_int(out: &mut Vec<u8>, tag: u64, value: u64) {
    put_varint(out, tag << 3);
    put_varint(out, value);
}

fn put_bytes(out: &mut Vec<u8>, tag: u64, bytes: &[u8]) {
    put_varint(out, tag << 3 | 2);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::super::{load, Board, Commit, Place, Range, Write};
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    const KP: i32 = 10;
    const MT: i32 = 11;
    const MO: i32 = 12;
    const BT: i32 = 13;
    const HM: i32 = 14;
    const UNLOCK: i32 = 15;
    const TRANS: i32 = 16;
    /// Layer ids differ from their indexes, as after a reorder.
    const BASE: u32 = 5;
    const UPPER: u32 = 7;
    const LAYOUTS: &[(&str, usize)] = &[("default", 4), ("five", 5)];
    fn layout_msg(name: &str, keys: usize) -> Vec<u8> {
        let mut layout = Vec::new();
        put_bytes(&mut layout, 1, name.as_bytes());
        for x in 0..keys as u64 {
            let mut attrs = Vec::new();
            put_int(&mut attrs, 1, 200);
            put_int(&mut attrs, 2, 200);
            put_int(&mut attrs, 3, x * 200);
            put_bytes(&mut layout, 2, &attrs);
        }
        layout
    }

    const fn key(id: u32) -> u32 {
        0x07 << 16 | id
    }

    fn bind(behavior: i32, param1: u32, param2: u32) -> Binding {
        Binding {
            behavior,
            param1,
            param2,
        }
    }

    #[derive(Default)]
    struct Firmware {
        locked: bool,
        layers: Vec<(u32, Vec<Binding>)>,
        /// Layer names by id.
        names: std::collections::HashMap<u32, String>,
        saves: usize,
        /// Which physical layout is selected.
        layout: usize,
    }

    /// Answers the way zmk/app/src/studio does, in protobuf.
    struct Fake(Rc<RefCell<Firmware>>);

    fn desc(name: &str, kind: u64, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        put_bytes(&mut out, 1, name.as_bytes());
        if kind == 3 {
            put_int(&mut out, 3, u64::from(payload[0]));
        } else {
            put_bytes(&mut out, kind, payload);
        }
        out
    }

    fn set(param1: &[Vec<u8>], param2: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        for d in param1 {
            put_bytes(&mut out, 1, d);
        }
        for d in param2 {
            put_bytes(&mut out, 2, d);
        }
        out
    }

    fn details(id: i32) -> Vec<u8> {
        let usage = || desc("Key", 5, &[]);
        let (name, sets) = match id {
            KP => ("Key Press", vec![set(&[usage()], &[])]),
            MT => ("Mod-Tap", vec![set(&[usage()], &[usage()])]),
            HM => ("homerow_mods", vec![set(&[usage()], &[usage()])]),
            MO => ("Momentary Layer", vec![set(&[desc("Layer", 6, &[])], &[])]),
            BT => (
                "Bluetooth",
                vec![
                    set(&[desc("Next Profile", 3, &[1])], &[]),
                    set(
                        &[desc("Select Profile", 3, &[3])],
                        &[desc("Profile", 4, &[0x10, 0x04])],
                    ),
                ],
            ),
            UNLOCK => ("Studio Unlock", vec![]),
            TRANS => ("Transparent", vec![]),
            _ => unreachable!(),
        };
        let mut body = Vec::new();
        put_int(&mut body, 1, id as u64);
        put_bytes(&mut body, 2, name.as_bytes());
        for s in sets {
            put_bytes(&mut body, 3, &s);
        }
        let mut out = Vec::new();
        put_bytes(&mut out, 2, &body);
        out
    }

    impl Rpc for Fake {
        fn call(&mut self, subsystem: u64, request: &[u8]) -> Result<Vec<u8>> {
            let mut fw = self.0.borrow_mut();
            let (tag, value) = fields(request)?.into_iter().next().expect("one request");
            if subsystem != CORE && fw.locked {
                return Err(Error::Locked);
            }
            let mut out = Vec::new();
            match (subsystem, tag, value) {
                (CORE, 2, _) => put_int(&mut out, 2, u64::from(!fw.locked)),
                (BEHAVIORS, 1, _) => {
                    let mut list = Vec::new();
                    for id in [KP, MT, MO, BT, HM, UNLOCK, TRANS] {
                        put_int(&mut list, 1, id as u64);
                    }
                    put_bytes(&mut out, 1, &list);
                }
                (BEHAVIORS, 2, Value::Bytes(ask)) => {
                    out = details(int(ask, 1)?.unwrap() as i32);
                }
                (KEYMAP, 1, _) => {
                    let mut keymap = Vec::new();
                    for (id, bindings) in &fw.layers {
                        let mut layer = Vec::new();
                        put_int(&mut layer, 1, u64::from(*id));
                        if let Some(name) = fw.names.get(id) {
                            put_bytes(&mut layer, 2, name.as_bytes());
                        }
                        for b in bindings {
                            let mut binding = Vec::new();
                            put_int(&mut binding, 1, u64::from((b.behavior << 1) as u32));
                            put_int(&mut binding, 2, u64::from(b.param1));
                            put_int(&mut binding, 3, u64::from(b.param2));
                            put_bytes(&mut layer, 3, &binding);
                        }
                        put_bytes(&mut keymap, 1, &layer);
                    }
                    put_bytes(&mut out, 1, &keymap);
                }
                (KEYMAP, 6, _) => {
                    let mut layouts = Vec::new();
                    put_int(&mut layouts, 1, fw.layout as u64);
                    for (name, keys) in LAYOUTS {
                        put_bytes(&mut layouts, 2, &layout_msg(name, *keys));
                    }
                    put_bytes(&mut out, 6, &layouts);
                }
                (KEYMAP, 7, Value::Int(index)) => {
                    let index = index as usize;
                    if index >= LAYOUTS.len() {
                        let mut err = Vec::new();
                        put_int(&mut err, 2, 1);
                        put_bytes(&mut out, 7, &err);
                    } else {
                        fw.layout = index;
                        let keys = LAYOUTS[index].1;
                        for layer in &mut fw.layers {
                            layer.1.resize(keys, bind(TRANS, 0, 0));
                        }
                        let mut ok = Vec::new();
                        put_bytes(&mut ok, 1, &[]);
                        put_bytes(&mut out, 7, &ok);
                    }
                }
                (KEYMAP, 2, Value::Bytes(ask)) => {
                    let layer = int(ask, 1)?.unwrap() as u32;
                    let position = int(ask, 2)?.unwrap_or(0) as usize;
                    let b = field(ask, 3)?.unwrap();
                    let binding = bind(
                        unzigzag(int(b, 1)?.unwrap_or(0)),
                        int(b, 2)?.unwrap_or(0) as u32,
                        int(b, 3)?.unwrap_or(0) as u32,
                    );
                    let slot = fw.layers.iter_mut().find(|l| l.0 == layer).unwrap();
                    slot.1[position] = binding;
                    put_int(&mut out, 2, 0);
                }
                (KEYMAP, 4, _) => {
                    fw.saves += 1;
                    put_bytes(&mut out, 4, &[0x08, 0x01]);
                }
                // add_layer: a new id after the highest, every key transparent.
                (KEYMAP, 9, _) => {
                    let id = fw.layers.iter().map(|l| l.0).max().unwrap_or(0) + 1;
                    let keys = fw.layers[0].1.len();
                    fw.layers.push((id, vec![bind(TRANS, 0, 0); keys]));
                    put_bytes(&mut out, 9, &[0x0A, 0x00]);
                }
                (KEYMAP, 10, Value::Bytes(ask)) => {
                    let index = int(ask, 1)?.unwrap_or(0) as usize;
                    fw.layers.remove(index);
                    put_bytes(&mut out, 10, &[0x0A, 0x00]);
                }
                (KEYMAP, 8, Value::Bytes(ask)) => {
                    let from = int(ask, 1)?.unwrap_or(0) as usize;
                    let to = int(ask, 2)?.unwrap_or(0) as usize;
                    let layer = fw.layers.remove(from);
                    fw.layers.insert(to, layer);
                    put_bytes(&mut out, 8, &[0x0A, 0x00]);
                }
                (KEYMAP, 12, Value::Bytes(ask)) => {
                    let id = int(ask, 1)?.unwrap_or(0) as u32;
                    fw.names.insert(id, text(ask, 2).unwrap_or_default());
                    put_int(&mut out, 12, 0);
                }
                _ => return Err(Error::Protocol("fake: unexpected request")),
            }
            Ok(out)
        }
    }

    fn firmware() -> Rc<RefCell<Firmware>> {
        Rc::new(RefCell::new(Firmware {
            layers: vec![
                (
                    BASE,
                    vec![
                        bind(KP, key(0x04), 0),
                        bind(MT, key(0xE1), key(0x05)),
                        bind(MO, UPPER, 0),
                        bind(HM, key(0xE3), key(0x06)),
                    ],
                ),
                (
                    UPPER,
                    vec![
                        bind(TRANS, 0, 0),
                        bind(BT, 3, 1),
                        bind(KP, 0x03 << 24 | key(0x07), 0),
                        bind(KP, 0x0C << 16 | 0xE9, 0),
                    ],
                ),
            ],
            ..Firmware::default()
        }))
    }

    fn open(fw: &Rc<RefCell<Firmware>>) -> Result<Board> {
        load(Candidate {
            listing: Box::default(),
            io: Some(Box::new(Zmk::new(Box::new(Fake(Rc::clone(fw)))))),
            guard: None,
        })
    }

    fn at(board: &Board, layer: u8, col: u8) -> u16 {
        board.code_at(layer, Place::Matrix { row: 0, col }).unwrap()
    }

    #[test]
    fn bindings_read_as_keycodes() {
        let board = open(&firmware()).unwrap();
        // A, LSFT_T(B), MO(1), then the hold-tap QMK has no code for.
        assert_eq!(
            (0..4).map(|c| at(&board, 0, c)).collect::<Vec<_>>(),
            [0x0004, 0x2205, 0x5221, CUSTOM]
        );
        // TRNS, Bluetooth profile 1, LCS(D), Vol +.
        assert_eq!(
            (0..4).map(|c| at(&board, 1, c)).collect::<Vec<_>>(),
            [0x0001, CUSTOM + 1, 0x0307, 0x00A9]
        );
        // Each reads as its ZMK binding: the behavior over its parameters.
        assert_eq!(board.legend(CUSTOM), "homerow_mods(⌘ C)");
        assert_eq!(board.binding_text(CUSTOM).unwrap(), "&homerow_mods LGUI C");
        assert_eq!(board.legend(CUSTOM + 1), "bt(BT_SEL 1)");
        assert_eq!(board.legend(0x5221), "mo(1)");
    }

    #[test]
    fn key_list_offers_only_what_zmk_stores() {
        let board = open(&firmware()).unwrap();
        let find = |text| board.list(Range::Find(text)).hits;
        assert!(find("Studio Unlock").iter().any(|hit| hit.code >= CUSTOM));
        assert!(find("Bluetooth Next Profile")
            .iter()
            .any(|hit| hit.code >= CUSTOM));
        assert!(find("MO(1)").iter().any(|hit| hit.code == 0x5221));
        assert!(find("DF(1)").is_empty());
        // No `&bootloader` on this board, so QMK's Boot key has nothing to become.
        assert!(!find("Boot").iter().any(|hit| hit.code == 0x7C00));
    }

    #[test]
    fn behaviors_without_metadata_take_keymap_parameters() {
        // Values read from a real Corne's `&mmv` and `&msc` keys.
        assert_eq!(bare_param("MOVE_X(-2500)"), Some(4131127296));
        assert_eq!(bare_param("MOVE_Y(2500)"), Some(2500));
        assert_eq!(bare_param("MOVE_Y(-20)"), Some(65516));
        assert_eq!(bare_param("SCRL_DOWN"), Some(0xFFF6));
        assert_eq!(bare_param("MOVE_LEFT"), bare_param("MOVE_X(-600)"));
        assert_eq!(bare_param("EP_TOG"), Some(2));
        assert_eq!(bare_param("4293656576"), Some(4293656576));
        assert_eq!(bare_param("-1"), Some(u32::MAX));
        assert_eq!(bare_param("MOVE_Z(1)"), None);
        for value in [4131127296, 2500, 65516, 1310720] {
            assert_eq!(bare_param(&move_text(value)), Some(value));
        }
        assert_eq!(move_text(4131127296), "MOVE_X(-2500)");
    }

    #[test]
    fn a_binding_is_composed_from_the_behaviors_the_board_sent() {
        let fw = firmware();
        let mut board = open(&fw).unwrap();
        let listed = board.list(Range::Find("&"));
        assert_eq!(listed.title, "behavior");
        assert!(listed
            .hits
            .iter()
            .any(|hit| hit.legend == "&kp" && hit.token == "Key Press"));
        assert!(listed.hits.iter().any(|hit| hit.legend == "&homerow_mods"));
        assert!(listed.hits.iter().any(|hit| hit.legend == "&trans"));

        let listed = board.list(Range::Find("&kp "));
        assert_eq!(listed.title, "key");
        // A hold-tap's parameters are named by role.
        assert_eq!(board.list(Range::Find("&mt ")).title, "hold · key");
        assert_eq!(board.list(Range::Find("&mt LSHIFT ")).title, "tap · key");
        assert!(listed.hits.iter().any(|hit| hit.legend == "A"));
        assert_eq!(board.intern("&kp A"), Some(0x0004));
        assert_eq!(board.intern("&mt LSHIFT B"), Some(0x2205));
        assert_eq!(board.intern("&trans"), Some(0x0001));
        assert_eq!(board.intern("&mo 1"), Some(0x5221));
        let listed = board.list(Range::Find("&kp LG(C)"));
        assert_eq!(
            listed.hits.first().map(|hit| hit.legend.as_str()),
            Some("LG(C)")
        );
        assert!(board.intern("&kp LG(C)").is_some());

        // Studio describes `&bt` by display name and value; the tokens are ZMK's own names.
        let listed = board.list(Range::Find("&bt "));
        let tokens: Vec<&str> = listed.hits.iter().map(|hit| hit.legend.as_str()).collect();
        assert!(
            tokens.contains(&"BT_SEL") && tokens.contains(&"BT_NXT"),
            "{tokens:?}"
        );
        let listed = board.list(Range::Find("&bt BT_SEL "));
        assert!(listed.hits.iter().any(|hit| hit.legend == "1"));
        let code = board.intern("&bt BT_SEL 1").unwrap();
        assert_eq!(board.binding_text(code).unwrap(), "&bt BT_SEL 1");
        // The same binding the board already had: BT_SEL is command 3, profile 1.
        assert_eq!(
            board.code_at(1, Place::Matrix { row: 0, col: 1 }),
            Some(code)
        );

        // The same path the key list uses: `&`, enter, the parameter, enter, then `:w`.
        let mut model = crate::session::Model::start(crate::session::Attached {
            kind: crate::session::Kind::Ready(crate::session::Session::mapped(board)),
        });
        model.handle(crate::Event::Char('/'), 80).unwrap();
        for ch in "&kp".chars() {
            model.handle(crate::Event::Char(ch), 80).unwrap();
        }
        model.handle(crate::Event::Confirm, 80).unwrap();
        model.handle(crate::Event::Char('Z'), 80).unwrap();
        model.handle(crate::Event::Confirm, 80).unwrap();
        let text = command(&mut model, "w");
        assert!(text.contains('Z'), "{text}");
        assert_eq!(fw.borrow().layers[0].1[0], bind(KP, key(0x1D), 0));
    }

    #[test]
    fn a_write_sets_the_binding_and_saves() {
        let fw = firmware();
        let mut board = open(&fw).unwrap();
        let write = Write::Key {
            layer: 1,
            place: Place::Matrix { row: 0, col: 0 },
            code: 0x5221,
        };
        assert!(matches!(board.commit(&write), Ok(Commit::Stored)));
        assert_eq!(fw.borrow().layers[1].1[0], bind(MO, UPPER, 0));
        assert_eq!(fw.borrow().saves, 1);
        drop(board);
        assert_eq!(at(&open(&fw).unwrap(), 1, 0), 0x5221);
    }

    /// `firmware()`'s keymap as a zmk-config writes it: the hold-tap by its node label `hm`,
    /// which Studio reports by its display name.
    const LINKED_KEYMAP: &str = r#"
/ {
    behaviors {
        hm: homerow_mods {
            compatible = "zmk,behavior-hold-tap";
            #binding-cells = <2>;
            bindings = <&kp>, <&kp>;
        };
    };
    keymap {
        compatible = "zmk,keymap";
        base { bindings = <&kp A &mt LSHIFT B &mo 1 &hm LGUI C>; };
        upper { bindings = <&trans &kp X &kp Y &kp Z>; };
    };
};
"#;

    fn linked(fw: &Rc<RefCell<Firmware>>, name: &str, keymap: &str) -> Candidate {
        let dir =
            std::env::temp_dir().join(format!("lazykeymap-link-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("corne.keymap");
        std::fs::write(&path, keymap).unwrap();
        let mut files = super::super::file_candidates(&path).unwrap();
        let board = Candidate {
            listing: Box::new(super::super::Listing {
                name: "Corne".to_string(),
                ..Default::default()
            }),
            io: Some(Box::new(Zmk::new(Box::new(Fake(Rc::clone(fw)))))),
            guard: None,
        };
        super::super::link_with(&mut files, vec![board]);
        files.remove(0)
    }

    fn linked_firmware() -> Rc<RefCell<Firmware>> {
        let fw = firmware();
        fw.borrow_mut().layers[1].1 = vec![
            bind(TRANS, 0, 0),
            bind(KP, key(0x1B), 0),
            bind(KP, key(0x1C), 0),
            bind(KP, key(0x1D), 0),
        ];
        fw
    }

    #[test]
    fn a_keymap_file_the_keyboard_runs_writes_to_both() {
        let fw = linked_firmware();
        let file = linked(&fw, "both", LINKED_KEYMAP);
        assert_eq!(file.listing.name, "corne + Corne");
        let path = std::path::PathBuf::from(&file.listing.usb);
        let mut board = load(file).unwrap();
        // A key, and the hold-tap under its file label, which the keyboard knows by another name.
        let hm = board.intern("&hm LCTRL D").unwrap();
        for (col, code) in [(0, 0x1D), (3, hm)] {
            let write = Write::Key {
                layer: 0,
                place: Place::Matrix { row: 0, col },
                code,
            };
            assert!(matches!(board.commit(&write), Ok(Commit::Stored)));
        }
        assert_eq!(fw.borrow().layers[0].1[0], bind(KP, key(0x1D), 0));
        assert_eq!(fw.borrow().layers[0].1[3], bind(HM, key(0xE0), key(0x07)));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("&kp Z  &mt LSHIFT B  &mo 1  &hm LCTRL D"),
            "{text}"
        );
    }

    #[test]
    fn a_keymap_file_the_keyboard_does_not_run_says_why_and_edits_the_file_alone() {
        let fw = linked_firmware();
        let file = linked(&fw, "differs", &LINKED_KEYMAP.replace("&kp Y", "&kp Q"));
        assert_eq!(file.listing.name, "corne");
        assert!(
            file.listing
                .serial
                .contains("not linked to Corne: layer 1 key 2"),
            "{}",
            file.listing.serial
        );
        let mut board = load(file).unwrap();
        let write = Write::Key {
            layer: 0,
            place: Place::Matrix { row: 0, col: 0 },
            code: 0x1D,
        };
        assert!(matches!(board.commit(&write), Ok(Commit::Stored)));
        assert_eq!(fw.borrow().layers[0].1[0], bind(KP, key(0x04), 0));
    }

    #[test]
    fn e_links_the_open_keyboard_to_the_keymap_file_it_runs() {
        let fw = linked_firmware();
        let dir = std::env::temp_dir().join(format!("lazykeymap-e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let other = dir.join("other.keymap");
        std::fs::write(&other, LINKED_KEYMAP.replace("&kp Y", "&kp Q")).unwrap();
        let corne = dir.join("corne.keymap");
        std::fs::write(&corne, LINKED_KEYMAP).unwrap();
        let mut model = crate::session::Model::start(crate::session::Attached {
            kind: crate::session::Kind::Ready(crate::session::Session::mapped(open(&fw).unwrap())),
        });
        // A file the keyboard does not run says where, and the board stays as it was.
        let text = command(&mut model, &format!("e {}", other.display()));
        assert!(text.contains("not linked: layer 1 key 2"), "{text}");
        // The one it runs links: the header names both, and `:w` writes both.
        let text = command(&mut model, &format!("e {}", corne.display()));
        assert!(text.contains("corne + "), "{text}");
        let crate::session::ModelKind::Map(session) = &mut model.kind else {
            panic!("not on the board");
        };
        session.board.stage(Write::Key {
            layer: 1,
            place: Place::Matrix { row: 0, col: 1 },
            code: 0x14,
        });
        let text = command(&mut model, "w");
        assert!(text.contains("the file and the keyboard"), "{text}");
        assert_eq!(fw.borrow().layers[1].1[1], bind(KP, key(0x14), 0));
        let file = std::fs::read_to_string(&corne).unwrap();
        assert!(file.contains("&trans &kp Q &kp Y &kp Z"), "{file}");
    }

    #[test]
    fn a_locked_board_says_how_to_unlock() {
        let fw = firmware();
        fw.borrow_mut().locked = true;
        assert!(
            matches!(open(&fw), Err(Error::Locked)),
            "a locked board opened"
        );
    }

    #[test]
    fn a_locked_board_opens_and_saves_once_unlocked_with_no_enter() {
        let fw = firmware();
        fw.borrow_mut().locked = true;
        let candidate = Candidate {
            listing: Box::default(),
            io: Some(Box::new(Zmk::new(Box::new(Fake(Rc::clone(&fw)))))),
            guard: None,
        };
        let mut model = crate::session::Model::start(crate::session::Attached {
            kind: crate::session::Kind::Pick(vec![candidate]),
        });
        let ticks = |model: &mut crate::session::Model| {
            for _ in 0..10 {
                model.handle(crate::Event::Tick, 80).unwrap();
            }
            plain(&model.frame(80, 20))
        };
        // Even one board waits for Enter on its card.
        let text = ticks(&mut model);
        assert!(
            !text.contains("opens by itself") && !text.contains("L1"),
            "{text}"
        );
        // After Enter its card waits for the unlock key, and opens on it with no second Enter.
        model.handle(crate::Event::Confirm, 80).unwrap();
        let text = ticks(&mut model);
        assert!(text.contains("it opens by itself"), "{text}");
        fw.borrow_mut().locked = false;
        let text = ticks(&mut model);
        assert!(
            text.contains("L1") && !text.contains("opens by itself"),
            "{text}"
        );

        // Idle, it locks itself again: `:w` waits for the key, then writes.
        model.handle(crate::Event::Char('/'), 80).unwrap();
        model.handle(crate::Event::Char('Z'), 80).unwrap();
        model.handle(crate::Event::Confirm, 80).unwrap();
        fw.borrow_mut().locked = true;
        let text = command(&mut model, "w");
        assert!(text.contains("to finish writing"), "{text}");
        assert_eq!(fw.borrow().layers[0].1[0], bind(KP, key(0x04), 0));
        fw.borrow_mut().locked = false;
        ticks(&mut model);
        assert_eq!(fw.borrow().layers[0].1[0], bind(KP, key(0x1D), 0));
    }

    #[test]
    fn every_keycode_turns_back_into_itself() {
        let mut zmk = Zmk::new(Box::new(Fake(firmware())));
        zmk.read().unwrap();
        let mut spoken = 0;
        for code in 0..=u16::MAX {
            if let Some(binding) = zmk.binding(code) {
                assert_eq!(zmk.code(binding), Some(code), "{code:#06x}");
                spoken += 1;
            }
        }
        assert!(spoken > 1000, "{spoken}");
    }

    /// Hands out what it was given a few bytes at a time, 20 ms apart, as a Bluetooth link does.
    struct Trickle(Vec<u8>);

    impl Link for Trickle {
        fn send(&mut self, _bytes: &[u8]) -> Result<()> {
            Ok(())
        }

        fn recv(&mut self, _until: Instant) -> Result<Vec<u8>> {
            std::thread::sleep(Duration::from_millis(20));
            let take = self.0.len().min(3);
            Ok(self.0.drain(..take).collect())
        }
    }

    fn framed(message: &[u8]) -> Vec<u8> {
        let mut out = vec![SOF];
        for &byte in message {
            if matches!(byte, SOF | ESC | EOF) {
                out.push(ESC);
            }
            out.push(byte);
        }
        out.push(EOF);
        out
    }

    /// The reply takes longer than `wait` in all, but never goes quiet that long.
    #[test]
    fn a_slow_split_escaped_reply_after_a_notification_is_read_whole() {
        // `«`, `¬`, and a soft hyphen are UTF-8 `C2 AB`, `C2 AC`, `C2 AD`: SOF, ESC, and EOF.
        let name = "Z«¬\u{AD}K";
        let mut info = Vec::new();
        put_bytes(&mut info, 1, name.as_bytes());
        let mut core = Vec::new();
        put_bytes(&mut core, 1, &info);
        let mut answer = Vec::new();
        put_int(&mut answer, 1, 1);
        put_bytes(&mut answer, CORE, &core);
        let mut reply = Vec::new();
        put_bytes(&mut reply, 1, &answer);
        // A lock-state notification first, then noise outside any frame.
        let mut notice = Vec::new();
        put_bytes(&mut notice, 2, &[0x12, 0x02, 0x08, 0x01]);
        let mut wire = framed(&notice);
        wire.push(0x00);
        wire.extend(framed(&reply));
        let start = Instant::now();
        let (_, got) = probe(Box::new(Trickle(wire)), Duration::from_millis(100)).unwrap();
        assert!(
            start.elapsed() > Duration::from_millis(100),
            "the reply must outlast `wait`"
        );
        assert_eq!(got.as_deref(), Some(name));
    }

    /// Each piece of the first read is its own step, so the screen repaints and says what is
    /// being read while a Bluetooth board answers slowly.
    #[test]
    fn opening_reads_one_piece_per_step() {
        let candidate = Candidate {
            listing: Box::default(),
            io: Some(Box::new(Zmk::new(Box::new(Fake(firmware()))))),
            guard: None,
        };
        let mut candidate = candidate;
        let mut load = super::super::Load::begin(&candidate).unwrap();
        let mut seen = Vec::new();
        while !load.ready() {
            load.step(&mut candidate).unwrap();
            seen.push(load.progress().what);
        }
        assert_eq!(seen[..3], ["behaviors", "behaviors 0/7", "behaviors 1/7"]);
        assert!(seen.contains(&"keymap".to_string()), "{seen:?}");
        assert!(seen.contains(&"layout".to_string()), "{seen:?}");
    }

    /// Types `line` on the `:` line, runs it, and ticks until the keyboard is read again.
    fn command(model: &mut crate::session::Model, line: &str) -> String {
        model.handle(crate::Event::Char(':'), 80).unwrap();
        for ch in line.chars() {
            model.handle(crate::Event::Char(ch), 80).unwrap();
        }
        model.handle(crate::Event::Confirm, 80).unwrap();
        for _ in 0..50 {
            model.handle(crate::Event::Tick, 80).unwrap();
        }
        plain(&model.frame(80, 20))
    }

    fn plain(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            if ch == '\x1b' {
                for end in chars.by_ref() {
                    if end.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(ch);
            }
        }
        out
    }

    #[test]
    fn layer_commands_change_the_keyboard_and_read_it_again() {
        let fw = firmware();
        let mut model = crate::session::Model::start(crate::session::Attached {
            kind: crate::session::Kind::Ready(crate::session::Session::mapped(open(&fw).unwrap())),
        });

        let text = command(&mut model, "layer-rename Nav");
        assert_eq!(
            fw.borrow().names.get(&BASE).map(String::as_str),
            Some("Nav")
        );
        assert!(
            text.contains("L0 Nav") && text.contains("layer 0/2  Nav"),
            "{text}"
        );

        let text = command(&mut model, "layer-add");
        assert_eq!(fw.borrow().layers.len(), 3);
        assert!(text.contains("layer 2/3"), "{text}");
        assert_eq!(fw.borrow().saves, 2);

        // The new layer moves left, past the second.
        let text = command(&mut model, "layer-left");
        let ids: Vec<u32> = fw.borrow().layers.iter().map(|l| l.0).collect();
        assert_eq!(ids, [BASE, UPPER + 1, UPPER]);
        assert!(text.contains("layer 1/3"), "{text}");

        // Removing asks first; nothing changes until Enter.
        let text = command(&mut model, "layer-remove");
        assert!(text.contains("remove layer 1"), "{text}");
        assert_eq!(fw.borrow().layers.len(), 3);
        model.handle(crate::Event::Confirm, 80).unwrap();
        for _ in 0..50 {
            model.handle(crate::Event::Tick, 80).unwrap();
        }
        let text = plain(&model.frame(80, 20));
        let ids: Vec<u32> = fw.borrow().layers.iter().map(|l| l.0).collect();
        assert_eq!(ids, [BASE, UPPER]);
        assert!(text.contains("layer 0/2"), "{text}");
    }

    #[test]
    fn layer_commands_wait_for_unsaved_changes() {
        let fw = firmware();
        let mut board = open(&fw).unwrap();
        board.stage(Write::Key {
            layer: 0,
            place: Place::Matrix { row: 0, col: 0 },
            code: 0x05,
        });
        let mut model = crate::session::Model::start(crate::session::Attached {
            kind: crate::session::Kind::Ready(crate::session::Session::mapped(board)),
        });
        let text = command(&mut model, "layer-add");
        assert!(
            text.contains("write or discard the changes first"),
            "{text}"
        );
        assert_eq!(fw.borrow().layers.len(), 2);
    }

    #[test]
    fn layout_switch_rereads_the_keyboard() {
        let fw = firmware();
        let mut model = crate::session::Model::start(crate::session::Attached {
            kind: crate::session::Kind::Ready(crate::session::Session::mapped(open(&fw).unwrap())),
        });
        let text = command(&mut model, "layout");
        assert!(
            text.contains("0 default*") && text.contains("1 five"),
            "{text}"
        );
        let text = command(&mut model, "layout five");
        assert_eq!(fw.borrow().layout, 1);
        assert_eq!(fw.borrow().layers[0].1.len(), 5);
        assert!(fw.borrow().saves >= 1);
        assert!(
            text.contains("layout 1 five") || text.contains("0,4"),
            "{text}"
        );
    }
}
