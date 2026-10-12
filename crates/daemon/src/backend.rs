use async_trait::async_trait;
use rp_extio_wire::Status;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct I2cMessage {
	pub address: u16,
	pub flags: u16,
	#[serde(default)]
	pub data: Vec<u8>,
	#[serde(default)]
	pub read_len: u16,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SpiSegment {
	pub speed_hz: u32,
	pub flags: u16,
	pub word_delay_ns: u32,
	pub delay_ns: u32,
	pub cs_change_delay_ns: u32,
	pub tx: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "bus", rename_all = "snake_case")]
pub enum Transaction {
	I2c {
		bus_id: u8,
		messages: Vec<I2cMessage>,
	},
	Spi {
		bus_id: u8,
		chip_select: u8,
		mode: u16,
		segments: Vec<SpiSegment>,
	},
	Gpio {
		command: u8,
		line: u8,
		value: u8,
	},
	Adc {
		channel: u8,
	},
	Pwm {
		command: u8,
		channel: u8,
		enabled: bool,
		inverted: bool,
		period_ns: u64,
		duty_ns: u64,
	},
}

impl Transaction {
	pub fn pwm_state(&self) -> Option<rp_extio_wire::PwmState> {
		let Self::Pwm {
			channel,
			enabled,
			inverted,
			period_ns,
			duty_ns,
			..
		} = self
		else {
			return None;
		};
		Some(rp_extio_wire::PwmState {
			channel: *channel,
			enabled: *enabled,
			inverted: *inverted,
			period_ns: *period_ns,
			duty_ns: *duty_ns,
		})
	}
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BackendReply {
	pub status: i32,
	#[serde(default)]
	pub completed: u32,
	#[serde(default)]
	pub data: Vec<u8>,
	#[serde(default)]
	pub effective_speeds: Vec<u32>,
}

impl BackendReply {
	pub fn error(status: Status) -> Self {
		Self {
			status: status as i32,
			completed: 0,
			data: vec![],
			effective_speeds: vec![],
		}
	}
}

#[async_trait]
pub trait Backend: Send + Sync {
	async fn transact(&self, transaction: Transaction) -> BackendReply;
	async fn disconnect(&self);
}
