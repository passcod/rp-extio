use crate::{
    backend::{Backend, BackendReply, Transaction},
    kernel::{DeviceRegistration, Kernel, RegistrationConfig},
};
use async_trait::async_trait;
use futures_lite::StreamExt;
use nusb::{
    DeviceId, DeviceInfo, Endpoint,
    descriptors::TransferType,
    transfer::{Buffer, Bulk, Direction, In, Out},
};
use rp_extio_wire::{
    CAP_GENERAL, CAP_GPIO, CAP_I2C, CAP_SPI, CAP_UART, MAX_PAYLOAD, Operation, Status,
    USB_HEADER_LEN, UsbHeader,
};
use std::{
    collections::HashMap,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, mpsc};

pub const DEVELOPMENT_VID: u16 = 0xcafe;
pub const DEVELOPMENT_PID: u16 = 0x4010;
const USB_DEADLINE: Duration = Duration::from_secs(6);

struct ConnectedDevice {
    backend: Arc<UsbBackend>,
    registration: DeviceRegistration,
    gpio_poll: tokio::task::JoinHandle<()>,
}

pub async fn run(kernel: Arc<Kernel>) -> io::Result<()> {
    let mut watch = nusb::watch_devices().map_err(nusb_error)?;
    let initial: Vec<_> = nusb::list_devices()
        .await
        .map_err(nusb_error)?
        .filter(matches_bridge)
        .collect();
    let (failed_tx, mut failed_rx) = mpsc::unbounded_channel();
    let mut devices = HashMap::new();
    for info in initial {
        try_connect(&kernel, &failed_tx, &mut devices, info).await;
    }
    loop {
        tokio::select! {
            event = watch.next() => match event {
                Some(nusb::hotplug::HotplugEvent::Connected(info)) if matches_bridge(&info) => {
                    try_connect(&kernel, &failed_tx, &mut devices, info).await;
                }
                Some(nusb::hotplug::HotplugEvent::Disconnected(id)) => {
                    remove_device(&mut devices, &id).await;
                }
                Some(nusb::hotplug::HotplugEvent::Connected(_)) => {}
                None => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "USB hotplug watch ended")),
            },
            failed = failed_rx.recv() => match failed {
                Some(id) => remove_device(&mut devices, &id).await,
                None => return Err(io::Error::new(io::ErrorKind::BrokenPipe, "USB failure channel closed")),
            },
        }
    }
}

fn matches_bridge(info: &DeviceInfo) -> bool {
    info.vendor_id() == DEVELOPMENT_VID
        && info.product_id() == DEVELOPMENT_PID
        && info.interfaces().any(|interface| {
            interface.class() == 0xff && interface.subclass() == 0 && interface.protocol() == 0
        })
}

async fn try_connect(
    kernel: &Arc<Kernel>,
    failed: &mpsc::UnboundedSender<DeviceId>,
    devices: &mut HashMap<DeviceId, ConnectedDevice>,
    info: DeviceInfo,
) {
    let id = info.id();
    if devices.contains_key(&id) {
        return;
    }
    let (backend, config) = match UsbBackend::connect(&info, failed.clone()).await {
        Ok(connected) => connected,
        Err(error) => {
            eprintln!(
                "failed to connect rp-extio USB device {}: {error}",
                info.serial_number().unwrap_or("without serial number")
            );
            return;
        }
    };
    match kernel.register_device(backend.clone(), config) {
        Ok(registration) => {
            let gpio_instance_id = registration.gpio_instance_id();
            let gpio_poll = tokio::spawn(poll_gpio(
                backend.clone(),
                kernel.clone(),
                gpio_instance_id,
                config.gpio_lines,
            ));
            devices.insert(
                id,
                ConnectedDevice {
                    backend,
                    registration,
                    gpio_poll,
                },
            );
        }
        Err(error) => {
            eprintln!(
                "failed to register rp-extio USB device {}: {error}",
                info.serial_number().unwrap_or("without serial number")
            );
            backend.disconnect().await;
        }
    }
}

