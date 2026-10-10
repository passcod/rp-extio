use crate::backend::{Backend, BackendReply, Transaction};
use async_trait::async_trait;
use rp_extio_wire::Status;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};
use tokio::sync::Mutex;

#[derive(Debug, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Create {
        device: String,
    },
    Remove {
        device: String,
    },
    Expect {
        device: String,
        transaction: Transaction,
        reply: BackendReply,
    },
    Observed {
        device: String,
    },
    Reset {
        device: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Created { i2c_bus: i32, spi_bus: i32 },
    Observed { transactions: Vec<Transaction> },
    Error { message: String },
}

struct Expectation {
    transaction: Transaction,
    reply: BackendReply,
}
#[derive(Default)]
struct Device {
    expectations: VecDeque<Expectation>,
    observed: Vec<Transaction>,
    connected: bool,
}
#[derive(Default)]
pub struct Simulator {
    devices: Mutex<HashMap<String, Device>>,
}

impl Simulator {
    pub async fn command(&self, command: Command) -> Response {
        let mut devices = self.devices.lock().await;
        match command {
            Command::Create { device } => {
                if devices
                    .insert(
                        device,
                        Device {
                            connected: true,
                            ..Default::default()
                        },
                    )
                    .is_some()
                {
                    Response::Error {
                        message: "device already exists".into(),
                    }
                } else {
                    Response::Ok
                }
            }
            Command::Remove { device } => {
                if devices.remove(&device).is_some() {
                    Response::Ok
                } else {
                    Response::Error {
                        message: "unknown device".into(),
                    }
                }
            }
            Command::Expect {
                device,
                transaction,
                reply,
            } => match devices.get_mut(&device) {
                Some(state) => {
                    state
                        .expectations
                        .push_back(Expectation { transaction, reply });
                    Response::Ok
                }
                None => Response::Error {
                    message: "unknown device".into(),
                },
            },
            Command::Observed { device } => match devices.get(&device) {
                Some(state) => Response::Observed {
                    transactions: state.observed.clone(),
                },
                None => Response::Error {
                    message: "unknown device".into(),
                },
            },
            Command::Reset { device } => match devices.get_mut(&device) {
                Some(state) => {
                    state.expectations.clear();
                    state.observed.clear();
                    state.connected = true;
                    Response::Ok
                }
                None => Response::Error {
                    message: "unknown device".into(),
                },
            },
        }
    }
    pub fn device(self: &Arc<Self>, name: impl Into<String>) -> SimBackend {
        SimBackend {
            simulator: self.clone(),
            device: name.into(),
        }
    }
}

pub struct SimBackend {
    simulator: Arc<Simulator>,
    device: String,
}

#[async_trait]
impl Backend for SimBackend {
    async fn transact(&self, transaction: Transaction) -> BackendReply {
        let mut devices = self.simulator.devices.lock().await;
        let Some(device) = devices.get_mut(&self.device) else {
            return BackendReply::error(Status::Disconnected);
        };
        device.observed.push(transaction.clone());
        if !device.connected {
            return BackendReply::error(Status::Disconnected);
        }
        let Some(expectation) = device.expectations.pop_front() else {
            return BackendReply::error(Status::IoError);
        };
        if expectation.transaction != transaction {
            device.expectations.push_front(expectation);
            return BackendReply::error(Status::IoError);
        }
        expectation.reply
    }
    async fn disconnect(&self) {
        if let Some(device) = self.simulator.devices.lock().await.get_mut(&self.device) {
            device.connected = false;
        }
    }
}
