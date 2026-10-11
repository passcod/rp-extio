#![no_std]
#![no_main]

mod pico2;

use core::panic::PanicInfo;

use embassy_futures::{join::join, yield_now};
use embassy_rp::{bind_interrupts, gpio::Flex, peripherals, usb};
use embassy_usb::class::cdc_acm::{CdcAcmClass, Receiver, Sender, State as CdcState};
use embassy_usb::driver::Driver as UsbDriver;
use embassy_usb::driver::{EndpointError, EndpointIn, EndpointOut};
use embassy_usb::{Builder, Config};
use pico2::Pico2Board;
use rp_extio_firmware::{Board, dispatch};
use rp_extio_wire::{MAX_PAYLOAD, Operation, Status, USB_HEADER_LEN, UsbHeader, WireError};
use static_cell::StaticCell;

include!(concat!(env!("OUT_DIR"), "/pin_config.rs"));

const USB_MAX_PACKET: usize = 64;
const USB_FRAME_LEN: usize = USB_HEADER_LEN + MAX_PAYLOAD;
const DEVELOPMENT_VID: u16 = 0xcafe;
const DEVELOPMENT_PID: u16 = 0x4010;

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => usb::InterruptHandler<peripherals::USB>;
});

struct UsbBuffers {
    config_descriptor: [u8; 512],
    bos_descriptor: [u8; 64],
    msos_descriptor: [u8; 0],
    control: [u8; 64],
}

impl UsbBuffers {
    const fn new() -> Self {
        Self {
            config_descriptor: [0; 512],
            bos_descriptor: [0; 64],
            msos_descriptor: [],
            control: [0; 64],
        }
    }
}

struct BridgeBuffers {
    receive: [u8; USB_FRAME_LEN],
    transmit: [u8; USB_FRAME_LEN],
    packet: [u8; USB_MAX_PACKET],
}

impl BridgeBuffers {
    const fn new() -> Self {
        Self {
            receive: [0; USB_FRAME_LEN],
            transmit: [0; USB_FRAME_LEN],
            packet: [0; USB_MAX_PACKET],
        }
    }
}

static USB_BUFFERS: StaticCell<UsbBuffers> = StaticCell::new();
static BRIDGE_BUFFERS: StaticCell<BridgeBuffers> = StaticCell::new();
static SERIAL_BYTES: StaticCell<[u8; 16]> = StaticCell::new();
static CDC_STATES: [StaticCell<CdcState<'static>>; 2] = [StaticCell::new(), StaticCell::new()];

struct RpTimerDriver;

impl embassy_time_driver::Driver for RpTimerDriver {
    fn now(&self) -> u64 {
        loop {
            let high = embassy_rp::pac::TIMER0.timerawh().read();
            let low = embassy_rp::pac::TIMER0.timerawl().read();
            if high == embassy_rp::pac::TIMER0.timerawh().read() {
                return (u64::from(high) << 32) | u64::from(low);
            }
        }
    }

    fn schedule_wake(&self, _at: u64, waker: &core::task::Waker) {
        // No firmware task currently awaits an Embassy timer. Keep the fallback correct by
        // repolling if a dependency uses one; TIMER0-backed `now` determines expiry.
        waker.wake_by_ref();
    }
}

embassy_time_driver::time_driver_impl!(static TIME_DRIVER: RpTimerDriver = RpTimerDriver);

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    // A startup panic happens before our USB device can enumerate. Return to BOOTSEL so a bad
    // firmware image remains recoverable without needing physical access to the BOOTSEL button.
    embassy_rp::rom_data::reset_to_usb_boot(0, 0);
    loop {
        core::hint::spin_loop();
    }
}

