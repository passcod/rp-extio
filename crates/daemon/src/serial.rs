use crate::{
	backend::{Backend, BackendReply, Transaction},
	usb::{
		ConfigureError, DeviceKey, NegotiatedConfig, UsbReply, decode_reply, encode_transaction,
		parse_capabilities,
	},
};
use async_trait::async_trait;
use rp_extio_wire::{
	CONTROL_SERIAL_BAUD, MAX_SERIAL_BODY, MAX_SERIAL_FRAME, Operation, PIN_MAP_LEN, PinMap,
	PinValidationError, Status, TransportKind, USB_HEADER_LEN, UsbHeader, decode_serial_frame,
	encode_serial_frame,
};
use std::{
	io,
	path::Path,
	sync::{
		Arc, OnceLock,
		atomic::{AtomicBool, AtomicU64, Ordering},
	},
	time::Duration,
};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	sync::{Mutex, mpsc},
};
use tokio_serial::{SerialPort, SerialPortBuilderExt, SerialStream};

const SERIAL_DEADLINE: Duration = Duration::from_secs(6);
const SERIAL_CONNECT_DEADLINE: Duration = Duration::from_secs(2);
const SERIAL_BOOT_SETTLE: Duration = Duration::from_millis(300);

pub struct SerialBackend {
	port: Mutex<SerialStream>,
	next_request_id: AtomicU64,
	connected: AtomicBool,
	board_kind: OnceLock<rp_extio_wire::BoardKind>,
	device_key: DeviceKey,
	failed: mpsc::UnboundedSender<DeviceKey>,
}

impl SerialBackend {
	pub(crate) async fn connect(
		path: &Path,
		device_key: DeviceKey,
		failed: mpsc::UnboundedSender<DeviceKey>,
	) -> io::Result<(Arc<Self>, NegotiatedConfig)> {
		let mut port =
			tokio_serial::new(path.to_string_lossy(), CONTROL_SERIAL_BAUD).open_native_async()?;
		port.set_exclusive(true)?;
		// Linux asserts DTR while opening a TTY. On ESP development boards DTR and
		// RTS feed the automatic boot/reset circuit, so leave both inactive while
		// the daemon owns the control transport.
		port.write_data_terminal_ready(false)?;
		port.write_request_to_send(false)?;
		// Opening common ESP development-board bridges can pulse the automatic-reset
		// circuit. Let the application regain UART0 before sending the one-shot HELLO.
		tokio::time::sleep(SERIAL_BOOT_SETTLE).await;
		let backend = Arc::new(Self {
			port: Mutex::new(port),
			next_request_id: AtomicU64::new(2),
			connected: AtomicBool::new(true),
			board_kind: OnceLock::new(),
			device_key,
			failed,
		});
		let hello = tokio::time::timeout(
			SERIAL_CONNECT_DEADLINE,
			backend.exchange(Operation::Hello, 1, &[]),
		)
		.await
		.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "serial HELLO timed out"))??;
		if hello.header.status != Status::Ok {
			return Err(io::Error::other(format!(
				"HELLO failed with {:?}",
				hello.header.status
			)));
		}
		let config = parse_capabilities(&hello.payload)?;
		if config.transport != TransportKind::Serial {
			return Err(invalid("serial device reported a non-serial transport"));
		}
		backend
			.board_kind
			.set(config.board_kind)
			.map_err(|_| invalid("board profile was already set"))?;
		Ok((backend, config))
	}

	pub async fn configure_pins(
		&self,
		map: PinMap,
		generation: u64,
	) -> Result<NegotiatedConfig, ConfigureError> {
		let kind =
			self.board_kind.get().copied().ok_or_else(|| {
				ConfigureError::Transport(invalid("board profile is unavailable"))
			})?;
		map.validate_for(kind.profile())
			.map_err(ConfigureError::Rejected)?;
		let mut payload = Vec::with_capacity(8 + PIN_MAP_LEN);
		payload.extend_from_slice(&generation.to_le_bytes());
		payload.extend_from_slice(&map.encode());
		let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
		let reply = self
			.exchange(Operation::ConfigurePins, request_id, &payload)
			.await
			.map_err(ConfigureError::Transport)?;
		let applied = match reply.header.status {
			Status::Ok
				if reply.payload.len() == 8 + PIN_MAP_LEN
					&& reply.payload[..8] == generation.to_le_bytes() =>
			{
				PinMap::decode(&reply.payload[8..])
					.map_err(|_| ConfigureError::Transport(invalid("invalid configured pin map")))?
			}
			Status::InvalidRoute | Status::PinConflict => {
				return Err(ConfigureError::Rejected(
					PinValidationError::decode(&reply.payload)
						.map_err(|_| ConfigureError::Transport(invalid("invalid pin rejection")))?,
				));
			}
			_ => {
				return Err(ConfigureError::Transport(io::Error::other(format!(
					"configure-pins failed with {:?}",
					reply.header.status
				))));
			}
		};
		let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
		let hello = self
			.exchange(Operation::Hello, request_id, &[])
			.await
			.map_err(ConfigureError::Transport)?;
		let config = parse_capabilities(&hello.payload).map_err(ConfigureError::Transport)?;
		if hello.header.status != Status::Ok
			|| config.pin_map != applied
			|| config.generation != generation
			|| config.board_kind != kind
		{
			return Err(ConfigureError::Transport(invalid(
				"firmware did not retain configured pin map",
			)));
		}
		Ok(config)
	}

	pub(crate) fn fail(&self) {
		if self.connected.swap(false, Ordering::AcqRel) {
			let _ = self.failed.send(self.device_key.clone());
		}
	}

	async fn exchange(
		&self,
		operation: Operation,
		request_id: u64,
		payload: &[u8],
	) -> io::Result<UsbReply> {
		let header = UsbHeader {
			operation,
			reply: false,
			request_id,
			payload_len: payload.len() as u32,
			status: Status::Ok,
		};
		let mut body = Vec::with_capacity(USB_HEADER_LEN + payload.len());
		body.extend_from_slice(&header.encode());
		body.extend_from_slice(payload);
		let mut port = self.port.lock().await;
		tokio::time::timeout(
			SERIAL_DEADLINE,
			serial_transfer(&mut port, operation, request_id, &body),
		)
		.await
		.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "serial transaction timed out"))?
	}
}

