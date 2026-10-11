#![no_std]

use rp_extio_wire::{
    CAP_GENERAL, CAP_GPIO, CAP_I2C, CAP_PIN_CONFIG, CAP_SPI, CAP_UART, I2cRequest, MAX_PAYLOAD,
    Operation, Peripheral, PinMap, PinValidationError, PinValidationKind, SPI_CS_CHANGE,
    SpiRequest, Status, UsbHeader, WireError, Writer,
};

pub use rp_extio_wire::DEFAULT_PIN_MAP;

pub const I2C_SPEED_HZ: u32 = 100_000;
pub const SPI_MIN_SPEED_HZ: u32 = 50_000;
pub const SPI_MAX_SPEED_HZ: u32 = 20_000_000;
pub const SPI_CHIP_SELECTS: u8 = 2;
pub const UART_BAUD: u32 = 115_200;

pub const SPI_CPHA: u16 = 1 << 0;
pub const SPI_CPOL: u16 = 1 << 1;
pub const SPI_CS_HIGH: u16 = 1 << 2;
pub const SPI_LSB_FIRST: u16 = 1 << 3;
pub const SPI_NO_CS: u16 = 1 << 6;
pub const SPI_SUPPORTED_MODES: u16 = SPI_CPHA | SPI_CPOL | SPI_CS_HIGH | SPI_LSB_FIRST | SPI_NO_CS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BusFailure {
    pub status: Status,
    pub completed: u32,
}

impl BusFailure {
    pub const fn new(status: Status, completed: u32) -> Self {
        Self { status, completed }
    }
}

pub struct SpiOutcome {
    pub completed_bytes: u32,
    pub effective_speeds: [u32; rp_extio_wire::MAX_SPI_SEGMENTS],
}

impl SpiOutcome {
    pub const fn new() -> Self {
        Self {
            completed_bytes: 0,
            effective_speeds: [0; rp_extio_wire::MAX_SPI_SEGMENTS],
        }
    }
}

impl Default for SpiOutcome {
    fn default() -> Self {
        Self::new()
    }
}

pub trait Board {
    fn pin_map(&self) -> &PinMap;

    fn generation(&self) -> u64;

    fn configure_pins(&mut self, map: PinMap, generation: u64) -> Result<(), PinValidationError>;

    fn i2c_transfer(
        &mut self,
        request: &I2cRequest<'_>,
        reads: &mut [u8],
    ) -> Result<usize, BusFailure>;

    fn spi_message(
        &mut self,
        request: &SpiRequest<'_>,
        receive: &mut [u8],
    ) -> Result<SpiOutcome, BusFailure>;

    fn disconnect(&mut self) {}

    fn gpio_command(&mut self, command: u8, line: u8, value: u8) -> Result<u8, BusFailure>;