#[embassy_executor::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let peripherals = embassy_rp::init(Default::default());
    let serial_bytes = SERIAL_BYTES.init([0; 16]);
    encode_serial(embassy_rp::otp::get_chipid().unwrap_or(0), serial_bytes);
    let serial = unsafe { core::str::from_utf8_unchecked(serial_bytes) };

    let driver = usb::Driver::new(peripherals.USB, Irqs);
    let mut config = Config::new(DEVELOPMENT_VID, DEVELOPMENT_PID);
    config.manufacturer = Some("rp-extio");
    config.product = Some("Pico 2 external buses");
    config.serial_number = Some(serial);
    config.device_class = 0xef;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x01;
    config.composite_with_iads = true;

    let usb_buffers = USB_BUFFERS.init(UsbBuffers::new());
    let mut builder = Builder::new(
        driver,
        config,
        &mut usb_buffers.config_descriptor,
        &mut usb_buffers.bos_descriptor,
        &mut usb_buffers.msos_descriptor,
        &mut usb_buffers.control,
    );
    let (out_endpoint, in_endpoint) = {
        let mut function = builder.function(0xff, 0, 0);
        let mut interface = function.interface();
        let mut alternate = interface.alt_setting(0xff, 0, 0, None);
        let out_endpoint = alternate.endpoint_bulk_out(None, USB_MAX_PACKET as u16);
        let in_endpoint = alternate.endpoint_bulk_in(None, USB_MAX_PACKET as u16);
        (out_endpoint, in_endpoint)
    };
    let cdc0 = CdcAcmClass::new(
        &mut builder,
        CDC_STATES[0].init(CdcState::new()),
        USB_MAX_PACKET as u16,
    );
    let cdc1 = CdcAcmClass::new(
        &mut builder,
        CDC_STATES[1].init(CdcState::new()),
        USB_MAX_PACKET as u16,
    );
    let mut usb = builder.build();

    let (cdc0_tx, cdc0_rx) = cdc0.split();
    let (cdc1_tx, cdc1_rx) = cdc1.split();

    let mut board = Pico2Board::new(
        [
            Flex::new(peripherals.PIN_0),
            Flex::new(peripherals.PIN_1),
            Flex::new(peripherals.PIN_2),
            Flex::new(peripherals.PIN_3),
            Flex::new(peripherals.PIN_4),
            Flex::new(peripherals.PIN_5),
            Flex::new(peripherals.PIN_6),
            Flex::new(peripherals.PIN_7),
            Flex::new(peripherals.PIN_8),
            Flex::new(peripherals.PIN_9),
            Flex::new(peripherals.PIN_10),
            Flex::new(peripherals.PIN_11),
            Flex::new(peripherals.PIN_12),
            Flex::new(peripherals.PIN_13),
            Flex::new(peripherals.PIN_14),
            Flex::new(peripherals.PIN_15),
            Flex::new(peripherals.PIN_16),
            Flex::new(peripherals.PIN_17),
            Flex::new(peripherals.PIN_18),
            Flex::new(peripherals.PIN_19),
            Flex::new(peripherals.PIN_20),
            Flex::new(peripherals.PIN_21),
            Flex::new(peripherals.PIN_22),
            Flex::new(peripherals.PIN_26),
            Flex::new(peripherals.PIN_27),
            Flex::new(peripherals.PIN_28),
        ],
        peripherals.SPI0,
        peripherals.SPI1,
        peripherals.UART0,
        peripherals.UART1,
        PIN_MAP,
    );
    let bridge_buffers = BRIDGE_BUFFERS.init(BridgeBuffers::new());
    join(
        usb.run(),
        join(
            bridge(out_endpoint, in_endpoint, &mut board, bridge_buffers),
            join(
                join(cdc_to_uart(0, cdc0_rx), uart_to_cdc(0, cdc0_tx)),
                join(cdc_to_uart(1, cdc1_rx), uart_to_cdc(1, cdc1_tx)),
            ),
        ),
    )
    .await;
    unreachable!()
}

async fn cdc_to_uart<'d, D: UsbDriver<'d>>(bus: usize, mut cdc: Receiver<'d, D>) -> ! {
    let mut packet = [0; USB_MAX_PACKET];
    loop {
        cdc.wait_connection().await;
        while let Ok(length) = cdc.read_packet(&mut packet).await {
            for &byte in &packet[..length] {
                while Pico2Board::uart_enabled(bus) && Pico2Board::uart_tx_full(bus) {
                    yield_now().await;
                }
                if Pico2Board::uart_enabled(bus) {
                    Pico2Board::uart_write(bus, byte);
                }
            }
        }
    }
}

async fn uart_to_cdc<'d, D: UsbDriver<'d>>(bus: usize, mut cdc: Sender<'d, D>) -> ! {
    let mut packet = [0; USB_MAX_PACKET];
    loop {
        cdc.wait_connection().await;
        loop {
            let mut length = Pico2Board::uart_read(bus, &mut packet);
            while length == 0 {
                yield_now().await;
                length = Pico2Board::uart_read(bus, &mut packet);
            }
            if cdc.write_packet(&packet[..length]).await.is_err() {
                break;
            }
            if length == USB_MAX_PACKET && cdc.write_packet(&[]).await.is_err() {
                break;
            }
        }
    }
}