async fn remove_device(devices: &mut HashMap<DeviceId, ConnectedDevice>, id: &DeviceId) {
    if let Some(device) = devices.remove(id) {
        device.gpio_poll.abort();
        device.backend.disconnect().await;
        let _ = device.registration.destroy();
    }
}

async fn poll_gpio(backend: Arc<UsbBackend>, kernel: Arc<Kernel>, instance_id: u32, lines: u16) {
    let mut previous: Option<u32> = None;
    let mut interval = tokio::time::interval(Duration::from_millis(10));
    loop {
        interval.tick().await;
        let reply = backend
            .transact(Transaction::Gpio {
                command: rp_extio_wire::GPIO_SNAPSHOT,
                line: 0,
                value: 0,
            })
            .await;
        if reply.status != Status::Ok as i32
            || reply.completed != lines as u32
            || reply.data.len() != 4
        {
            backend.fail();
            return;
        }
        let values = u32::from_le_bytes(reply.data.try_into().unwrap());
        if let Some(old) = previous {
            let mut changed = (old ^ values) & ((1_u32 << lines) - 1);
            while changed != 0 {
                let line = changed.trailing_zeros() as u8;
                let mask = 1_u32 << line;
                if kernel
                    .gpio_event(
                        instance_id,
                        line,
                        u8::from(old & mask != 0),
                        u8::from(values & mask != 0),
                    )
                    .is_err()
                {
                    backend.fail();
                    return;
                }
                changed &= !mask;
            }
        }
        previous = Some(values);
    }
}

struct Transport {
    out_endpoint: Endpoint<Bulk, Out>,
    in_endpoint: Endpoint<Bulk, In>,
}

pub struct UsbBackend {
    transport: Mutex<Transport>,
    next_request_id: AtomicU64,
    connected: AtomicBool,
    device_id: DeviceId,
    failed: mpsc::UnboundedSender<DeviceId>,
}

impl UsbBackend {
    pub async fn connect(
        info: &DeviceInfo,
        failed: mpsc::UnboundedSender<DeviceId>,
    ) -> io::Result<(Arc<Self>, RegistrationConfig)> {
        let interface_number = info
            .interfaces()
            .find(|interface| {
                interface.class() == 0xff && interface.subclass() == 0 && interface.protocol() == 0
            })
            .map(|interface| interface.interface_number())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "vendor interface not found"))?;
        let device = info.open().await.map_err(nusb_error)?;
        let interface = device
            .claim_interface(interface_number)
            .await
            .map_err(nusb_error)?;
        let descriptor = interface.descriptor().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "active interface not found")
        })?;
        let mut out_address = None;
        let mut in_address = None;
        for endpoint in descriptor.endpoints() {
            if endpoint.transfer_type() != TransferType::Bulk {
                continue;
            }
            match endpoint.direction() {
                Direction::Out if out_address.replace(endpoint.address()).is_some() => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "multiple bulk OUT endpoints",
                    ));
                }
                Direction::In if in_address.replace(endpoint.address()).is_some() => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "multiple bulk IN endpoints",
                    ));
                }
                Direction::Out | Direction::In => {}
            }
        }
        let out_endpoint = interface
            .endpoint::<Bulk, Out>(out_address.ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "bulk OUT endpoint not found")
            })?)
            .map_err(nusb_error)?;
        let in_endpoint = interface
            .endpoint::<Bulk, In>(in_address.ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "bulk IN endpoint not found")
            })?)
            .map_err(nusb_error)?;
        let backend = Arc::new(Self {
            transport: Mutex::new(Transport {
                out_endpoint,
                in_endpoint,
            }),
            next_request_id: AtomicU64::new(2),
            connected: AtomicBool::new(true),
            device_id: info.id(),
            failed,
        });
        let hello = backend
            .exchange(Operation::Hello, 1, Vec::new())
            .await
            .map_err(|failure| failure.error)?;
        if hello.header.status != Status::Ok {
            return Err(io::Error::other(format!(
                "HELLO failed with {:?}",
                hello.header.status
            )));
        }
        let capabilities = parse_capabilities(&hello.payload)?;
        Ok((backend, capabilities))
    }

    fn fail(&self) {
        if self.connected.swap(false, Ordering::AcqRel) {
            let _ = self.failed.send(self.device_id);
        }
    }

    async fn exchange(
        &self,
        operation: Operation,
        request_id: u64,
        payload: Vec<u8>,
    ) -> Result<UsbReply, UsbFailure> {
        let header = UsbHeader {
            operation,
            reply: false,
            request_id,
            payload_len: payload.len() as u32,
            status: Status::Ok,
        };
        let mut frame = Vec::with_capacity(USB_HEADER_LEN + payload.len());
        frame.extend_from_slice(&header.encode());
        frame.extend_from_slice(&payload);
        let mut transport = self.transport.lock().await;
        match tokio::time::timeout(
            USB_DEADLINE,
            transfer(&mut transport, operation, request_id, frame),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(UsbFailure::new(
                Status::Timeout,
                io::Error::new(io::ErrorKind::TimedOut, "USB transaction timed out"),
            )),
        }
    }
}

