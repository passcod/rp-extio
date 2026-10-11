#![no_std]

pub const USB_MAGIC: [u8; 4] = *b"RPXB";
pub const KERNEL_MAGIC: [u8; 4] = *b"RPKD";
pub const VERSION_MAJOR: u8 = 1;
pub const VERSION_MINOR: u8 = 3;
pub const USB_HEADER_LEN: usize = 24;
pub const KERNEL_HEADER_LEN: usize = 32;
pub const MAX_PAYLOAD: usize = 4096;
pub const MAX_DATA: usize = 3584;
pub const MAX_I2C_MESSAGES: usize = 16;
pub const MAX_SPI_SEGMENTS: usize = 16;
pub const USB_REPLY: u8 = 1;
pub const CAP_GENERAL: u16 = 1;
pub const CAP_I2C: u16 = 2;
pub const CAP_SPI: u16 = 3;
pub const CAP_UART: u16 = 4;
pub const CAP_GPIO: u16 = 5;
pub const GPIO_GET: u8 = 1;
pub const GPIO_SET: u8 = 2;
pub const GPIO_DIRECTION_INPUT: u8 = 3;
pub const GPIO_DIRECTION_OUTPUT: u8 = 4;
pub const GPIO_GET_DIRECTION: u8 = 5;
pub const GPIO_SNAPSHOT: u8 = 6;
pub const HEADER_PINS: [u8; 26] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 26, 27, 28,
];
pub const GPIO_LINE_COUNT: usize = 17;
pub const PIN_MAP_LEN: usize = 9;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Operation {
    Hello = 1,
    I2cTransfer = 2,
    SpiMessage = 3,
    GpioCommand = 4,
    ConfigurePins = 5,
}

impl TryFrom<u8> for Operation {
    type Error = WireError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Hello),
            2 => Ok(Self::I2cTransfer),
            3 => Ok(Self::SpiMessage),
            4 => Ok(Self::GpioCommand),
            5 => Ok(Self::ConfigurePins),
            _ => Err(WireError::Unsupported),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Status {
    Ok = 0,
    Nack = 1,
    ArbitrationLost = 2,
    Timeout = 3,
    Disconnected = 4,
    Unsupported = 5,
    BadFrame = 6,
    IoError = 7,
    InvalidRoute = 8,
    PinConflict = 9,
}

impl TryFrom<i32> for Status {
    type Error = WireError;
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Ok),
            1 => Ok(Self::Nack),
            2 => Ok(Self::ArbitrationLost),
            3 => Ok(Self::Timeout),
            4 => Ok(Self::Disconnected),
            5 => Ok(Self::Unsupported),
            6 => Ok(Self::BadFrame),
            7 => Ok(Self::IoError),
            8 => Ok(Self::InvalidRoute),
            9 => Ok(Self::PinConflict),
            _ => Err(WireError::Invalid),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    Truncated,
    Invalid,
    Unsupported,
    TooLarge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PinSignal {
    UartTx = 0,
    UartRx = 1,
    I2cSda = 2,
    I2cScl = 3,
    SpiMiso = 4,
    SpiSck = 5,
    SpiMosi = 6,
    SpiCs0 = 7,
    SpiCs1 = 8,
}

impl PinSignal {
    pub const ALL: [Self; PIN_MAP_LEN] = [
        Self::UartTx,
        Self::UartRx,
        Self::I2cSda,
        Self::I2cScl,
        Self::SpiMiso,
        Self::SpiSck,
        Self::SpiMosi,
        Self::SpiCs0,
        Self::SpiCs1,
    ];

    pub const fn valid_pins(self) -> &'static [u8] {
        match self {
            Self::UartTx => &[0, 2, 12, 14, 16, 18, 28],
            Self::UartRx => &[1, 3, 13, 15, 17, 19],
            Self::I2cSda => &[0, 4, 8, 12, 16, 20],
            Self::I2cScl => &[1, 5, 9, 13, 17, 21],
            Self::SpiMiso => &[0, 4, 16, 20],
            Self::SpiSck => &[2, 6, 18, 22],
            Self::SpiMosi => &[3, 7, 19],
            Self::SpiCs0 | Self::SpiCs1 => &HEADER_PINS,
        }
    }
}

