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
	process::Stdio,
	sync::Arc,
};
use tokio::{
	io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
	net::{UnixListener, UnixStream, unix::UCred},
	process::Command as ProcessCommand,
	sync::{Mutex, mpsc, oneshot},
};

const POLKIT_ACTION: &str = "org.rp-extio.configure";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
	let mut arguments = env::args_os().skip(1);
	let mut simulation_socket = None;
	let mut control = PathBuf::from("/dev/rp-extio-control");
	let mut config = PathBuf::from("/etc/rp-extio/config.json");
	let mut control_socket = PathBuf::from("/run/rp-extiod.sock");
	let mut strict_config = false;
	let mut operator_group = None;
	let mut serial_paths = Vec::new();
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
		} else if argument == "--serial" {
			serial_paths.push(
				arguments
					.next()
					.map(PathBuf::from)
					.ok_or("--serial needs a device path")?,
			);
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
	let usb = usb::run(
		kernel.clone(),
		config,
		strict_config,
		serial_paths,
		control_rx,
	);
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
	std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o666))?;
	if let Some(group) = group {
		let path = CString::new(socket.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
		if unsafe { libc::chown(path.as_ptr(), u32::MAX, group) } != 0 {
			return Err(std::io::Error::last_os_error());
		}
	}
	loop {
		let (stream, _) = listener.accept().await?;
		let peer = PeerCredentials::from_credentials(stream.peer_cred()?);
		let commands = commands.clone();
		tokio::spawn(async move {
			if let Err(error) = serve_control_client(stream, peer, group, commands).await {
				eprintln!("rp-extio control client failed: {error}");
			}
		});
	}
}