#[async_trait]
impl Backend for UsbBackend {
    async fn transact(&self, transaction: Transaction) -> BackendReply {
        if !self.connected.load(Ordering::Acquire) {
            return BackendReply::error(Status::Disconnected);
        }
        let (operation, payload) = match encode_transaction(&transaction) {
            Ok(encoded) => encoded,
            Err(_) => return BackendReply::error(Status::BadFrame),
        };
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        match self.exchange(operation, request_id, payload).await {
            Ok(reply) => match decode_reply(&transaction, reply) {
                Ok(reply) => {
                    if reply.status == Status::Disconnected as i32 {
                        self.fail();
                    }
                    reply
                }
                Err(_) => {
                    self.fail();
                    BackendReply::error(Status::BadFrame)
                }
            },
            Err(failure) => {
                self.fail();
                BackendReply::error(failure.status)
            }
        }
    }

    async fn disconnect(&self) {
        self.connected.store(false, Ordering::Release);
    }
}

struct UsbReply {
    header: UsbHeader,
    payload: Vec<u8>,
}

struct UsbFailure {
    status: Status,
    error: io::Error,
}

impl UsbFailure {
    fn new(status: Status, error: io::Error) -> Self {
        Self { status, error }
    }
}

async fn transfer(
    transport: &mut Transport,
    operation: Operation,
    request_id: u64,
    frame: Vec<u8>,
) -> Result<UsbReply, UsbFailure> {
    transport.out_endpoint.submit(frame.into());
    let completion = transport.out_endpoint.next_complete().await;
    let actual_len = completion.actual_len;
    let expected_len = completion.buffer.len();
    completion.status.map_err(transfer_error)?;
    if actual_len != expected_len {
        return Err(protocol_error("short USB OUT transfer"));
    }

    let packet_size = transport.in_endpoint.max_packet_size();
    let mut received = Vec::new();
    let mut expected = None;
    loop {
        transport.in_endpoint.submit(Buffer::new(packet_size));
        let packet = transport
            .in_endpoint
            .next_complete()
            .await
            .into_result()
            .map_err(transfer_error)?;
        if packet.is_empty() {
            return Err(protocol_error("empty USB packet before complete frame"));
        }
        received.extend_from_slice(&packet);
        if expected.is_none() && received.len() >= USB_HEADER_LEN {
            let header = UsbHeader::decode(&received[..USB_HEADER_LEN])
                .map_err(|_| protocol_error("invalid USB reply header"))?;
            if !header.reply || header.operation != operation || header.request_id != request_id {
                return Err(protocol_error("USB reply does not match request"));
            }
            expected = Some(USB_HEADER_LEN + header.payload_len as usize);
        }
        if let Some(expected) = expected {
            if received.len() > expected {
                return Err(protocol_error("extra bytes after USB frame"));
            }
            if received.len() == expected {
                let header = UsbHeader::decode(&received[..USB_HEADER_LEN])
                    .map_err(|_| protocol_error("invalid USB reply header"))?;
                return Ok(UsbReply {
                    header,
                    payload: received.split_off(USB_HEADER_LEN),
                });
            }
        }
    }
}

