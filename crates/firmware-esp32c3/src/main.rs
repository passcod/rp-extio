#![no_std]
#![no_main]

mod board;

use board::Esp32C3Board;
use esp_backtrace as _;
use esp_hal::{
	efuse,
	gpio::Pin,
	uart::{Config as UartConfig, Uart},
};
use rp_extio_firmware::dispatch;
use rp_extio_wire::{
	CONTROL_SERIAL_BAUD, DeviceIdentity, MAX_PAYLOAD, MAX_SERIAL_BODY, MAX_SERIAL_FRAME,
	USB_HEADER_LEN, UsbHeader, decode_serial_frame, encode_serial_frame,
};
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

struct Buffers {
	candidate: [u8; MAX_SERIAL_FRAME],
	body: [u8; MAX_SERIAL_BODY],
	response: [u8; MAX_PAYLOAD],
}

static BUFFERS: StaticCell<Buffers> = StaticCell::new();

#[esp_hal::main]
fn main() -> ! {
	let peripherals = esp_hal::init(esp_hal::Config::default());
	let mut transport = Uart::new(
		peripherals.UART0,
		UartConfig::default().with_baudrate(CONTROL_SERIAL_BAUD),
	)
	.unwrap()
	.with_rx(peripherals.GPIO20)
	.with_tx(peripherals.GPIO21);
	let identity = device_identity();
	let adc0 = unsafe { peripherals.GPIO0.clone_unchecked() };
	let adc1 = unsafe { peripherals.GPIO1.clone_unchecked() };
	let adc2 = unsafe { peripherals.GPIO2.clone_unchecked() };
	let adc3 = unsafe { peripherals.GPIO3.clone_unchecked() };
	let adc4 = unsafe { peripherals.GPIO4.clone_unchecked() };
	let mut board = Esp32C3Board::new(
		[
			peripherals.GPIO0.degrade(),
			peripherals.GPIO1.degrade(),
			peripherals.GPIO2.degrade(),
			peripherals.GPIO3.degrade(),
			peripherals.GPIO4.degrade(),
			peripherals.GPIO5.degrade(),
			peripherals.GPIO6.degrade(),
			peripherals.GPIO7.degrade(),
			peripherals.GPIO8.degrade(),
			peripherals.GPIO9.degrade(),
			peripherals.GPIO10.degrade(),
			peripherals.GPIO18.degrade(),
			peripherals.GPIO19.degrade(),
		],
		peripherals.I2C0,
		peripherals.SPI2,
		peripherals.UART1,
		peripherals.ADC1,
		(adc0, adc1, adc2, adc3, adc4),
		peripherals.LEDC,
		peripherals.RMT,
		identity,
	);
	let buffers = BUFFERS.init(Buffers {
		candidate: [0; MAX_SERIAL_FRAME],
		body: [0; MAX_SERIAL_BODY],
		response: [0; MAX_PAYLOAD],
	});
	let mut candidate_len = 0;
	let mut input = [0; 64];

	loop {
		let Ok(count) = transport.read(&mut input) else {
			candidate_len = 0;
			continue;
		};
		for &byte in &input[..count] {
			if byte != 0 {
				if candidate_len < buffers.candidate.len() {
					buffers.candidate[candidate_len] = byte;
					candidate_len += 1;
				} else {
					candidate_len = 0;
				}
				continue;
			}
			if candidate_len == 0 {
				continue;
			}
			let encoded_len = candidate_len;
			candidate_len = 0;
			let Ok(body_len) =
				decode_serial_frame(&buffers.candidate[..encoded_len], &mut buffers.body)
			else {
				continue;
			};
			if body_len < USB_HEADER_LEN {
				continue;
			}
			let Ok(header) = UsbHeader::decode(&buffers.body[..USB_HEADER_LEN]) else {
				continue;
			};
			if body_len != USB_HEADER_LEN + header.payload_len as usize {
				continue;
			}
			let reply = dispatch(
				header,
				&buffers.body[USB_HEADER_LEN..body_len],
				&mut board,
				&mut buffers.response,
			);
			buffers.body[..USB_HEADER_LEN].copy_from_slice(&reply.header.encode());
			buffers.body[USB_HEADER_LEN..USB_HEADER_LEN + reply.payload_len]
				.copy_from_slice(&buffers.response[..reply.payload_len]);
			let Ok(frame_len) = encode_serial_frame(
				&buffers.body[..USB_HEADER_LEN + reply.payload_len],
				&mut buffers.candidate,
			) else {
				continue;
			};
			write_all(&mut transport, &buffers.candidate[..frame_len]);
		}
	}
}

fn write_all(transport: &mut Uart<'_, esp_hal::Blocking>, mut bytes: &[u8]) {
	while !bytes.is_empty() {
		match transport.write(bytes) {
			Ok(written) => bytes = &bytes[written..],
			Err(_) => return,
		}
	}
	let _ = transport.flush();
}

fn device_identity() -> DeviceIdentity {
	const HEX: &[u8; 16] = b"0123456789ABCDEF";
	let mac = efuse::base_mac_address();
	let mut encoded = [0; 12];
	for (index, &byte) in mac.as_bytes().iter().enumerate() {
		encoded[index * 2] = HEX[(byte >> 4) as usize];
		encoded[index * 2 + 1] = HEX[(byte & 0x0f) as usize];
	}
	DeviceIdentity::new(&encoded).unwrap()
}
