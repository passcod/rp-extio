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
    SetInput {
        device: String,
        line: u8,
        value: u8,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Created {
        i2c_buses: [Option<i32>; 2],
        spi_buses: [Option<i32>; 2],
    },
    Observed {
        transactions: Vec<Transaction>,
    },
    Error {
        message: String,
    },
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
    gpio_values: u32,
    gpio_outputs: u32,
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
                    state.gpio_values = 0;
                    state.gpio_outputs = 0;
                    Response::Ok
                }
                None => Response::Error {
                    message: "unknown device".into(),
                },
            },
            Command::SetInput { .. } => Response::Error {
                message: "set_input requires the kernel-connected simulator".into(),
            },
        }
    }

    pub async fn set_input(&self, device: &str, line: u8, value: u8) -> Result<u8, String> {
        if line >= 17 || value > 1 {
            return Err("invalid GPIO input value".into());
        }
        let mut devices = self.devices.lock().await;
        let state = devices.get_mut(device).ok_or("unknown device")?;
        let mask = 1_u32 << line;
        if state.gpio_outputs & mask != 0 {
            return Err("GPIO line is configured as an output".into());
        }
        let old = u8::from(state.gpio_values & mask != 0);
        if value == 0 {
            state.gpio_values &= !mask;
        } else {
            state.gpio_values |= mask;
        }
        Ok(old)
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
        if let Transaction::Gpio {
            command,
            line,
            value,
        } = &transaction
        {
            let (command, line, value) = (*command, *line, *value);
            if command == rp_extio_wire::GPIO_SNAPSHOT {
                return BackendReply {
                    status: 0,
                    completed: 17,
                    data: device.gpio_values.to_le_bytes().to_vec(),
                    effective_speeds: vec![],
                };
            }
            if line >= 17 || value > 1 {
                return BackendReply::error(Status::BadFrame);
            }
            let mask = 1_u32 << line;
            let result = match command {
                rp_extio_wire::GPIO_GET => u8::from(device.gpio_values & mask != 0),
                rp_extio_wire::GPIO_SET if device.gpio_outputs & mask != 0 => {
                    if value == 0 {
                        device.gpio_values &= !mask;
                    } else {
                        device.gpio_values |= mask;
                    }
                    value
                }
                rp_extio_wire::GPIO_DIRECTION_INPUT => {
                    device.gpio_outputs &= !mask;
                    1
                }
                rp_extio_wire::GPIO_DIRECTION_OUTPUT => {
                    device.gpio_outputs |= mask;
                    if value == 0 {
                        device.gpio_values &= !mask;
                    } else {
                        device.gpio_values |= mask;
                    }
                    0
                }
                rp_extio_wire::GPIO_GET_DIRECTION => u8::from(device.gpio_outputs & mask == 0),
                _ => return BackendReply::error(Status::Unsupported),
            };
            return BackendReply {
                status: 0,
                completed: 1,
                data: vec![result],
                effective_speeds: vec![],
            };
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn gpio(backend: &SimBackend, command: u8, line: u8, value: u8) -> BackendReply {
        backend
            .transact(Transaction::Gpio {
                command,
                line,
                value,
            })
            .await
    }

    #[tokio::test]
    async fn gpio_state_supports_direction_values_snapshot_and_reset() {
        let simulator = Arc::new(Simulator::default());
        assert!(matches!(
            simulator
                .command(Command::Create {
                    device: "gpio".into()
                })
                .await,
            Response::Ok
        ));
        let backend = simulator.device("gpio");

        let reply = gpio(&backend, rp_extio_wire::GPIO_DIRECTION_OUTPUT, 2, 1).await;
        assert_eq!(reply.status, Status::Ok as i32);
        assert_eq!(reply.data, [0]);
        assert_eq!(
            gpio(&backend, rp_extio_wire::GPIO_GET, 2, 0).await.data,
            [1]
        );
        assert!(simulator.set_input("gpio", 2, 0).await.is_err());

        assert_eq!(
            gpio(&backend, rp_extio_wire::GPIO_DIRECTION_INPUT, 2, 0)
                .await
                .data,
            [1]
        );
        assert_eq!(simulator.set_input("gpio", 2, 0).await.unwrap(), 1);
        let snapshot = gpio(&backend, rp_extio_wire::GPIO_SNAPSHOT, 0, 0).await;
        assert_eq!(snapshot.completed, 17);
        assert_eq!(snapshot.data, 0_u32.to_le_bytes());

        gpio(&backend, rp_extio_wire::GPIO_DIRECTION_OUTPUT, 3, 1).await;
        simulator
            .command(Command::Reset {
                device: "gpio".into(),
            })
            .await;
        let snapshot = gpio(&backend, rp_extio_wire::GPIO_SNAPSHOT, 0, 0).await;
        assert_eq!(snapshot.data, 0_u32.to_le_bytes());
        assert_eq!(
            gpio(&backend, rp_extio_wire::GPIO_GET_DIRECTION, 3, 0)
                .await
                .data,
            [1]
        );
    }

    #[tokio::test]
    async fn gpio_rejects_invalid_lines_values_and_commands() {
        let simulator = Arc::new(Simulator::default());
        simulator
            .command(Command::Create {
                device: "gpio".into(),
            })
            .await;
        let backend = simulator.device("gpio");

        assert_eq!(
            gpio(&backend, rp_extio_wire::GPIO_GET, 17, 0).await.status,
            Status::BadFrame as i32
        );
        assert_eq!(
            gpio(&backend, rp_extio_wire::GPIO_SET, 0, 2).await.status,
            Status::BadFrame as i32
        );
        assert_eq!(
            gpio(&backend, 0xff, 0, 0).await.status,
            Status::Unsupported as i32
        );
    }
}