fn encode_transaction(transaction: &Transaction) -> io::Result<(Operation, Vec<u8>)> {
    let mut payload = Vec::new();
    match transaction {
        Transaction::I2c { bus_id, messages } => {
            payload.push(*bus_id);
            payload.push(
                messages
                    .len()
                    .try_into()
                    .map_err(|_| invalid("too many I2C messages"))?,
            );
            payload.extend_from_slice(&[0; 2]);
            for message in messages {
                let read = message.flags & rp_extio_wire::I2C_READ != 0;
                let length = if read {
                    message.read_len
                } else {
                    message
                        .data
                        .len()
                        .try_into()
                        .map_err(|_| invalid("I2C message too large"))?
                };
                payload.extend_from_slice(&message.address.to_le_bytes());
                payload.extend_from_slice(&message.flags.to_le_bytes());
                payload.extend_from_slice(&length.to_le_bytes());
                payload.extend_from_slice(&[0; 2]);
                if !read {
                    payload.extend_from_slice(&message.data);
                }
            }
            if payload.len() > MAX_PAYLOAD {
                return Err(invalid("I2C transaction too large"));
            }
            Ok((Operation::I2cTransfer, payload))
        }
        Transaction::Spi {
            bus_id,
            chip_select,
            mode,
            segments,
        } => {
            payload.push(*bus_id);
            payload.push(*chip_select);
            payload.extend_from_slice(&mode.to_le_bytes());
            payload.push(
                segments
                    .len()
                    .try_into()
                    .map_err(|_| invalid("too many SPI segments"))?,
            );
            payload.extend_from_slice(&[0; 3]);
            for segment in segments {
                let length: u16 = segment
                    .tx
                    .len()
                    .try_into()
                    .map_err(|_| invalid("SPI segment too large"))?;
                payload.extend_from_slice(&segment.speed_hz.to_le_bytes());
                payload.extend_from_slice(&length.to_le_bytes());
                payload.extend_from_slice(&segment.flags.to_le_bytes());
                payload.extend_from_slice(&segment.word_delay_ns.to_le_bytes());
                payload.extend_from_slice(&segment.delay_ns.to_le_bytes());
                payload.extend_from_slice(&segment.cs_change_delay_ns.to_le_bytes());
                payload.extend_from_slice(&segment.tx);
            }
            if payload.len() > MAX_PAYLOAD {
                return Err(invalid("SPI transaction too large"));
            }
            Ok((Operation::SpiMessage, payload))
        }
        Transaction::Gpio {
            command,
            line,
            value,
        } => Ok((Operation::GpioCommand, vec![*command, *line, *value, 0])),
    }
}