    fn gpio_snapshot(&mut self) -> Result<u32, BusFailure>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reply {
    pub header: UsbHeader,
    pub payload_len: usize,
}

pub fn dispatch<B: Board>(
    request: UsbHeader,
    payload: &[u8],
    board: &mut B,
    response: &mut [u8; MAX_PAYLOAD],
) -> Reply {
    if request.reply || request.payload_len as usize != payload.len() {
        return error_reply(request, Status::BadFrame, 0);
    }

    if request.operation == Operation::ConfigurePins {
        return configure_pins(request, payload, board, response);
    }

    let result = match request.operation {
        Operation::Hello => hello(payload, board, response),
        Operation::I2cTransfer => i2c(payload, board, response),
        Operation::SpiMessage => spi(payload, board, response),
        Operation::GpioCommand => gpio(payload, board, response),
        Operation::ConfigurePins => unreachable!(),
    };

    match result {
        Ok(payload_len) => reply(request, Status::Ok, payload_len),
        Err(failure) => {
            response[..4].copy_from_slice(&failure.completed.to_le_bytes());
            error_reply(request, failure.status, 4)
        }
    }
}

fn reply(request: UsbHeader, status: Status, payload_len: usize) -> Reply {
    Reply {
        header: UsbHeader {
            operation: request.operation,
            reply: true,
            request_id: request.request_id,
            payload_len: payload_len as u32,
            status,
        },
        payload_len,
    }
}

fn error_reply(request: UsbHeader, status: Status, payload_len: usize) -> Reply {
    reply(request, status, payload_len)
}

fn hello<B: Board>(payload: &[u8], board: &B, response: &mut [u8]) -> Result<usize, BusFailure> {
    if !payload.is_empty() {
        return Err(BusFailure::new(Status::BadFrame, 0));
    }

    let pins = board.pin_map();
    let gpio_pins = pins
        .gpio_pins()
        .map_err(|_| BusFailure::new(Status::IoError, 0))?;
    let mut out = Writer::new(response);
    record(&mut out, CAP_GENERAL, 16, |out| {
        out.push(rp_extio_wire::VERSION_MAJOR)?;
        out.push(rp_extio_wire::VERSION_MAJOR)?;
        out.push(rp_extio_wire::VERSION_MINOR)?;
        out.push(0)?;
        out.u32(MAX_PAYLOAD as u32)?;
        out.extend(&board.generation().to_le_bytes())
    })?;
    for bus in 0..2 {
        if pins.is_enabled([Peripheral::I2c0, Peripheral::I2c1][bus]) {
            record(&mut out, CAP_I2C, 16, |out| {
                out.push(bus as u8)?;
                out.push(0)?;
                out.u16(rp_extio_wire::I2C_READ | rp_extio_wire::I2C_TEN_BIT)?;
                out.u32(I2C_SPEED_HZ)?;
                out.u32(I2C_SPEED_HZ)?;
                out.push(pins.i2c_sda[bus])?;
                out.push(pins.i2c_scl[bus])?;
                out.u16(0)
            })?;
        }
        if pins.is_enabled([Peripheral::Spi0, Peripheral::Spi1][bus]) {
            record(&mut out, CAP_SPI, 17, |out| {
                out.push(bus as u8)?;
                out.push(SPI_CHIP_SELECTS)?;
                out.u16(SPI_SUPPORTED_MODES)?;
                out.u32(SPI_MIN_SPEED_HZ)?;
                out.u32(SPI_MAX_SPEED_HZ)?;
                out.push(pins.spi_sck[bus])?;
                out.push(pins.spi_mosi[bus])?;
                out.push(pins.spi_miso[bus])?;
                out.extend(&pins.spi_chip_selects[bus])
            })?;
        }
        if pins.is_enabled([Peripheral::Uart0, Peripheral::Uart1][bus]) {
            record(&mut out, CAP_UART, 12, |out| {
                out.push(bus as u8)?;
                out.push(pins.uart_tx[bus])?;
                out.push(pins.uart_rx[bus])?;
                out.push(0)?;
                out.u32(UART_BAUD)?;
                out.u32(0)
            })?;
        }
    }
    record(
        &mut out,
        CAP_PIN_CONFIG,
        rp_extio_wire::PIN_MAP_LEN as u16,
        |out| out.extend(&pins.encode()),
    )?;
    record(&mut out, CAP_GPIO, 4 + gpio_pins.len() as u16, |out| {
        out.push(gpio_pins.len() as u8)?;
        out.push(0)?;
        out.u16(0)?;
        out.extend(gpio_pins.as_slice())
    })?;
    Ok(out.len())
}

fn gpio<B: Board>(payload: &[u8], board: &mut B, response: &mut [u8]) -> Result<usize, BusFailure> {
    if payload.len() != 4 || payload[2] > 1 || payload[3] != 0 {
        return Err(BusFailure::new(Status::BadFrame, 0));
    }
    if payload[0] == rp_extio_wire::GPIO_SNAPSHOT {
        if payload[1] != 0 || payload[2] != 0 {
            return Err(BusFailure::new(Status::BadFrame, 0));
        }
        let lines = board
            .pin_map()
            .gpio_pins()
            .map_err(|_| BusFailure::new(Status::IoError, 0))?
            .len();
        response[..4].copy_from_slice(&(lines as u32).to_le_bytes());
        response[4..8].copy_from_slice(&board.gpio_snapshot()?.to_le_bytes());
        return Ok(8);
    }
    let value = board.gpio_command(payload[0], payload[1], payload[2])?;
    response[..4].copy_from_slice(&1_u32.to_le_bytes());
    response[4] = value;
    response[5..8].fill(0);
    Ok(8)
}

fn configure_pins<B: Board>(
    request: UsbHeader,
    payload: &[u8],
    board: &mut B,
    response: &mut [u8],
) -> Reply {
    let Some((generation, encoded_map)) = payload.split_first_chunk::<8>() else {
        return error_reply(request, Status::BadFrame, 0);
    };
    let Ok(map) = PinMap::decode(encoded_map) else {
        return error_reply(request, Status::BadFrame, 0);
    };
    let generation = u64::from_le_bytes(*generation);
    if let Err(error) = board.configure_pins(map, generation) {
        response[..PinValidationError::ENCODED_LEN].copy_from_slice(&error.encode());
        let status = if error.kind == PinValidationKind::DuplicatePin {
            Status::PinConflict
        } else {
            Status::InvalidRoute
        };
        return error_reply(request, status, PinValidationError::ENCODED_LEN);
    }
    response[..8].copy_from_slice(&generation.to_le_bytes());
    response[8..8 + rp_extio_wire::PIN_MAP_LEN].copy_from_slice(&map.encode());
    reply(request, Status::Ok, 8 + rp_extio_wire::PIN_MAP_LEN)
}

fn record(
    out: &mut Writer<'_>,
    kind: u16,
    len: u16,
    value: impl FnOnce(&mut Writer<'_>) -> Result<(), WireError>,
) -> Result<(), BusFailure> {
    out.u16(kind).map_err(bad_frame)?;
    out.u16(len).map_err(bad_frame)?;
    value(out).map_err(bad_frame)
}

fn i2c<B: Board>(payload: &[u8], board: &mut B, response: &mut [u8]) -> Result<usize, BusFailure> {
    let request = I2cRequest::parse(payload).map_err(bad_frame)?;
    let peripheral = match request.bus_id {
        0 => Peripheral::I2c0,
        1 => Peripheral::I2c1,
        _ => return Err(BusFailure::new(Status::Unsupported, 0)),
    };
    if !board.pin_map().is_enabled(peripheral) {
        return Err(BusFailure::new(Status::Unsupported, 0));
    }
    let read_len = board.i2c_transfer(&request, &mut response[4..])?;
    if read_len > response.len() - 4 {
        return Err(BusFailure::new(Status::IoError, 0));
    }
    response[..4].copy_from_slice(&(request.count as u32).to_le_bytes());
    Ok(4 + read_len)
}

fn spi<B: Board>(payload: &[u8], board: &mut B, response: &mut [u8]) -> Result<usize, BusFailure> {
    let request = SpiRequest::parse(payload).map_err(bad_frame)?;
    let peripheral = match request.bus_id {
        0 => Peripheral::Spi0,
        1 => Peripheral::Spi1,
        _ => return Err(BusFailure::new(Status::Unsupported, 0)),
    };
    if !board.pin_map().is_enabled(peripheral)
        || request.mode & !SPI_SUPPORTED_MODES != 0
        || (request.mode & SPI_NO_CS == 0 && request.chip_select >= SPI_CHIP_SELECTS)
    {
        return Err(BusFailure::new(Status::Unsupported, 0));
    }

    let metadata_len = 8 + request.count as usize * 4;
    let outcome = board.spi_message(&request, &mut response[metadata_len..])?;
    response[..4].copy_from_slice(&outcome.completed_bytes.to_le_bytes());
    response[4] = request.count;
    response[5..8].fill(0);
    for (index, speed) in outcome.effective_speeds[..request.count as usize]
        .iter()
        .enumerate()
    {
        let offset = 8 + index * 4;
        response[offset..offset + 4].copy_from_slice(&speed.to_le_bytes());
    }
    let rx_len = request
        .segments()
        .map(|segment| segment.map(|segment| segment.len as usize))
        .try_fold(0usize, |sum, len| len.map(|len| sum + len))
        .map_err(bad_frame)?;
    Ok(metadata_len + rx_len)
}

fn bad_frame(_: WireError) -> BusFailure {
    BusFailure::new(Status::BadFrame, 0)
}

pub fn cumulative_spi_delay_ns(request: &SpiRequest<'_>) -> Result<u64, BusFailure> {
    request
        .segments()
        .try_fold(0u64, |sum, segment| {
            let segment = segment.map_err(bad_frame)?;
            let word = u64::from(segment.word_delay_ns) * u64::from(segment.len);
            Ok(sum
                + word
                + u64::from(segment.delay_ns)
                + if segment.flags & SPI_CS_CHANGE != 0 {
                    u64::from(segment.cs_change_delay_ns)
                } else {
                    0
                })
        })
        .and_then(|delay| {
            if delay <= 2_000_000_000 {
                Ok(delay)
            } else {
                Err(BusFailure::new(Status::Unsupported, 0))
            }
        })
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    #[derive(Default)]
    struct Stub {
        gpio_values: u32,
        gpio_outputs: u32,
        pin_map: PinMap,
        generation: u64,
    }

    impl Board for Stub {
        fn pin_map(&self) -> &PinMap {
            &self.pin_map
        }

        fn generation(&self) -> u64 {
            self.generation
        }

        fn configure_pins(
            &mut self,
            map: PinMap,
            generation: u64,
        ) -> Result<(), PinValidationError> {
            map.validate()?;
            self.pin_map = map;
            self.generation = generation;
            Ok(())
        }

        fn i2c_transfer(
            &mut self,
            request: &I2cRequest<'_>,
            reads: &mut [u8],
        ) -> Result<usize, BusFailure> {
            let mut len = 0;
            for message in request.messages() {
                let message = message.map_err(bad_frame)?;
                if message.flags & rp_extio_wire::I2C_READ != 0 {
                    reads[len..len + message.len as usize].fill(0xa5);
                    len += message.len as usize;
                }
            }
            Ok(len)
        }

        fn spi_message(
            &mut self,
            request: &SpiRequest<'_>,
            receive: &mut [u8],
        ) -> Result<SpiOutcome, BusFailure> {
            let mut outcome = SpiOutcome::new();
            let mut offset = 0;
            for (index, segment) in request.segments().enumerate() {
                let segment = segment.map_err(bad_frame)?;
                receive[offset..offset + segment.len as usize].copy_from_slice(segment.tx);
                offset += segment.len as usize;
                outcome.effective_speeds[index] = segment.speed_hz;
            }
            outcome.completed_bytes = offset as u32;
            Ok(outcome)
        }

        fn gpio_command(&mut self, command: u8, line: u8, value: u8) -> Result<u8, BusFailure> {
            if line as usize >= self.pin_map.gpio_pins().unwrap().len() {
                return Err(BusFailure::new(Status::Unsupported, 0));
            }
            let mask = 1_u32 << line;
            match command {
                rp_extio_wire::GPIO_GET => Ok(u8::from(self.gpio_values & mask != 0)),
                rp_extio_wire::GPIO_SET if self.gpio_outputs & mask != 0 => {
                    self.gpio_values = (self.gpio_values & !mask) | (u32::from(value) << line);
                    Ok(value)
                }
                rp_extio_wire::GPIO_DIRECTION_INPUT => {
                    self.gpio_outputs &= !mask;
                    Ok(1)
                }
                rp_extio_wire::GPIO_DIRECTION_OUTPUT => {
                    self.gpio_outputs |= mask;
                    self.gpio_values = (self.gpio_values & !mask) | (u32::from(value) << line);
                    Ok(0)
                }
                rp_extio_wire::GPIO_GET_DIRECTION => Ok(u8::from(self.gpio_outputs & mask == 0)),
                _ => Err(BusFailure::new(Status::Unsupported, 0)),
            }
        }

        fn gpio_snapshot(&mut self) -> Result<u32, BusFailure> {
            Ok(self.gpio_values)
        }
    }

    fn header(operation: Operation, len: usize) -> UsbHeader {
        UsbHeader {
            operation,
            reply: false,
            request_id: 9,
            payload_len: len as u32,
            status: Status::Ok,
        }
    }

    #[test]
    fn hello_reports_bus_uart_and_gpio_capabilities() {
        let mut response = [0; MAX_PAYLOAD];
        let reply = dispatch(
            header(Operation::Hello, 0),
            &[],
            &mut Stub::default(),
            &mut response,
        );
        assert_eq!(reply.header.status, Status::Ok);
        assert_eq!(&response[..2], &CAP_GENERAL.to_le_bytes());
        assert!(
            response[..reply.payload_len]
                .windows(2)
                .any(|window| window == CAP_SPI.to_le_bytes())
        );
        assert!(
            response[..reply.payload_len]
                .windows(2)
                .any(|window| window == CAP_UART.to_le_bytes())
        );
        assert!(
            response[..reply.payload_len]
                .windows(2)
                .any(|window| window == CAP_GPIO.to_le_bytes())
        );
        assert!(
            response[..reply.payload_len]
                .windows(2)
                .any(|window| window == CAP_PIN_CONFIG.to_le_bytes())
        );
    }

    #[test]
    fn configures_complete_pin_map_and_reports_generation() {
        let mut board = Stub::default();
        let mut response = [0; MAX_PAYLOAD];
        let mut map = DEFAULT_PIN_MAP;
        map.i2c_sda[0] = 12;
        map.i2c_scl[0] = 13;
        map.spi_chip_selects[0] = [4, 5];
        let mut payload = [0; 8 + rp_extio_wire::PIN_MAP_LEN];
        payload[..8].copy_from_slice(&42_u64.to_le_bytes());
        payload[8..].copy_from_slice(&map.encode());

        let configured = dispatch(
            header(Operation::ConfigurePins, payload.len()),
            &payload,
            &mut board,
            &mut response,
        );
        assert_eq!(configured.header.status, Status::Ok);
        assert_eq!(&response[..configured.payload_len], &payload);
        assert_eq!(board.pin_map, map);
        assert_eq!(board.generation, 42);

        let hello = dispatch(header(Operation::Hello, 0), &[], &mut board, &mut response);
        assert_eq!(hello.header.status, Status::Ok);
        assert_eq!(&response[12..20], &42_u64.to_le_bytes());
    }

    #[test]
    fn rejected_pin_map_preserves_active_configuration() {
        let mut board = Stub::default();
        let mut response = [0; MAX_PAYLOAD];
        let mut map = DEFAULT_PIN_MAP;
        map.spi_chip_selects[0][0] = map.uart_tx[0];
        let mut payload = [0; 8 + rp_extio_wire::PIN_MAP_LEN];
        payload[..8].copy_from_slice(&7_u64.to_le_bytes());
        payload[8..].copy_from_slice(&map.encode());

        let reply = dispatch(
            header(Operation::ConfigurePins, payload.len()),
            &payload,
            &mut board,
            &mut response,
        );
        assert_eq!(reply.header.status, Status::PinConflict);
        assert_eq!(board.pin_map, DEFAULT_PIN_MAP);
        assert_eq!(board.generation, 0);
        assert_eq!(
            PinValidationError::decode(&response[..reply.payload_len])
                .unwrap()
                .kind,
            PinValidationKind::DuplicatePin
        );
    }

    #[test]
    fn i2c_dispatches_and_returns_read_bytes() {
        let mut request = [0; 12];
        request[1] = 1;
        request[4] = 0x50;
        request[6] = rp_extio_wire::I2C_READ as u8;
        request[8] = 2;
        let mut response = [0; MAX_PAYLOAD];
        let reply = dispatch(
            header(Operation::I2cTransfer, request.len()),
            &request,
            &mut Stub::default(),
            &mut response,
        );
        assert_eq!(reply.header.status, Status::Ok);
        assert_eq!(&response[..6], &[1, 0, 0, 0, 0xa5, 0xa5]);
    }

    #[test]
    fn gpio_dispatches_commands_and_snapshots() {
        let mut board = Stub::default();
        let mut response = [0; MAX_PAYLOAD];

        let reply = dispatch(
            header(Operation::GpioCommand, 4),
            &[rp_extio_wire::GPIO_DIRECTION_OUTPUT, 3, 1, 0],
            &mut board,
            &mut response,
        );
        assert_eq!(reply.header.status, Status::Ok);
        assert_eq!(&response[..8], &[1, 0, 0, 0, 0, 0, 0, 0]);

        let reply = dispatch(
            header(Operation::GpioCommand, 4),
            &[rp_extio_wire::GPIO_GET, 3, 0, 0],
            &mut board,
            &mut response,
        );
        assert_eq!(reply.header.status, Status::Ok);
        assert_eq!(&response[..8], &[1, 0, 0, 0, 1, 0, 0, 0]);

        let reply = dispatch(
            header(Operation::GpioCommand, 4),
            &[rp_extio_wire::GPIO_SNAPSHOT, 0, 0, 0],
            &mut board,
            &mut response,
        );
        assert_eq!(reply.header.status, Status::Ok);
        assert_eq!(
            &response[..4],
            &(DEFAULT_PIN_MAP.gpio_pins().unwrap().len() as u32).to_le_bytes()
        );
        assert_eq!(&response[4..8], &(1_u32 << 3).to_le_bytes());
    }

    #[test]
    fn gpio_rejects_malformed_payloads() {
        let mut response = [0; MAX_PAYLOAD];
        let mut board = Stub::default();
        for payload in [
            &[rp_extio_wire::GPIO_GET, 0, 0][..],
            &[rp_extio_wire::GPIO_GET, 0, 2, 0],
            &[rp_extio_wire::GPIO_GET, 0, 0, 1],
            &[rp_extio_wire::GPIO_SNAPSHOT, 1, 0, 0],
        ] {
            let reply = dispatch(
                header(Operation::GpioCommand, payload.len()),
                payload,
                &mut board,
                &mut response,
            );
            assert_eq!(reply.header.status, Status::BadFrame);
        }
    }
}
