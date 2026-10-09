#![no_std]

use rp_extio_wire::{
    CAP_GENERAL, CAP_I2C, CAP_SPI, I2cRequest, MAX_PAYLOAD, Operation, SPI_CS_CHANGE,
    SpiRequest, Status, UsbHeader, WireError, Writer,
};

pub const I2C_BUS_ID: u8 = 0;
pub const SPI_BUS_ID: u8 = 0;
pub const I2C_SPEED_HZ: u32 = 100_000;
pub const SPI_MIN_SPEED_HZ: u32 = 50_000;
pub const SPI_MAX_SPEED_HZ: u32 = 20_000_000;
pub const SPI_CHIP_SELECTS: u8 = 2;

pub const SPI_CPHA: u16 = 1 << 0;
pub const SPI_CPOL: u16 = 1 << 1;
pub const SPI_CS_HIGH: u16 = 1 << 2;
pub const SPI_LSB_FIRST: u16 = 1 << 3;
pub const SPI_NO_CS: u16 = 1 << 4;
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

    let result = match request.operation {
        Operation::Hello => hello(payload, response),
        Operation::I2cTransfer => i2c(payload, board, response),
        Operation::SpiMessage => spi(payload, board, response),
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

fn hello(payload: &[u8], response: &mut [u8]) -> Result<usize, BusFailure> {
    if !payload.is_empty() {
        return Err(BusFailure::new(Status::BadFrame, 0));
    }

    let mut out = Writer::new(response);
    record(&mut out, CAP_GENERAL, 8, |out| {
        out.push(rp_extio_wire::VERSION_MAJOR)?;
        out.push(rp_extio_wire::VERSION_MAJOR)?;
        out.push(rp_extio_wire::VERSION_MINOR)?;
        out.push(0)?;
        out.u32(MAX_PAYLOAD as u32)
    })?;
    record(&mut out, CAP_I2C, 16, |out| {
        out.push(I2C_BUS_ID)?;
        out.push(0)?;
        out.u16(rp_extio_wire::I2C_READ | rp_extio_wire::I2C_TEN_BIT)?;
        out.u32(I2C_SPEED_HZ)?;
        out.u32(I2C_SPEED_HZ)?;
        out.u32(0)
    })?;
    record(&mut out, CAP_SPI, 12, |out| {
        out.push(SPI_BUS_ID)?;
        out.push(SPI_CHIP_SELECTS)?;
        out.u16(SPI_SUPPORTED_MODES)?;
        out.u32(SPI_MIN_SPEED_HZ)?;
        out.u32(SPI_MAX_SPEED_HZ)
    })?;
    Ok(out.len())
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

fn i2c<B: Board>(
    payload: &[u8],
    board: &mut B,
    response: &mut [u8],
) -> Result<usize, BusFailure> {
    let request = I2cRequest::parse(payload).map_err(bad_frame)?;
    if request.bus_id != I2C_BUS_ID {
        return Err(BusFailure::new(Status::Unsupported, 0));
    }
    let read_len = board.i2c_transfer(&request, &mut response[4..])?;
    if read_len > response.len() - 4 {
        return Err(BusFailure::new(Status::IoError, 0));
    }
    response[..4].copy_from_slice(&(request.count as u32).to_le_bytes());
    Ok(4 + read_len)
}

fn spi<B: Board>(
    payload: &[u8],
    board: &mut B,
    response: &mut [u8],
) -> Result<usize, BusFailure> {
    let request = SpiRequest::parse(payload).map_err(bad_frame)?;
    if request.bus_id != SPI_BUS_ID
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
    for (index, speed) in outcome.effective_speeds[..request.count as usize].iter().enumerate() {
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

    struct Stub;

    impl Board for Stub {
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
    fn hello_reports_both_buses() {
        let mut response = [0; MAX_PAYLOAD];
        let reply = dispatch(header(Operation::Hello, 0), &[], &mut Stub, &mut response);
        assert_eq!(reply.header.status, Status::Ok);
        assert_eq!(&response[..2], &CAP_GENERAL.to_le_bytes());
        assert!(response[..reply.payload_len]
            .windows(2)
            .any(|window| window == CAP_SPI.to_le_bytes()));
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
            &mut Stub,
            &mut response,
        );
        assert_eq!(reply.header.status, Status::Ok);
        assert_eq!(&response[..6], &[1, 0, 0, 0, 0xa5, 0xa5]);
    }
}