fn decode_reply(transaction: &Transaction, reply: UsbReply) -> io::Result<BackendReply> {
    if reply.payload.len() < 4 {
        return Err(invalid("USB bus reply is missing completion count"));
    }
    let completed = u32::from_le_bytes(reply.payload[..4].try_into().unwrap());
    if reply.header.status != Status::Ok {
        if reply.payload.len() != 4 {
            return Err(invalid("USB error reply has trailing data"));
        }
        return Ok(BackendReply {
            status: reply.header.status as i32,
            completed,
            data: Vec::new(),
            effective_speeds: Vec::new(),
        });
    }
    match transaction {
        Transaction::I2c { messages, .. } => {
            let expected: usize = messages
                .iter()
                .map(|message| message.read_len as usize)
                .sum();
            if completed != messages.len() as u32 || reply.payload.len() != 4 + expected {
                return Err(invalid("invalid successful I2C reply"));
            }
            Ok(BackendReply {
                status: Status::Ok as i32,
                completed,
                data: reply.payload[4..].to_vec(),
                effective_speeds: Vec::new(),
            })
        }
        Transaction::Spi { segments, .. } => {
            let data_len: usize = segments.iter().map(|segment| segment.tx.len()).sum();
            let metadata_len = 8 + segments.len() * 4;
            if completed != data_len as u32
                || reply.payload.len() != metadata_len + data_len
                || reply.payload[4] as usize != segments.len()
                || reply.payload[5..8] != [0; 3]
            {
                return Err(invalid("invalid successful SPI reply"));
            }
            let effective_speeds = reply.payload[8..metadata_len]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| u32::from_le_bytes(*bytes))
                .collect();
            Ok(BackendReply {
                status: Status::Ok as i32,
                completed,
                data: reply.payload[metadata_len..].to_vec(),
                effective_speeds,
            })
        }
        Transaction::Gpio { command, .. } => {
            if *command == rp_extio_wire::GPIO_SNAPSHOT {
                if reply.payload.len() != 8 {
                    return Err(invalid("invalid GPIO snapshot reply"));
                }
                return Ok(BackendReply {
                    status: Status::Ok as i32,
                    completed,
                    data: reply.payload[4..8].to_vec(),
                    effective_speeds: Vec::new(),
                });
            }
            if completed != 1
                || reply.payload.len() != 8
                || reply.payload[4] > 1
                || reply.payload[5..8] != [0; 3]
            {
                return Err(invalid("invalid successful GPIO reply"));
            }
            Ok(BackendReply {
                status: Status::Ok as i32,
                completed,
                data: vec![reply.payload[4]],
                effective_speeds: Vec::new(),
            })
        }
    }
}

