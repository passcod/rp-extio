use rp_extiod::{
    control::{ControlRequest, ControlResponse},
    kernel::{DeviceRegistration, Kernel, RegistrationConfig},
    sim::{self, Command, Simulator},
    usb::{self, ControlCall},
};
use std::{
    collections::{HashMap, hash_map::Entry},
    env,
    error::Error,
    ffi::CString,
    os::unix::{ffi::OsStrExt, fs::FileTypeExt, fs::PermissionsExt},
    path::PathBuf,
    sync::Arc,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{Mutex, mpsc, oneshot},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1);
    let mut simulation_socket = None;
    let mut control = PathBuf::from("/dev/rp-extio-control");
    let mut config = PathBuf::from("/etc/rp-extio/config.json");
    let mut control_socket = PathBuf::from("/run/rp-extiod.sock");
    let mut strict_config = false;
    let mut operator_group = None;
    while let Some(argument) = arguments.next() {
        if argument == "--simulate" {
            simulation_socket = arguments.next().map(PathBuf::from);
        } else if argument == "--control" {
            control = arguments
                .next()
                .map(PathBuf::from)
                .ok_or("--control needs a path")?;
        } else if argument == "--config" {
            config = arguments
                .next()
                .map(PathBuf::from)
                .ok_or("--config needs a path")?;
        } else if argument == "--socket" {
            control_socket = arguments
                .next()
                .map(PathBuf::from)
                .ok_or("--socket needs a path")?;
        } else if argument == "--strict-config" {
            strict_config = true;
        } else if argument == "--operator-group" {
            operator_group = Some(
                arguments
                    .next()
                    .ok_or("--operator-group needs a group name")?
                    .to_string_lossy()
                    .into_owned(),
            );
        } else {
            return Err(format!("unknown argument: {}", argument.to_string_lossy()).into());
        }
    }
    let kernel = Kernel::open(control)?;
    let dispatcher = kernel.clone().run();
    let (control_tx, control_rx) = mpsc::channel(32);
    let usb = usb::run(kernel.clone(), config, strict_config, control_rx);
    let control = serve_control(control_socket, operator_group, control_tx);
    tokio::pin!(dispatcher, usb);
    tokio::pin!(control);
    if let Some(socket) = simulation_socket {
        let simulation = serve_simulation(socket, kernel);
        tokio::pin!(simulation);
        tokio::select! {
            result = &mut dispatcher => result?,
            result = &mut usb => result?,
            result = &mut control => result?,
            result = &mut simulation => result?,
            result = tokio::signal::ctrl_c() => result?,
        }
    } else {
        tokio::select! {
            result = &mut dispatcher => result?,
            result = &mut usb => result?,
            result = &mut control => result?,
            result = tokio::signal::ctrl_c() => result?,
        }
    }
    Ok(())
}

struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn serve_control(
    socket: PathBuf,
    operator_group: Option<String>,
    commands: mpsc::Sender<ControlCall>,
) -> std::io::Result<()> {
    if let Ok(metadata) = std::fs::symlink_metadata(&socket) {
        if !metadata.file_type().is_socket() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("refusing to replace non-socket {}", socket.display()),
            ));
        }
        std::fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    let _guard = SocketGuard(socket.clone());
    let group = operator_group.as_deref().map(group_id).transpose()?;
    std::fs::set_permissions(
        &socket,
        std::fs::Permissions::from_mode(if group.is_some() { 0o660 } else { 0o600 }),
    )?;
    if let Some(group) = group {
        let path = CString::new(socket.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
        if unsafe { libc::chown(path.as_ptr(), u32::MAX, group) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    loop {
        let (stream, _) = listener.accept().await?;
        let commands = commands.clone();
        tokio::spawn(async move {
            if let Err(error) = serve_control_client(stream, commands).await {
                eprintln!("rp-extio control client failed: {error}");
            }
        });
    }
}

async fn serve_control_client(
    stream: UnixStream,
    commands: mpsc::Sender<ControlCall>,
) -> std::io::Result<()> {
    let _credentials = stream.peer_cred()?;
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        let response = match serde_json::from_str::<ControlRequest>(&line) {
            Ok(request) => {
                let (response, receive) = oneshot::channel();
                commands
                    .send(ControlCall { request, response })
                    .await
                    .map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::BrokenPipe,
                            "device manager stopped",
                        )
                    })?;
                receive.await.map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "device manager dropped response",
                    )
                })?
            }
            Err(error) => ControlResponse::error("invalid_request", error.to_string()),
        };
        let mut encoded = serde_json::to_vec(&response).map_err(std::io::Error::other)?;
        encoded.push(b'\n');
        write.write_all(&encoded).await?;
    }
    Ok(())
}

fn group_id(name: &str) -> std::io::Result<u32> {
    let name = CString::new(name).map_err(std::io::Error::other)?;
    let group = unsafe { libc::getgrnam(name.as_ptr()) };
    if group.is_null() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "operator group does not exist",
        ));
    }
    Ok(unsafe { (*group).gr_gid })
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
                    Ok(Command::SetInput {
                        device,
                        line,
                        value,
                    }) => {
                        let registrations = registrations.lock().await;
                        match registrations.get(&device) {
                            Some(registration) => {
                                match simulator.set_input(&device, line, value).await {
                                    Ok(old) => {
                                        let result = if old == value {
                                            Ok(())
                                        } else {
                                            kernel.gpio_event(
                                                registration.gpio_instance_id(),
                                                line,
                                                old,
                                                value,
                                            )
                                        };
                                        match result {
                                            Ok(()) => sim::Response::Ok,
                                            Err(error) => sim::Response::Error {
                                                message: error.to_string(),
                                            },
                                        }
                                    }
                                    Err(message) => sim::Response::Error { message },
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