fn encode_serial(id: u64, output: &mut [u8; 16]) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for (index, byte) in output.iter_mut().enumerate() {
        let shift = (15 - index) * 4;
        *byte = HEX[((id >> shift) & 0xf) as usize];
    }
}

async fn bridge<OUT, IN, B>(
    mut out_endpoint: OUT,
    mut in_endpoint: IN,
    board: &mut B,
    buffers: &mut BridgeBuffers,
) -> !
where
    OUT: EndpointOut,
    IN: EndpointIn,
    B: Board,
{
    loop {
        out_endpoint.wait_enabled().await;
        let mut received = 0;
        let mut expected = None;

        loop {
            let count = match out_endpoint.read(&mut buffers.packet).await {
                Ok(count) => count,
                Err(EndpointError::Disabled) => break,
                Err(EndpointError::BufferOverflow) => {
                    if send_bad_frame(&mut in_endpoint, &mut buffers.transmit, 0, Status::BadFrame)
                        .await
                        .is_err()
                    {
                        break;
                    }
                    received = 0;
                    expected = None;
                    continue;
                }
            };

            if received + count > buffers.receive.len() {
                if send_bad_frame(&mut in_endpoint, &mut buffers.transmit, 0, Status::BadFrame)
                    .await
                    .is_err()
                {
                    break;
                }
                received = 0;
                expected = None;
                continue;
            }
            buffers.receive[received..received + count].copy_from_slice(&buffers.packet[..count]);
            received += count;

            if expected.is_none() && received >= USB_HEADER_LEN {
                match UsbHeader::decode(&buffers.receive[..USB_HEADER_LEN]) {
                    Ok(header) => expected = Some(USB_HEADER_LEN + header.payload_len as usize),
                    Err(error) => {
                        let status = if error == WireError::Unsupported {
                            Status::Unsupported
                        } else {
                            Status::BadFrame
                        };
                        if send_bad_frame(
                            &mut in_endpoint,
                            &mut buffers.transmit,
                            raw_request_id(&buffers.receive[..USB_HEADER_LEN]),
                            status,
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                        received = 0;
                        expected = None;
                        continue;
                    }
                }
            }

            let Some(frame_len) = expected else { continue };
            if received > frame_len {
                if send_bad_frame(
                    &mut in_endpoint,
                    &mut buffers.transmit,
                    raw_request_id(&buffers.receive[..USB_HEADER_LEN]),
                    Status::BadFrame,
                )
                .await
                .is_err()
                {
                    break;
                }
                received = 0;
                expected = None;
                continue;
            }
            if received != frame_len {
                continue;
            }

            let request = UsbHeader::decode(&buffers.receive[..USB_HEADER_LEN]).unwrap();
            let response_payload: &mut [u8; MAX_PAYLOAD] = (&mut buffers.transmit
                [USB_HEADER_LEN..])
                .try_into()
                .unwrap();
            let reply = dispatch(
                request,
                &buffers.receive[USB_HEADER_LEN..frame_len],
                board,
                response_payload,
            );
            buffers.transmit[..USB_HEADER_LEN].copy_from_slice(&reply.header.encode());
            if write_frame(
                &mut in_endpoint,
                &buffers.transmit[..USB_HEADER_LEN + reply.payload_len],
            )
            .await
            .is_err()
            {
                break;
            }
            received = 0;
            expected = None;
        }
        board.disconnect();
    }
}

fn raw_request_id(header: &[u8]) -> u64 {
    u64::from_le_bytes(header[8..16].try_into().unwrap())
}

async fn send_bad_frame<IN: EndpointIn>(
    endpoint: &mut IN,
    transmit: &mut [u8; USB_FRAME_LEN],
    request_id: u64,
    status: Status,
) -> Result<(), EndpointError> {
    let header = UsbHeader {
        operation: Operation::Hello,
        reply: true,
        request_id,
        payload_len: 0,
        status,
    };
    transmit[..USB_HEADER_LEN].copy_from_slice(&header.encode());
    write_frame(endpoint, &transmit[..USB_HEADER_LEN]).await
}

async fn write_frame<IN: EndpointIn>(endpoint: &mut IN, frame: &[u8]) -> Result<(), EndpointError> {
    for packet in frame.chunks(USB_MAX_PACKET) {
        endpoint.write(packet).await?;
    }
    Ok(())
}