fn parse_capabilities(payload: &[u8]) -> io::Result<RegistrationConfig> {
    let mut offset = 0;
    let mut general = false;
    let mut i2c = None;
    let mut spi = None;
    let mut uart = None;
    let mut gpio_pins = None;
    while offset < payload.len() {
        if payload.len() - offset < 4 {
            return Err(invalid("truncated capability record"));
        }
        let kind = u16::from_le_bytes(payload[offset..offset + 2].try_into().unwrap());
        let length =
            u16::from_le_bytes(payload[offset + 2..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let value = payload
            .get(offset..offset + length)
            .ok_or_else(|| invalid("truncated capability value"))?;
        offset += length;
        match kind {
            CAP_GENERAL => {
                if general
                    || value.len() != 8
                    || value[0] > rp_extio_wire::VERSION_MAJOR
                    || value[1] < rp_extio_wire::VERSION_MAJOR
                    || value[3] != 0
                    || u32::from_le_bytes(value[4..8].try_into().unwrap()) < MAX_PAYLOAD as u32
                {
                    return Err(invalid("incompatible general capability"));
                }
                general = true;
            }
            CAP_I2C => {
                if i2c.is_some()
                    || value.len() != 16
                    || value[0] != 0
                    || value[1] != 0
                    || u16::from_le_bytes(value[2..4].try_into().unwrap())
                        & (rp_extio_wire::I2C_READ | rp_extio_wire::I2C_TEN_BIT)
                        != (rp_extio_wire::I2C_READ | rp_extio_wire::I2C_TEN_BIT)
                    || value[14..16] != [0; 2]
                {
                    return Err(invalid("invalid I2C capability"));
                }
                let minimum = u32::from_le_bytes(value[4..8].try_into().unwrap());
                let maximum = u32::from_le_bytes(value[8..12].try_into().unwrap());
                if minimum == 0 || minimum > maximum {
                    return Err(invalid("invalid I2C speed range"));
                }
                i2c = Some((maximum, [value[12], value[13]]));
            }
            CAP_SPI => {
                if spi.is_some()
                    || value.len() < 16
                    || value[0] != 0
                    || value[1] == 0
                    || value.len() != 15 + value[1] as usize
                {
                    return Err(invalid("invalid SPI capability"));
                }
                let mode_bits = u16::from_le_bytes(value[2..4].try_into().unwrap()) as u32;
                let minimum = u32::from_le_bytes(value[4..8].try_into().unwrap());
                let maximum = u32::from_le_bytes(value[8..12].try_into().unwrap());
                if mode_bits & !RegistrationConfig::PICO2.spi_mode_bits != 0
                    || minimum == 0
                    || minimum > maximum
                {
                    return Err(invalid("invalid SPI capability values"));
                }
                spi = Some((
                    mode_bits,
                    minimum,
                    maximum,
                    value[1] as u16,
                    value[12..].to_vec(),
                ));
            }
            CAP_UART => {
                if uart.is_some()
                    || value.len() != 12
                    || value[0] != 0
                    || value[3] != 0
                    || value[8..12] != [0; 4]
                {
                    return Err(invalid("invalid UART capability"));
                }
                uart = Some([value[1], value[2]]);
            }
            CAP_GPIO => {
                if gpio_pins.is_some()
                    || value.len() < 4
                    || value[0] != 17
                    || value[1..4] != [0; 3]
                    || value.len() != 4 + value[0] as usize
                {
                    return Err(invalid("invalid GPIO capability"));
                }
                gpio_pins = Some(value[4..].try_into().unwrap());
            }
            _ => {}
        }
    }
    let (spi_mode_bits, spi_min_speed_hz, spi_max_speed_hz, spi_chip_selects, spi_pins) =
        spi.ok_or_else(|| invalid("SPI capability missing"))?;
    let (i2c_speed_hz, i2c_pins) = i2c.ok_or_else(|| invalid("I2C capability missing"))?;
    let uart_pins = uart.ok_or_else(|| invalid("UART capability missing"))?;
    let gpio_pins = gpio_pins.ok_or_else(|| invalid("GPIO capability missing"))?;
    if !general {
        return Err(invalid("general capability missing"));
    }
    validate_pin_map(i2c_pins, &spi_pins, uart_pins, &gpio_pins)?;
    Ok(RegistrationConfig {
        i2c_speed_hz,
        spi_mode_bits,
        spi_min_speed_hz,
        spi_max_speed_hz,
        spi_chip_selects,
        gpio_lines: gpio_pins.len() as u16,
        gpio_pins,
    })
}

fn validate_pin_map(i2c: [u8; 2], spi: &[u8], uart: [u8; 2], gpio: &[u8; 17]) -> io::Result<()> {
    const HEADER: [u8; 26] = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 26, 27,
        28,
    ];
    if ![0, 4, 8, 12, 16, 20].contains(&i2c[0]) || i2c[1] != i2c[0] + 1 {
        return Err(invalid("invalid I2C0 pin route"));
    }
    if spi.len() != 5
        || ![2, 6, 18, 22].contains(&spi[0])
        || ![3, 7, 19].contains(&spi[1])
        || ![0, 4, 16, 20].contains(&spi[2])
        || ![0, 2, 12, 14, 16, 18, 28].contains(&uart[0])
        || ![1, 3, 13, 15, 17, 19].contains(&uart[1])
    {
        return Err(invalid("invalid SPI0 or UART0 pin route"));
    }
    let mut used = [false; 29];
    for &pin in i2c.iter().chain(spi).chain(&uart).chain(gpio) {
        if !HEADER.contains(&pin) || used[pin as usize] {
            return Err(invalid("duplicate or unavailable configured pin"));
        }
        used[pin as usize] = true;
    }
    if HEADER.iter().any(|&pin| !used[pin as usize]) {
        return Err(invalid("configured pins do not cover Pico 2 headers"));
    }
    Ok(())
}

fn nusb_error(error: nusb::Error) -> io::Error {
    io::Error::other(error)
}

fn transfer_error(error: nusb::transfer::TransferError) -> UsbFailure {
    UsbFailure::new(Status::Disconnected, io::Error::from(error))
}

fn protocol_error(message: &'static str) -> UsbFailure {
    UsbFailure::new(Status::BadFrame, invalid(message))
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{I2cMessage, SpiSegment};

    #[test]
    fn parses_firmware_capabilities() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&CAP_GENERAL.to_le_bytes());
        payload.extend_from_slice(&8_u16.to_le_bytes());
        payload.extend_from_slice(&[1, 1, 0, 0]);
        payload.extend_from_slice(&(MAX_PAYLOAD as u32).to_le_bytes());
        payload.extend_from_slice(&CAP_I2C.to_le_bytes());
        payload.extend_from_slice(&16_u16.to_le_bytes());
        payload.extend_from_slice(&[0, 0]);
        payload.extend_from_slice(&3_u16.to_le_bytes());
        payload.extend_from_slice(&100_000_u32.to_le_bytes());
        payload.extend_from_slice(&100_000_u32.to_le_bytes());
        payload.extend_from_slice(&[4, 5, 0, 0]);
        payload.extend_from_slice(&CAP_SPI.to_le_bytes());
        payload.extend_from_slice(&17_u16.to_le_bytes());
        payload.extend_from_slice(&[0, 2]);
        payload.extend_from_slice(&0x4f_u16.to_le_bytes());
        payload.extend_from_slice(&50_000_u32.to_le_bytes());
        payload.extend_from_slice(&20_000_000_u32.to_le_bytes());
        payload.extend_from_slice(&[18, 19, 16, 17, 21]);
        payload.extend_from_slice(&CAP_UART.to_le_bytes());
        payload.extend_from_slice(&12_u16.to_le_bytes());
        payload.extend_from_slice(&[0, 0, 1, 0]);
        payload.extend_from_slice(&115_200_u32.to_le_bytes());
        payload.extend_from_slice(&0_u32.to_le_bytes());
        payload.extend_from_slice(&CAP_GPIO.to_le_bytes());
        payload.extend_from_slice(&21_u16.to_le_bytes());
        payload.extend_from_slice(&[17, 0, 0, 0]);
        payload.extend_from_slice(&rp_extio_firmware_gpio_pins());
        assert_eq!(
            parse_capabilities(&payload).unwrap(),
            RegistrationConfig::PICO2
        );
    }

    #[test]
    fn transaction_and_reply_codecs_preserve_bus_data() {
        let transaction = Transaction::I2c {
            bus_id: 0,
            messages: vec![
                I2cMessage {
                    address: 0x50,
                    flags: 0,
                    data: vec![0xaa],
                    read_len: 0,
                },
                I2cMessage {
                    address: 0x50,
                    flags: 1,
                    data: vec![],
                    read_len: 2,
                },
            ],
        };
        let (operation, payload) = encode_transaction(&transaction).unwrap();
        assert_eq!(operation, Operation::I2cTransfer);
        assert_eq!(&payload[..4], &[0, 2, 0, 0]);
        let reply = decode_reply(
            &transaction,
            UsbReply {
                header: UsbHeader {
                    operation,
                    reply: true,
                    request_id: 7,
                    payload_len: 6,
                    status: Status::Ok,
                },
                payload: vec![2, 0, 0, 0, 3, 4],
            },
        )
        .unwrap();
        assert_eq!(reply.data, [3, 4]);

        let spi = Transaction::Spi {
            bus_id: 0,
            chip_select: 1,
            mode: 3,
            segments: vec![SpiSegment {
                speed_hz: 1_000_000,
                flags: 0,
                word_delay_ns: 1,
                delay_ns: 2,
                cs_change_delay_ns: 3,
                tx: vec![0xaa],
            }],
        };
        assert_eq!(encode_transaction(&spi).unwrap().0, Operation::SpiMessage);
    }

    #[test]
    fn gpio_transaction_and_reply_codecs_validate_commands_and_snapshots() {
        let command = Transaction::Gpio {
            command: rp_extio_wire::GPIO_GET,
            line: 4,
            value: 0,
        };
        assert_eq!(
            encode_transaction(&command).unwrap(),
            (
                Operation::GpioCommand,
                vec![rp_extio_wire::GPIO_GET, 4, 0, 0]
            )
        );
        let reply = decode_reply(
            &command,
            UsbReply {
                header: UsbHeader {
                    operation: Operation::GpioCommand,
                    reply: true,
                    request_id: 8,
                    payload_len: 8,
                    status: Status::Ok,
                },
                payload: vec![1, 0, 0, 0, 1, 0, 0, 0],
            },
        )
        .unwrap();
        assert_eq!(reply.completed, 1);
        assert_eq!(reply.data, [1]);

        let snapshot = Transaction::Gpio {
            command: rp_extio_wire::GPIO_SNAPSHOT,
            line: 0,
            value: 0,
        };
        let reply = decode_reply(
            &snapshot,
            UsbReply {
                header: UsbHeader {
                    operation: Operation::GpioCommand,
                    reply: true,
                    request_id: 9,
                    payload_len: 8,
                    status: Status::Ok,
                },
                payload: vec![17, 0, 0, 0, 0x34, 0x12, 0, 0],
            },
        )
        .unwrap();
        assert_eq!(reply.completed, 17);
        assert_eq!(reply.data, [0x34, 0x12, 0, 0]);

        let malformed = UsbReply {
            header: UsbHeader {
                operation: Operation::GpioCommand,
                reply: true,
                request_id: 10,
                payload_len: 8,
                status: Status::Ok,
            },
            payload: vec![1, 0, 0, 0, 1, 1, 0, 0],
        };
        assert!(decode_reply(&command, malformed).is_err());
    }

    #[test]
    fn rejects_missing_and_malformed_capabilities() {
        assert!(parse_capabilities(&[]).is_err());
        assert!(parse_capabilities(&[1, 0, 8, 0, 1]).is_err());

        let mut payload = Vec::new();
        payload.extend_from_slice(&CAP_GPIO.to_le_bytes());
        payload.extend_from_slice(&21_u16.to_le_bytes());
        payload.extend_from_slice(&[17, 0, 0, 0]);
        let mut pins = rp_extio_firmware_gpio_pins();
        pins[0] = 0;
        payload.extend_from_slice(&pins);
        assert!(parse_capabilities(&payload).is_err());
    }

    fn rp_extio_firmware_gpio_pins() -> [u8; 17] {
        [2, 3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 20, 22, 26, 27, 28]
    }

    #[tokio::test]
    #[ignore = "requires a connected rp-extio Pico 2"]
    async fn negotiates_connected_bridge_and_reads_gpio_snapshot() {
        let info = nusb::list_devices()
            .await
            .unwrap()
            .find(matches_bridge)
            .expect("rp-extio Pico 2 not found");
        let (failed, _failures) = mpsc::unbounded_channel();
        let (backend, config) = UsbBackend::connect(&info, failed).await.unwrap();
        assert_eq!(config.i2c_speed_hz, RegistrationConfig::PICO2.i2c_speed_hz);
        assert_eq!(config.spi_chip_selects, 2);
        assert_eq!(config.gpio_lines, 17);

        let snapshot = backend
            .transact(Transaction::Gpio {
                command: rp_extio_wire::GPIO_SNAPSHOT,
                line: 0,
                value: 0,
            })
            .await;
        assert_eq!(snapshot.status, Status::Ok as i32);
        assert_eq!(snapshot.completed, 17);
        assert_eq!(snapshot.data.len(), 4);
        backend.disconnect().await;
    }
}
