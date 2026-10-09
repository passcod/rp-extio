#![no_std]

pub const USB_MAGIC: [u8; 4] = *b"RPXB";
pub const KERNEL_MAGIC: [u8; 4] = *b"RPKD";
pub const VERSION_MAJOR: u8 = 1;
pub const VERSION_MINOR: u8 = 0;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Operation {
    Hello = 1,
    I2cTransfer = 2,
    SpiMessage = 3,
}

impl TryFrom<u8> for Operation {
    type Error = WireError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Hello),
            2 => Ok(Self::I2cTransfer),
            3 => Ok(Self::SpiMessage),
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