async fn serve_control_client(
	stream: UnixStream,
	peer: PeerCredentials,
	operator_group: Option<u32>,
	commands: mpsc::Sender<ControlCall>,
) -> std::io::Result<()> {
	let (read, mut write) = stream.into_split();
	let mut lines = BufReader::new(read).lines();
	while let Some(line) = lines.next_line().await? {
		let response = match serde_json::from_str::<ControlRequest>(&line) {
			Ok(request) => {
				if request.requires_authorization()
					&& let Err(response) = authorize(&peer, operator_group).await
				{
					write_response(&mut write, &response).await?;
					continue;
				}
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
		write_response(&mut write, &response).await?;
	}
	Ok(())
}

async fn write_response(
	write: &mut tokio::net::unix::OwnedWriteHalf,
	response: &ControlResponse,
) -> std::io::Result<()> {
	let mut encoded = serde_json::to_vec(response).map_err(std::io::Error::other)?;
	encoded.push(b'\n');
	write.write_all(&encoded).await
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PeerCredentials {
	pid: Option<u32>,
	uid: u32,
	gid: u32,
	process: Result<ProcessIdentity, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProcessIdentity {
	pid: u32,
	start_time: u64,
	groups: Vec<u32>,
}

impl PeerCredentials {
	fn from_credentials(credentials: UCred) -> Self {
		let pid = credentials.pid().and_then(|pid| u32::try_from(pid).ok());
		let process = pid
			.ok_or_else(|| "peer PID is unavailable".to_owned())
			.and_then(|pid| {
				ProcessIdentity::read(pid)
					.map_err(|error| format!("cannot resolve peer process {pid}: {error}"))
			});
		Self {
			pid,
			uid: credentials.uid(),
			gid: credentials.gid(),
			process,
		}
	}

	fn is_primary_operator(&self, operator_group: Option<u32>) -> bool {
		self.uid == 0 || operator_group.is_some_and(|group| self.gid == group)
	}

	fn process_identity(&self) -> Result<&ProcessIdentity, ControlResponse> {
		let process = self.process.as_ref().map_err(|detail| {
			ControlResponse::error("authorization_identity", detail.to_owned())
		})?;
		let current_start_time = read_process_start_time(process.pid).map_err(|error| {
			ControlResponse::error(
				"authorization_identity",
				format!("cannot validate peer process {}: {error}", process.pid),
			)
		})?;
		if current_start_time != process.start_time {
			return Err(ControlResponse::error(
				"authorization_identity",
				format!(
					"peer process {} no longer matches this connection",
					process.pid
				),
			));
		}
		Ok(process)
	}
}

impl ProcessIdentity {
	fn read(pid: u32) -> std::io::Result<Self> {
		let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
		let status = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
		Ok(Self {
			pid,
			start_time: parse_process_start_time(&stat)?,
			groups: parse_process_groups(&status)?,
		})
	}
}

fn read_process_start_time(pid: u32) -> std::io::Result<u64> {
	let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
	parse_process_start_time(&stat)
}

fn parse_process_start_time(stat: &str) -> std::io::Result<u64> {
	let (_, fields) = stat.rsplit_once(')').ok_or_else(|| {
		std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid process stat")
	})?;
	fields
		.split_whitespace()
		.nth(19)
		.ok_or_else(|| {
			std::io::Error::new(
				std::io::ErrorKind::InvalidData,
				"process start time is missing",
			)
		})?
		.parse()
		.map_err(std::io::Error::other)
}

fn parse_process_groups(status: &str) -> std::io::Result<Vec<u32>> {
	let groups = status
		.lines()
		.find_map(|line| line.strip_prefix("Groups:"))
		.ok_or_else(|| {
			std::io::Error::new(
				std::io::ErrorKind::InvalidData,
				"process groups are missing",
			)
		})?;
	groups
		.split_whitespace()
		.map(|group| group.parse().map_err(std::io::Error::other))
		.collect()
}

async fn authorize(
	peer: &PeerCredentials,
	operator_group: Option<u32>,
) -> Result<(), ControlResponse> {
	if peer.is_primary_operator(operator_group) {
		return Ok(());
	}
	let process = peer.process_identity()?;
	if operator_group.is_some_and(|group| process.groups.contains(&group)) {
		return Ok(());
	}
	let subject = format!("{},{},{}", process.pid, process.start_time, peer.uid);
	let output = ProcessCommand::new("pkcheck")
		.args([
			"--action-id",
			POLKIT_ACTION,
			"--process",
			&subject,
			"--allow-user-interaction",
		])
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::piped())
		.output()
		.await
		.map_err(|error| ControlResponse::error("authorization_error", error.to_string()))?;
	match output.status.code() {
		Some(0) => Ok(()),
		Some(1) => Err(ControlResponse::error(
			"authorization_denied",
			"authorization to configure rp-extio devices was denied",
		)),
		Some(2) => Err(ControlResponse::error(
			"authorization_unavailable",
			"no Polkit authentication agent is available",
		)),
		Some(3) => Err(ControlResponse::error(
			"authorization_cancelled",
			"rp-extio configuration authentication was cancelled",
		)),
		code => {
			let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
			Err(ControlResponse::error(
				"authorization_error",
				if detail.is_empty() {
					format!("Polkit authorization failed with status {code:?}")
				} else {
					detail
				},
			))
		}
	}
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
											i2c_buses: buses.i2c,
											spi_buses: buses.spi,
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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_process_identity_fields() {
		let fields = (3_u64..=22)
			.map(|field| field.to_string())
			.collect::<Vec<_>>()
			.join(" ");
		let stat = format!("42 (name with ) and spaces) {fields}");
		assert_eq!(parse_process_start_time(&stat).unwrap(), 22);
		assert_eq!(
			parse_process_groups("Name:\ttest\nGroups:\t10 20 60426 \n").unwrap(),
			vec![10, 20, 60426]
		);
	}

	#[test]
	fn root_primary_and_supplementary_operator_groups_bypass_polkit() {
		let process = ProcessIdentity {
			pid: std::process::id(),
			start_time: read_process_start_time(std::process::id()).unwrap(),
			groups: vec![10, 20],
		};
		let mut peer = PeerCredentials {
			pid: Some(process.pid),
			uid: 1000,
			gid: 1000,
			process: Ok(process),
		};
		assert!(!peer.is_primary_operator(Some(30)));
		assert!(peer.is_primary_operator(Some(1000)));
		assert!(peer.process_identity().unwrap().groups.contains(&20));
		peer.uid = 0;
		assert!(peer.is_primary_operator(None));
	}

	#[tokio::test]
	async fn malformed_process_identity_blocks_only_nonoperators() {
		let mut peer = PeerCredentials {
			pid: Some(42),
			uid: 1000,
			gid: 1000,
			process: Err("process disappeared".into()),
		};
		assert!(matches!(
			authorize(&peer, None).await,
			Err(ControlResponse::Error { code, .. }) if code == "authorization_identity"
		));

		peer.uid = 0;
		assert_eq!(authorize(&peer, None).await, Ok(()));
	}
}