impl TryFrom<u8> for PinSignal {
    type Error = WireError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::ALL
            .get(value as usize)
            .copied()
            .ok_or(WireError::Invalid)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PinValidationKind {
    UnavailablePin = 1,
    InvalidRoute = 2,
    MismatchedPair = 3,
    DuplicatePin = 4,
}

impl TryFrom<u8> for PinValidationKind {
    type Error = WireError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::UnavailablePin),
            2 => Ok(Self::InvalidRoute),
            3 => Ok(Self::MismatchedPair),
            4 => Ok(Self::DuplicatePin),
            _ => Err(WireError::Invalid),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PinValidationError {
    pub kind: PinValidationKind,
    pub signal: PinSignal,
    pub other: Option<PinSignal>,
    pub pin: u8,
}

impl PinValidationError {
    pub const ENCODED_LEN: usize = 4;

    pub const fn encode(self) -> [u8; Self::ENCODED_LEN] {
        [
            self.kind as u8,
            self.signal as u8,
            match self.other {
                Some(signal) => signal as u8,
                None => u8::MAX,
            },
            self.pin,
        ]
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.len() != Self::ENCODED_LEN {
            return Err(WireError::Truncated);
        }
        Ok(Self {
            kind: PinValidationKind::try_from(bytes[0])?,
            signal: PinSignal::try_from(bytes[1])?,
            other: if bytes[2] == u8::MAX {
                None
            } else {
                Some(PinSignal::try_from(bytes[2])?)
            },
            pin: bytes[3],
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PinMap {
    pub uart_tx: u8,
    pub uart_rx: u8,
    pub i2c_sda: u8,
    pub i2c_scl: u8,
    pub spi_miso: u8,
    pub spi_sck: u8,
    pub spi_mosi: u8,
    pub spi_chip_selects: [u8; 2],
}

pub const DEFAULT_PIN_MAP: PinMap = PinMap {
    uart_tx: 0,
    uart_rx: 1,
    i2c_sda: 4,
    i2c_scl: 5,
    spi_miso: 16,
    spi_sck: 18,
    spi_mosi: 19,
    spi_chip_selects: [17, 21],
};

impl PinMap {
    pub const fn encode(self) -> [u8; PIN_MAP_LEN] {
        [
            self.uart_tx,
            self.uart_rx,
            self.i2c_sda,
            self.i2c_scl,
            self.spi_miso,
            self.spi_sck,
            self.spi_mosi,
            self.spi_chip_selects[0],
            self.spi_chip_selects[1],
        ]
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.len() != PIN_MAP_LEN {
            return Err(WireError::Truncated);
        }
        Ok(Self {
            uart_tx: bytes[0],
            uart_rx: bytes[1],
            i2c_sda: bytes[2],
            i2c_scl: bytes[3],
            spi_miso: bytes[4],
            spi_sck: bytes[5],
            spi_mosi: bytes[6],
            spi_chip_selects: [bytes[7], bytes[8]],
        })
    }

    pub const fn pin(self, signal: PinSignal) -> u8 {
        self.encode()[signal as usize]
    }

    pub fn set_pin(&mut self, signal: PinSignal, pin: u8) {
        match signal {
            PinSignal::UartTx => self.uart_tx = pin,
            PinSignal::UartRx => self.uart_rx = pin,
            PinSignal::I2cSda => self.i2c_sda = pin,
            PinSignal::I2cScl => self.i2c_scl = pin,
            PinSignal::SpiMiso => self.spi_miso = pin,
            PinSignal::SpiSck => self.spi_sck = pin,
            PinSignal::SpiMosi => self.spi_mosi = pin,
            PinSignal::SpiCs0 => self.spi_chip_selects[0] = pin,
            PinSignal::SpiCs1 => self.spi_chip_selects[1] = pin,
        }
    }

    pub fn validate(self) -> Result<(), PinValidationError> {
        for signal in PinSignal::ALL {
            let pin = self.pin(signal);
            if !HEADER_PINS.contains(&pin) {
                return Err(PinValidationError {
                    kind: PinValidationKind::UnavailablePin,
                    signal,
                    other: None,
                    pin,
                });
            }
            if !signal.valid_pins().contains(&pin) {
                return Err(PinValidationError {
                    kind: PinValidationKind::InvalidRoute,
                    signal,
                    other: None,
                    pin,
                });
            }
        }
        if self.i2c_scl != self.i2c_sda + 1 {
            return Err(PinValidationError {
                kind: PinValidationKind::MismatchedPair,
                signal: PinSignal::I2cScl,
                other: Some(PinSignal::I2cSda),
                pin: self.i2c_scl,
            });
        }
        for (index, signal) in PinSignal::ALL.iter().copied().enumerate() {
            let pin = self.pin(signal);
            if let Some(other) = PinSignal::ALL[..index]
                .iter()
                .copied()
                .find(|&other| self.pin(other) == pin)
            {
                return Err(PinValidationError {
                    kind: PinValidationKind::DuplicatePin,
                    signal,
                    other: Some(other),
                    pin,
                });
            }
        }
        Ok(())
    }

    pub fn gpio_pins(self) -> Result<[u8; GPIO_LINE_COUNT], PinValidationError> {
        self.validate()?;
        let assigned = self.encode();
        let mut gpio = [0; GPIO_LINE_COUNT];
        let mut output = 0;
        for pin in HEADER_PINS {
            if !assigned.contains(&pin) {
                gpio[output] = pin;
                output += 1;
            }
        }
        Ok(gpio)
    }
}

fn read_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}
fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}
fn read_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes[..8].try_into().unwrap())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsbHeader {
    pub operation: Operation,
    pub reply: bool,
    pub request_id: u64,
    pub payload_len: u32,
    pub status: Status,
}

impl UsbHeader {
    pub fn encode(self) -> [u8; USB_HEADER_LEN] {
        let mut out = [0; USB_HEADER_LEN];
        out[..4].copy_from_slice(&USB_MAGIC);
        out[4] = VERSION_MAJOR;
        out[5] = VERSION_MINOR;
        out[6] = self.operation as u8;
        out[7] = u8::from(self.reply);
        out[8..16].copy_from_slice(&self.request_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.payload_len.to_le_bytes());
        out[20..24].copy_from_slice(&(self.status as i32).to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.len() != USB_HEADER_LEN {
            return Err(WireError::Truncated);
        }
        if bytes[..4] != USB_MAGIC || bytes[4] != VERSION_MAJOR || bytes[7] & !USB_REPLY != 0 {
            return Err(WireError::Invalid);
        }
        let payload_len = read_u32(&bytes[16..20]);
        if payload_len as usize > MAX_PAYLOAD {
            return Err(WireError::TooLarge);
        }
        let status = Status::try_from(i32::from_le_bytes(bytes[20..24].try_into().unwrap()))?;
        let reply = bytes[7] & USB_REPLY != 0;
        if !reply && status != Status::Ok {
            return Err(WireError::Invalid);
        }
        Ok(Self {
            operation: Operation::try_from(bytes[6])?,
            reply,
            request_id: read_u64(&bytes[8..16]),
            payload_len,
            status,
        })
    }
}

/// Writes a complete payload into caller-owned storage without allocation.
pub struct Writer<'a> {
    bytes: &'a mut [u8],
    len: usize,
}

impl<'a> Writer<'a> {
    pub fn new(bytes: &'a mut [u8]) -> Self {
        Self { bytes, len: 0 }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn written(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
    pub fn push(&mut self, value: u8) -> Result<(), WireError> {
        self.extend(&[value])
    }
    pub fn u16(&mut self, value: u16) -> Result<(), WireError> {
        self.extend(&value.to_le_bytes())
    }
    pub fn u32(&mut self, value: u32) -> Result<(), WireError> {
        self.extend(&value.to_le_bytes())
    }
    pub fn i32(&mut self, value: i32) -> Result<(), WireError> {
        self.extend(&value.to_le_bytes())
    }
    pub fn extend(&mut self, value: &[u8]) -> Result<(), WireError> {
        if value.len() > self.bytes.len().saturating_sub(self.len) {
            return Err(WireError::TooLarge);
        }
        self.bytes[self.len..self.len + value.len()].copy_from_slice(value);
        self.len += value.len();
        Ok(())
    }
}

pub const I2C_READ: u16 = 1;
pub const I2C_TEN_BIT: u16 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct I2cMessage<'a> {
    pub address: u16,
    pub flags: u16,
    pub len: u16,
    pub write_data: &'a [u8],
}

pub struct I2cRequest<'a> {
    pub bus_id: u8,
    pub count: u8,
    payload: &'a [u8],
}

impl<'a> I2cRequest<'a> {
    pub fn parse(payload: &'a [u8]) -> Result<Self, WireError> {
        if payload.len() < 4 {
            return Err(WireError::Truncated);
        }
        let count = payload[1] as usize;
        if count == 0 || count > MAX_I2C_MESSAGES || read_u16(&payload[2..4]) != 0 {
            return Err(WireError::Invalid);
        }
        let request = Self {
            bus_id: payload[0],
            count: count as u8,
            payload,
        };
        let mut iter = request.messages();
        while iter.next().transpose()?.is_some() {}
        if iter.remaining != 0
            || iter.offset != payload.len()
            || iter.read_len > MAX_DATA
            || iter.write_len > MAX_DATA
        {
            return Err(WireError::Invalid);
        }
        Ok(request)
    }