#[async_trait]
impl Backend for SerialBackend {
	async fn transact(&self, transaction: Transaction) -> BackendReply {
		if !self.connected.load(Ordering::Acquire) {
			return BackendReply::error(Status::Disconnected);
		}
		let (operation, payload) = match encode_transaction(&transaction) {
			Ok(value) => value,
			Err(_) => return BackendReply::error(Status::BadFrame),
		};
		let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
		match self.exchange(operation, request_id, &payload).await {
			Ok(reply) => match decode_reply(&transaction, reply) {
				Ok(reply) => reply,
				Err(_) => {
					self.fail();
					BackendReply::error(Status::BadFrame)
				}
			},
			Err(_) => {
				self.fail();
				BackendReply::error(Status::Disconnected)
			}
		}
	}

	async fn disconnect(&self) {
		self.connected.store(false, Ordering::Release);
	}
}

async fn serial_transfer(
	port: &mut SerialStream,
	operation: Operation,
	request_id: u64,
	body: &[u8],
) -> io::Result<UsbReply> {
	let mut encoded = [0; MAX_SERIAL_FRAME];
	let length = encode_serial_frame(body, &mut encoded)
		.map_err(|_| invalid("serial request is too large"))?;
	port.write_all(&encoded[..length]).await?;
	port.flush().await?;

	let mut candidate = Vec::with_capacity(MAX_SERIAL_FRAME);
	let mut chunk = [0; 256];
	loop {
		let count = port.read(&mut chunk).await?;
		if count == 0 {
			return Err(io::Error::new(
				io::ErrorKind::UnexpectedEof,
				"serial device disconnected",
			));
		}
		for &byte in &chunk[..count] {
			if byte != 0 {
				if candidate.len() < MAX_SERIAL_FRAME {
					candidate.push(byte);
				}
				continue;
			}
			let mut decoded = [0; MAX_SERIAL_BODY];
			let Ok(decoded_len) = decode_serial_frame(&candidate, &mut decoded) else {
				candidate.clear();
				continue;
			};
			candidate.clear();
			if decoded_len < USB_HEADER_LEN {
				continue;
			}
			let Ok(header) = UsbHeader::decode(&decoded[..USB_HEADER_LEN]) else {
				continue;
			};
			if !header.reply || header.operation != operation || header.request_id != request_id {
				continue;
			}
			if decoded_len != USB_HEADER_LEN + header.payload_len as usize {
				continue;
			}
			return Ok(UsbReply {
				header,
				payload: decoded[USB_HEADER_LEN..decoded_len].to_vec(),
			});
		}
	}
}

fn invalid(message: &'static str) -> io::Error {
	io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	#[ignore = "requires RP_EXTIO_SERIAL_TEST pointing to a connected rp-extio serial board"]
	async fn negotiates_connected_serial_firmware() {
		let path = std::env::var_os("RP_EXTIO_SERIAL_TEST")
			.map(std::path::PathBuf::from)
			.expect("set RP_EXTIO_SERIAL_TEST");
		let (failed, _failures) = mpsc::unbounded_channel();
		let (_backend, config) =
			SerialBackend::connect(&path, DeviceKey::Serial(path.clone()), failed)
				.await
				.unwrap();
		assert_eq!(config.transport, TransportKind::Serial);
		assert_eq!(config.board_kind, rp_extio_wire::BoardKind::Esp32C3DevKitM1);
	}
}
