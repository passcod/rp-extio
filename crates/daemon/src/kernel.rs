use crate::backend::{Backend, BackendReply, I2cMessage, SpiSegment, Transaction};
use rp_extio_wire::{
    I2cRequest, KERNEL_HEADER_LEN, KERNEL_MAGIC, MAX_PAYLOAD, Operation, SpiRequest, Status,
};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, RawFd},
        unix::fs::OpenOptionsExt,
    },
    path::Path,
    sync::{Arc, RwLock},
};

const ABI_MAJOR: u16 = 2;
const IOC_MAGIC: u32 = b'R' as u32;
const I2C_FUNCTIONALITY: u32 = 0x0eff_000b;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegistrationConfig {
    pub i2c: [Option<I2cRegistration>; 2],
    pub spi: [Option<SpiRegistration>; 2],
    pub gpio_lines: u16,
    pub gpio_pins: [u8; 26],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct I2cRegistration {
    pub speed_hz: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpiRegistration {
    pub spi_mode_bits: u32,
    pub spi_min_speed_hz: u32,
    pub spi_max_speed_hz: u32,
    pub spi_chip_selects: u16,
}

impl RegistrationConfig {
    pub const PICO2: Self = Self {
        i2c: [Some(I2cRegistration { speed_hz: 100_000 }), None],
        spi: [
            Some(SpiRegistration {
                spi_mode_bits: 0x4f,
                spi_min_speed_hz: 50_000,
                spi_max_speed_hz: 20_000_000,
                spi_chip_selects: 2,
            }),
            None,
        ],
        gpio_lines: 17,
        gpio_pins: [
            2, 3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 20, 22, 26, 27, 28, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
    };
}

#[repr(C)]
#[derive(Default)]
struct IocVersion {
    major: u16,
    minor: u16,
    max_payload: u32,
    features: u64,
}

#[repr(C)]
struct IocCreateI2c {
    functionality: u32,
    speed_hz: u32,
    bus_id: u8,
    reserved: [u8; 3],
    instance_id: u32,
    bus_num: i32,
}

#[repr(C)]
struct IocCreateSpi {
    mode_bits: u32,
    min_speed_hz: u32,
    max_speed_hz: u32,
    chip_selects: u16,
    bus_id: u8,
    reserved: u8,
    instance_id: u32,
    bus_num: i32,
}

#[repr(C)]
struct IocDestroy {
    instance_id: u32,
    reserved: u32,
}

#[repr(C)]
struct IocCreateGpio {
    lines: u16,
    reserved: u16,
    instance_id: u32,
    features: u32,
    reserved2: u32,
    pins: [u8; 26],
    reserved3: [u8; 6],
}

const _: () = assert!(size_of::<IocVersion>() == 16);
const _: () = assert!(size_of::<IocCreateI2c>() == 20);
const _: () = assert!(size_of::<IocCreateSpi>() == 24);
const _: () = assert!(size_of::<IocDestroy>() == 8);
const _: () = assert!(size_of::<IocCreateGpio>() == 48);

const IOC_GET_VERSION: libc::Ioctl = libc::_IOR::<IocVersion>(IOC_MAGIC, 0x00);
const IOC_CREATE_I2C: libc::Ioctl = libc::_IOWR::<IocCreateI2c>(IOC_MAGIC, 0x01);
const IOC_CREATE_SPI: libc::Ioctl = libc::_IOWR::<IocCreateSpi>(IOC_MAGIC, 0x02);
const IOC_DESTROY: libc::Ioctl = libc::_IOW::<IocDestroy>(IOC_MAGIC, 0x03);
const IOC_CREATE_GPIO: libc::Ioctl = libc::_IOWR::<IocCreateGpio>(IOC_MAGIC, 0x04);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BusNumbers {
    pub i2c: [Option<i32>; 2],
    pub spi: [Option<i32>; 2],
}

pub struct DeviceRegistration {
    kernel: Arc<Kernel>,
    instance_ids: Vec<u32>,
    gpio_instance_id: u32,
    pub buses: BusNumbers,
}

impl DeviceRegistration {
    pub fn gpio_instance_id(&self) -> u32 {
        self.gpio_instance_id
    }

    pub fn destroy(self) -> io::Result<()> {
        let mut first_error = None;
        for id in self.instance_ids {
            if let Err(error) = self.kernel.destroy_instance(id) {
                first_error.get_or_insert(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

pub struct Kernel {
    file: tokio::io::unix::AsyncFd<File>,
    backends: RwLock<HashMap<u32, Arc<dyn Backend>>>,
}

impl Kernel {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Arc<Self>> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;
        let mut version = IocVersion::default();
        ioctl(file.as_raw_fd(), IOC_GET_VERSION, &mut version)?;
        if version.major != ABI_MAJOR || version.max_payload < MAX_PAYLOAD as u32 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "unsupported kernel ABI {}.{} with {}-byte payloads",
                    version.major, version.minor, version.max_payload
                ),
            ));
        }
        Ok(Arc::new(Self {
            file: tokio::io::unix::AsyncFd::new(file)?,
            backends: RwLock::new(HashMap::new()),
        }))
    }

    pub fn register_device(
        self: &Arc<Self>,
        backend: Arc<dyn Backend>,
        config: RegistrationConfig,
    ) -> io::Result<DeviceRegistration> {
        let mut instance_ids = Vec::new();
        let mut buses = BusNumbers {
            i2c: [None; 2],
            spi: [None; 2],
        };
        for (bus_id, registration) in config.i2c.into_iter().enumerate() {
            let Some(registration) = registration else {
                continue;
            };
            let mut create = IocCreateI2c {
                functionality: I2C_FUNCTIONALITY,
                speed_hz: registration.speed_hz,
                bus_id: bus_id as u8,
                reserved: [0; 3],
                instance_id: 0,
                bus_num: -1,
            };
            if let Err(error) = ioctl(self.file.get_ref().as_raw_fd(), IOC_CREATE_I2C, &mut create)
            {
                self.destroy_instances(&instance_ids);
                return Err(error);
            }
            instance_ids.push(create.instance_id);
            buses.i2c[bus_id] = Some(create.bus_num);
            self.backends
                .write()
                .unwrap()
                .insert(create.instance_id, backend.clone());
        }
        for (bus_id, registration) in config.spi.into_iter().enumerate() {
            let Some(registration) = registration else {
                continue;
            };
            let mut create = IocCreateSpi {
                mode_bits: registration.spi_mode_bits,
                min_speed_hz: registration.spi_min_speed_hz,
                max_speed_hz: registration.spi_max_speed_hz,
                chip_selects: registration.spi_chip_selects,
                bus_id: bus_id as u8,
                reserved: 0,
                instance_id: 0,
                bus_num: -1,
            };
            if let Err(error) = ioctl(self.file.get_ref().as_raw_fd(), IOC_CREATE_SPI, &mut create)
            {
                self.destroy_instances(&instance_ids);
                return Err(error);
            }
            instance_ids.push(create.instance_id);
            buses.spi[bus_id] = Some(create.bus_num);
            self.backends
                .write()
                .unwrap()
                .insert(create.instance_id, backend.clone());
        }
        let mut gpio = IocCreateGpio {
            lines: config.gpio_lines,
            reserved: 0,
            instance_id: 0,
            features: 0,
            reserved2: 0,
            pins: config.gpio_pins,
            reserved3: [0; 6],
        };
        if let Err(error) = ioctl(self.file.get_ref().as_raw_fd(), IOC_CREATE_GPIO, &mut gpio) {
            self.destroy_instances(&instance_ids);
            return Err(error);
        }
        self.backends
            .write()
            .unwrap()
            .insert(gpio.instance_id, backend);
        instance_ids.push(gpio.instance_id);
        Ok(DeviceRegistration {
            kernel: self.clone(),
            instance_ids,
            gpio_instance_id: gpio.instance_id,
            buses,
        })
    }

    pub async fn run(self: Arc<Self>) -> io::Result<()> {
        let mut transactions = tokio::task::JoinSet::new();
        let mut frame = vec![0_u8; KERNEL_HEADER_LEN + MAX_PAYLOAD];
        loop {
            tokio::select! {
                ready = self.file.readable() => {
                    let mut ready = ready?;
                    let length = match ready.try_io(|file| {
                        read_frame(file.get_ref().as_raw_fd(), &mut frame)
                    }) {
                        Ok(result) => result?,
                        Err(_) => continue,
                    };
                    let request = Request::decode(&frame[..length])?;
                    let backend = self.backends.read().unwrap().get(&request.instance_id).cloned();
                    let kernel = self.clone();
                    transactions.spawn(async move {
                        let reply = match backend {
                            Some(backend) => backend.transact(request.transaction.clone()).await,
                            None => BackendReply::error(Status::Disconnected),
                        };
                        write_frame(kernel.file.get_ref().as_raw_fd(), &request.encode_reply(reply))
                    });
                }
                result = transactions.join_next(), if !transactions.is_empty() => {
                    result.expect("transaction set is not empty").map_err(join_error)??;
                }
            }
        }
    }

    fn destroy_instance(&self, instance_id: u32) -> io::Result<()> {
        let mut destroy = IocDestroy {
            instance_id,
            reserved: 0,
        };
        let result = ioctl(self.file.get_ref().as_raw_fd(), IOC_DESTROY, &mut destroy);
        self.backends.write().unwrap().remove(&instance_id);
        result
    }

    fn destroy_instances(&self, instance_ids: &[u32]) {
        for &instance_id in instance_ids.iter().rev() {
            let _ = self.destroy_instance(instance_id);
        }
    }

    pub fn gpio_event(
        &self,
        instance_id: u32,
        line: u8,
        old_value: u8,
        new_value: u8,
    ) -> io::Result<()> {
        let mut frame = vec![0_u8; KERNEL_HEADER_LEN + 4];
        frame[..4].copy_from_slice(&KERNEL_MAGIC);
        frame[4..6].copy_from_slice(&ABI_MAJOR.to_le_bytes());
        frame[6..8].copy_from_slice(&5_u16.to_le_bytes());
        frame[8..12].copy_from_slice(&instance_id.to_le_bytes());
        frame[12..16].copy_from_slice(&4_u32.to_le_bytes());
        frame[KERNEL_HEADER_LEN..].copy_from_slice(&[line, old_value, new_value, 0]);
        write_frame(self.file.get_ref().as_raw_fd(), &frame)
    }
}

fn ioctl<T>(fd: RawFd, command: libc::Ioctl, value: &mut T) -> io::Result<()> {
    // SAFETY: `value` has the exact repr(C) layout encoded in `command` and remains valid for
    // the duration of the ioctl. The kernel copies the fixed-size structure synchronously.
    if unsafe { libc::ioctl(fd, command, value) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn read_frame(fd: RawFd, buffer: &mut [u8]) -> io::Result<usize> {
    // SAFETY: `buffer` is writable for its full reported length and the syscall does not retain it.
    let length = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
    if length < 0 {
        Err(io::Error::last_os_error())
    } else if length == 0 {
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "kernel control device closed",
        ))
    } else {
        Ok(length as usize)
    }
}

fn write_frame(fd: RawFd, frame: &[u8]) -> io::Result<()> {
    // One write is required by the kernel ABI; a partial write cannot be resumed as another frame.
    // SAFETY: `frame` is readable for its full length and the syscall does not retain it.
    let length = unsafe { libc::write(fd, frame.as_ptr().cast(), frame.len()) };
    if length < 0 {
        Err(io::Error::last_os_error())
    } else if length as usize != frame.len() {
        Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "short write to kernel control device",
        ))
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Request {
    operation: Operation,
    instance_id: u32,
    request_id: u64,
    transaction: Transaction,
}

impl Request {
    fn decode(frame: &[u8]) -> io::Result<Self> {
        if frame.len() < KERNEL_HEADER_LEN {
            return invalid("truncated kernel frame");
        }
        if frame[..4] != KERNEL_MAGIC
            || u16::from_le_bytes(frame[4..6].try_into().unwrap()) != ABI_MAJOR
            || i32::from_le_bytes(frame[24..28].try_into().unwrap()) != 0
            || u32::from_le_bytes(frame[28..32].try_into().unwrap()) != 0
        {
            return invalid("invalid kernel frame header");
        }
        let payload_len = u32::from_le_bytes(frame[12..16].try_into().unwrap()) as usize;
        if payload_len > MAX_PAYLOAD || frame.len() != KERNEL_HEADER_LEN + payload_len {
            return invalid("invalid kernel frame length");
        }
        let operation = match u16::from_le_bytes(frame[6..8].try_into().unwrap()) {
            2 => Operation::I2cTransfer,
            3 => Operation::SpiMessage,
            4 => Operation::GpioCommand,
            _ => return invalid("unsupported kernel operation"),
        };
        let payload = &frame[KERNEL_HEADER_LEN..];
        let transaction = match operation {
            Operation::I2cTransfer => {
                let request = I2cRequest::parse(payload).map_err(wire_error)?;
                let messages = request
                    .messages()
                    .map(|message| {
                        let message = message.map_err(wire_error)?;
                        Ok(I2cMessage {
                            address: message.address,
                            flags: message.flags,
                            data: message.write_data.to_vec(),
                            read_len: if message.flags & rp_extio_wire::I2C_READ != 0 {
                                message.len
                            } else {
                                0
                            },
                        })
                    })
                    .collect::<io::Result<Vec<_>>>()?;
                Transaction::I2c {
                    bus_id: request.bus_id,
                    messages,
                }
            }
            Operation::SpiMessage => {
                let request = SpiRequest::parse(payload).map_err(wire_error)?;
                let segments = request
                    .segments()
                    .map(|segment| {
                        let segment = segment.map_err(wire_error)?;
                        Ok(SpiSegment {
                            speed_hz: segment.speed_hz,
                            flags: segment.flags,
                            word_delay_ns: segment.word_delay_ns,
                            delay_ns: segment.delay_ns,
                            cs_change_delay_ns: segment.cs_change_delay_ns,
                            tx: segment.tx.to_vec(),
                        })
                    })
                    .collect::<io::Result<Vec<_>>>()?;
                Transaction::Spi {
                    bus_id: request.bus_id,
                    chip_select: request.chip_select,
                    mode: request.mode,
                    segments,
                }
            }
            Operation::GpioCommand => {
                if payload.len() != 4
                    || payload[0] < rp_extio_wire::GPIO_GET
                    || payload[0] > rp_extio_wire::GPIO_GET_DIRECTION
                    || payload[1] >= rp_extio_wire::MAX_GPIO_LINES as u8
                    || payload[2] > 1
                    || payload[3] != 0
                {
                    return invalid("invalid GPIO command payload");
                }
                Transaction::Gpio {
                    command: payload[0],
                    line: payload[1],
                    value: payload[2],
                }
            }
            Operation::Hello | Operation::ConfigurePins | Operation::AdcRead | Operation::Pwm => {
                unreachable!()
            }
        };
        Ok(Self {
            operation,
            instance_id: u32::from_le_bytes(frame[8..12].try_into().unwrap()),
            request_id: u64::from_le_bytes(frame[16..24].try_into().unwrap()),
            transaction,
        })
    }

    fn encode_reply(&self, reply: BackendReply) -> Vec<u8> {
        let status = Status::try_from(reply.status).unwrap_or(Status::BadFrame);
        let payload = if status == Status::Ok {
            self.success_payload(&reply)
                .unwrap_or_else(|| 0_u32.to_le_bytes().to_vec())
        } else {
            reply.completed.to_le_bytes().to_vec()
        };
        let status = if status == Status::Ok && self.success_payload(&reply).is_none() {
            Status::BadFrame
        } else {
            status
        };
        let mut frame = vec![0_u8; KERNEL_HEADER_LEN + payload.len()];
        frame[..4].copy_from_slice(&KERNEL_MAGIC);
        frame[4..6].copy_from_slice(&ABI_MAJOR.to_le_bytes());
        frame[6..8].copy_from_slice(&(self.operation as u8 as u16).to_le_bytes());
        frame[8..12].copy_from_slice(&self.instance_id.to_le_bytes());
        frame[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        frame[16..24].copy_from_slice(&self.request_id.to_le_bytes());
        frame[24..28].copy_from_slice(&(status as i32).to_le_bytes());
        frame[KERNEL_HEADER_LEN..].copy_from_slice(&payload);
        frame
    }

    fn success_payload(&self, reply: &BackendReply) -> Option<Vec<u8>> {
        match &self.transaction {
            Transaction::I2c { messages, .. } => {
                let read_len: usize = messages
                    .iter()
                    .map(|message| message.read_len as usize)
                    .sum();
                if reply.completed != messages.len() as u32
                    || reply.data.len() != read_len
                    || !reply.effective_speeds.is_empty()
                {
                    return None;
                }
                let mut payload = reply.completed.to_le_bytes().to_vec();
                payload.extend_from_slice(&reply.data);
                Some(payload)
            }
            Transaction::Spi { segments, .. } => {
                let data_len: usize = segments.iter().map(|segment| segment.tx.len()).sum();
                if reply.completed != data_len as u32
                    || reply.data.len() != data_len
                    || reply.effective_speeds.len() != segments.len()
                {
                    return None;
                }
                let mut payload = Vec::with_capacity(8 + segments.len() * 4 + data_len);
                payload.extend_from_slice(&reply.completed.to_le_bytes());
                payload.push(segments.len() as u8);
                payload.extend_from_slice(&[0; 3]);
                for speed in &reply.effective_speeds {
                    payload.extend_from_slice(&speed.to_le_bytes());
                }
                payload.extend_from_slice(&reply.data);
                Some(payload)
            }
            Transaction::Gpio { .. } => {
                if reply.completed != 1
                    || reply.data.len() != 1
                    || reply.data[0] > 1
                    || !reply.effective_speeds.is_empty()
                {
                    return None;
                }
                let mut payload = reply.completed.to_le_bytes().to_vec();
                payload.push(reply.data[0]);
                payload.extend_from_slice(&[0; 3]);
                Some(payload)
            }
        }
    }
}

fn wire_error(error: rp_extio_wire::WireError) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid bus payload: {error:?}"),
    )
}

fn invalid<T>(message: &'static str) -> io::Result<T> {
    Err(io::Error::new(io::ErrorKind::InvalidData, message))
}

fn join_error(error: tokio::task::JoinError) -> io::Error {
    io::Error::other(format!("kernel dispatcher task failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(operation: u16, payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![0; KERNEL_HEADER_LEN + payload.len()];
        frame[..4].copy_from_slice(&KERNEL_MAGIC);
        frame[4..6].copy_from_slice(&ABI_MAJOR.to_le_bytes());
        frame[6..8].copy_from_slice(&operation.to_le_bytes());
        frame[8..12].copy_from_slice(&9_u32.to_le_bytes());
        frame[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        frame[16..24].copy_from_slice(&17_u64.to_le_bytes());
        frame[KERNEL_HEADER_LEN..].copy_from_slice(payload);
        frame
    }

    #[test]
    fn decodes_i2c_request_and_encodes_reply() {
        let payload = [
            1, 2, 0, 0, 0x50, 0, 0, 0, 1, 0, 0, 0, 0xaa, 0x51, 0, 1, 0, 2, 0, 0, 0,
        ];
        let request = Request::decode(&frame(2, &payload)).unwrap();
        assert_eq!(
            request.transaction,
            Transaction::I2c {
                bus_id: 1,
                messages: vec![
                    I2cMessage {
                        address: 0x50,
                        flags: 0,
                        data: vec![0xaa],
                        read_len: 0
                    },
                    I2cMessage {
                        address: 0x51,
                        flags: 1,
                        data: vec![],
                        read_len: 2
                    },
                ],
            }
        );
        let reply = request.encode_reply(BackendReply {
            status: 0,
            completed: 2,
            data: vec![3, 4],
            effective_speeds: vec![],
        });
        assert_eq!(&reply[24..28], &0_i32.to_le_bytes());
        assert_eq!(&reply[KERNEL_HEADER_LEN..], &[2, 0, 0, 0, 3, 4]);
    }

    #[test]
    fn decodes_spi_request_and_encodes_reply() {
        let payload = [
            1, 1, 3, 0, 1, 0, 0, 0, 0x40, 0x42, 0x0f, 0, 2, 0, 1, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0,
            0, 0, 0xaa, 0xbb,
        ];
        let request = Request::decode(&frame(3, &payload)).unwrap();
        assert!(matches!(
            request.transaction,
            Transaction::Spi { bus_id: 1, .. }
        ));
        let reply = request.encode_reply(BackendReply {
            status: 0,
            completed: 2,
            data: vec![0x12, 0x34],
            effective_speeds: vec![1_000_000],
        });
        assert_eq!(
            &reply[KERNEL_HEADER_LEN..],
            &[2, 0, 0, 0, 1, 0, 0, 0, 0x40, 0x42, 0x0f, 0, 0x12, 0x34]
        );
    }

    #[test]
    fn decodes_gpio_request_and_encodes_reply() {
        let request = Request::decode(&frame(
            Operation::GpioCommand as u16,
            &[rp_extio_wire::GPIO_DIRECTION_OUTPUT, 3, 1, 0],
        ))
        .unwrap();
        assert_eq!(
            request.transaction,
            Transaction::Gpio {
                command: rp_extio_wire::GPIO_DIRECTION_OUTPUT,
                line: 3,
                value: 1,
            }
        );
        let reply = request.encode_reply(BackendReply {
            status: Status::Ok as i32,
            completed: 1,
            data: vec![0],
            effective_speeds: vec![],
        });
        assert_eq!(&reply[24..28], &0_i32.to_le_bytes());
        assert_eq!(&reply[KERNEL_HEADER_LEN..], &[1, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn rejects_malformed_frames_and_backend_replies() {
        let mut bad_header = frame(2, &[0; 4]);
        bad_header[28] = 1;
        assert!(Request::decode(&bad_header).is_err());
        let bad_length = frame(2, &[0; 4]);
        assert!(Request::decode(&bad_length[..KERNEL_HEADER_LEN + 3]).is_err());

        let payload = [0, 1, 0, 0, 0x50, 0, 1, 0, 1, 0, 0, 0];
        let request = Request::decode(&frame(2, &payload)).unwrap();
        let reply = request.encode_reply(BackendReply {
            status: 0,
            completed: 1,
            data: vec![],
            effective_speeds: vec![],
        });
        assert_eq!(
            i32::from_le_bytes(reply[24..28].try_into().unwrap()),
            Status::BadFrame as i32
        );

        for payload in [
            &[rp_extio_wire::GPIO_GET, 0, 0][..],
            &[rp_extio_wire::GPIO_GET, 26, 0, 0],
            &[rp_extio_wire::GPIO_SNAPSHOT, 0, 0, 0],
            &[rp_extio_wire::GPIO_GET, 0, 2, 0],
            &[rp_extio_wire::GPIO_GET, 0, 0, 1],
        ] {
            assert!(Request::decode(&frame(Operation::GpioCommand as u16, payload)).is_err());
        }
    }
}