    pub fn messages(&self) -> I2cMessages<'a> {
        I2cMessages {
            payload: self.payload,
            remaining: self.count,
            offset: 4,
            read_len: 0,
            write_len: 0,
        }
    }
}

pub struct I2cMessages<'a> {
    payload: &'a [u8],
    remaining: u8,
    offset: usize,
    read_len: usize,
    write_len: usize,
}

impl<'a> Iterator for I2cMessages<'a> {
    type Item = Result<I2cMessage<'a>, WireError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let tail = self.payload.get(self.offset..)?;
        if tail.len() < 8 {
            return Some(Err(WireError::Truncated));
        }
        let address = read_u16(tail);
        let flags = read_u16(&tail[2..4]);
        let len = read_u16(&tail[4..6]);
        if read_u16(&tail[6..8]) != 0
            || flags & !(I2C_READ | I2C_TEN_BIT) != 0
            || address
                > if flags & I2C_TEN_BIT != 0 {
                    0x3ff
                } else {
                    0x7f
                }
        {
            return Some(Err(WireError::Invalid));
        }
        self.offset += 8;
        let data = if flags & I2C_READ != 0 {
            &[][..]
        } else {
            let Some(data) = self.payload.get(self.offset..self.offset + len as usize) else {
                return Some(Err(WireError::Truncated));
            };
            self.offset += len as usize;
            data
        };
        if flags & I2C_READ != 0 {
            self.read_len += len as usize;
        } else {
            self.write_len += len as usize;
        }
        Some(Ok(I2cMessage {
            address,
            flags,
            len,
            write_data: data,
        }))
    }
}

