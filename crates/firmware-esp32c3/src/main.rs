#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::uart::{Config as UartConfig, Uart};
use rp_extio_wire::CONTROL_SERIAL_BAUD;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_hal::main]
fn main() -> ! {
	let peripherals = esp_hal::init(esp_hal::Config::default());
	let _transport = Uart::new(
		peripherals.UART0,
		UartConfig::default().with_baudrate(CONTROL_SERIAL_BAUD),
	)
	.unwrap()
	.with_rx(peripherals.GPIO20)
	.with_tx(peripherals.GPIO21);

	loop {
		core::hint::spin_loop();
	}
}
