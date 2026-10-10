use rp_extiod::{
    kernel::{DeviceRegistration, Kernel, RegistrationConfig},
    sim::{self, Command, Simulator},
    usb,
};
use std::{
    collections::{HashMap, hash_map::Entry},
    env,
    error::Error,
    path::PathBuf,
    sync::Arc,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixListener,
    sync::Mutex,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1);
    let mut socket = None;
    let mut control = PathBuf::from("/dev/rp-extio-control");
    while let Some(argument) = arguments.next() {
        if argument == "--simulate" {
            socket = arguments.next().map(PathBuf::from);
        } else if argument == "--control" {
            control = arguments
                .next()
                .map(PathBuf::from)
                .ok_or("--control needs a path")?;
        } else {
            return Err(format!("unknown argument: {}", argument.to_string_lossy()).into());
        }
    }
    let kernel = Kernel::open(control)?;
    let dispatcher = kernel.clone().run();
    let usb = usb::run(kernel.clone());
    tokio::pin!(dispatcher, usb);
    if let Some(socket) = socket {
        let simulation = serve_simulation(socket, kernel);
        tokio::pin!(simulation);
        tokio::select! {
            result = &mut dispatcher => result?,
            result = &mut usb => result?,
            result = &mut simulation => result?,
            result = tokio::signal::ctrl_c() => result?,
        }
    } else {
        tokio::select! {
            result = &mut dispatcher => result?,
            result = &mut usb => result?,
            result = tokio::signal::ctrl_c() => result?,
        }
    }
    Ok(())
}

async fn serve_simulation(socket: PathBuf, kernel: Arc<Kernel>) -> std::io::Result<()> {
    if socket.exists() {
        std::fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    let simulator = Arc::new(Simulator::default());
    let registrations = Arc::new(Mutex::new(HashMap::<String, DeviceRegistration>::new()));
    loop {
        let (stream, _) = listener.accept().await?;
        let simulator = simulator.clone();
        let kernel = kernel.clone();
        let registrations = registrations.clone();
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let response = match serde_json::from_str::<Command>(&line) {
                    Ok(Command::Create { device }) => {
                        let mut registrations = registrations.lock().await;
                        match registrations.entry(device.clone()) {
                            Entry::Occupied(_) => sim::Response::Error {
                                message: "device already exists".into(),
                            },
                            Entry::Vacant(entry) => match simulator
                                .command(Command::Create {
                                    device: device.clone(),
                                })
                                .await
                            {
                                sim::Response::Ok => match kernel.register_device(
                                    Arc::new(simulator.device(device.clone())),
                                    RegistrationConfig::PICO2,
                                ) {
                                    Ok(registration) => {
                                        let buses = registration.buses;
                                        entry.insert(registration);
                                        sim::Response::Created {
                                            i2c_bus: buses.i2c,
                                            spi_bus: buses.spi,
                                        }
                                    }
                                    Err(error) => {
                                        simulator.command(Command::Remove { device }).await;
                                        sim::Response::Error {
                                            message: error.to_string(),
                                        }
                                    }
                                },
                                response => response,
                            },
                        }
                    }
                    Ok(Command::Remove { device }) => {
                        let registration = registrations.lock().await.remove(&device);
                        match registration {
                            Some(registration) => {
                                let destroy = registration.destroy();
                                let response = simulator.command(Command::Remove { device }).await;
                                match destroy {
                                    Ok(()) => response,
                                    Err(error) => sim::Response::Error {
                                        message: error.to_string(),
                                    },
                                }
                            }
                            None => sim::Response::Error {
                                message: "unknown device".into(),
                            },
                        }
                    }
                    Ok(command) => simulator.command(command).await,
                    Err(error) => sim::Response::Error {
                        message: error.to_string(),
                    },
                };
                let mut encoded = serde_json::to_vec(&response).unwrap();
                encoded.push(b'\n');
                if write.write_all(&encoded).await.is_err() {
                    break;
                }
            }
        });
    }
}