pub const SPI_CS_CHANGE: u16 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpiSegment<'a> {
    pub speed_hz: u32,
    pub len: u16,
    pub flags: u16,
    pub word_delay_ns: u32,
    pub delay_ns: u32,
    pub cs_change_delay_ns: u32,
    pub tx: &'a [u8],
}

pub struct SpiRequest<'a> {
    pub bus_id: u8,
    pub chip_select: u8,
    pub mode: u16,
    pub count: u8,
    payload: &'a [u8],
}

impl<'a> SpiRequest<'a> {
    pub fn parse(payload: &'a [u8]) -> Result<Self, WireError> {
        if payload.len() < 8 {
            return Err(WireError::Truncated);
        }
        let count = payload[4] as usize;
        if count == 0 || count > MAX_SPI_SEGMENTS || payload[5..8] != [0; 3] {
            return Err(WireError::Invalid);
        }
        let request = Self {
            bus_id: payload[0],
            chip_select: payload[1],
            mode: read_u16(&payload[2..4]),
            count: count as u8,
            payload,
        };
        let mut iter = request.segments();
        while iter.next().transpose()?.is_some() {}
        if iter.remaining != 0 || iter.offset != payload.len() || iter.data_len > MAX_DATA {
            return Err(WireError::Invalid);
        }
        Ok(request)
    }

    pub fn segments(&self) -> SpiSegments<'a> {
        SpiSegments {
            payload: self.payload,
            remaining: self.count,
            offset: 8,
            data_len: 0,
        }
    }
}

pub struct SpiSegments<'a> {
    payload: &'a [u8],
    remaining: u8,
    offset: usize,
    data_len: usize,
}

impl<'a> Iterator for SpiSegments<'a> {
    type Item = Result<SpiSegment<'a>, WireError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let tail = self.payload.get(self.offset..)?;
        if tail.len() < 20 {
            return Some(Err(WireError::Truncated));
        }
        let speed_hz = read_u32(tail);
        let len = read_u16(&tail[4..6]);
        let flags = read_u16(&tail[6..8]);
        if flags & !SPI_CS_CHANGE != 0 || speed_hz == 0 {
            return Some(Err(WireError::Invalid));
        }
        let word_delay_ns = read_u32(&tail[8..12]);
        let delay_ns = read_u32(&tail[12..16]);
        let cs_change_delay_ns = read_u32(&tail[16..20]);
        self.offset += 20;
        let Some(tx) = self.payload.get(self.offset..self.offset + len as usize) else {
            return Some(Err(WireError::Truncated));
        };
        self.offset += len as usize;
        self.data_len += len as usize;
        Some(Ok(SpiSegment {
            speed_hz,
            len,
            flags,
            word_delay_ns,
            delay_ns,
            cs_change_delay_ns,
            tx,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trip_and_rejects_invalid_status() {
        let header = UsbHeader {
            operation: Operation::I2cTransfer,
            reply: false,
            request_id: 72,
            payload_len: 12,
            status: Status::Ok,
        };
        assert_eq!(UsbHeader::decode(&header.encode()), Ok(header));
        let mut malformed = header.encode();
        malformed[20] = Status::Nack as u8;
        assert_eq!(UsbHeader::decode(&malformed), Err(WireError::Invalid));
    }

    #[test]
    fn pin_maps_validate_routes_conflicts_and_gpio_derivation() {
        assert_eq!(DEFAULT_PIN_MAP.validate(), Ok(()));
        assert_eq!(
            DEFAULT_PIN_MAP.gpio_pins().unwrap(),
            [2, 3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 20, 22, 26, 27, 28]
        );

        let mut alternate = DEFAULT_PIN_MAP;
        alternate.i2c_sda = 12;
        alternate.i2c_scl = 13;
        alternate.spi_chip_selects = [4, 5];
        assert_eq!(PinMap::decode(&alternate.encode()), Ok(alternate));
        assert_eq!(alternate.validate(), Ok(()));

        let mut invalid = alternate;
        invalid.spi_sck = 3;
        assert_eq!(
            invalid.validate().unwrap_err(),
            PinValidationError {
                kind: PinValidationKind::InvalidRoute,
                signal: PinSignal::SpiSck,
                other: None,
                pin: 3,
            }
        );

        let mut duplicate = alternate;
        duplicate.spi_chip_selects[0] = duplicate.uart_tx;
        assert_eq!(
            duplicate.validate().unwrap_err(),
            PinValidationError {
                kind: PinValidationKind::DuplicatePin,
                signal: PinSignal::SpiCs0,
                other: Some(PinSignal::UartTx),
                pin: duplicate.uart_tx,
            }
        );
    }

    #[test]
    fn pin_validation_errors_round_trip() {
        let error = PinValidationError {
            kind: PinValidationKind::MismatchedPair,
            signal: PinSignal::I2cScl,
            other: Some(PinSignal::I2cSda),
            pin: 9,
        };
        assert_eq!(PinValidationError::decode(&error.encode()), Ok(error));
    }

    #[test]
    fn i2c_rejects_short_write_and_trailing_data() {
        let mut bytes = [0; 13];
        bytes[1] = 1;
        bytes[4] = 0x50;
        bytes[8] = 2;
        bytes[12] = 0xaa;
        assert!(matches!(
            I2cRequest::parse(&bytes),
            Err(WireError::Truncated)
        ));
        bytes[8] = 1;
        assert_eq!(
            I2cRequest::parse(&bytes)
                .unwrap()
                .messages()
                .next()
                .unwrap()
                .unwrap()
                .write_data,
            &[0xaa]
        );
    }

    #[test]
    fn spi_segment_bounds() {
        let mut bytes = [0; 29];
        bytes[4] = 1;
        bytes[8..12].copy_from_slice(&1_000_000u32.to_le_bytes());
        bytes[12] = 1;
        bytes[28] = 0x5a;
        assert_eq!(
            SpiRequest::parse(&bytes)
                .unwrap()
                .segments()
                .next()
                .unwrap()
                .unwrap()
                .tx,
            &[0x5a]
        );
        bytes[12] = 2;
        assert!(SpiRequest::parse(&bytes).is_err());
    }
}
