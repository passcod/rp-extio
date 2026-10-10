use rp_extiod::sim::{self, Simulator};
use std::{env, error::Error, path::PathBuf, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixListener,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let socket = env::args_os()
        .skip_while(|a| a != "--simulate")
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: rp-extiod --simulate SOCKET")?;
    if socket.exists() {
        std::fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    let simulator = Arc::new(Simulator::default());
    loop {
        let (stream, _) = listener.accept().await?;
        let simulator = simulator.clone();
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let response = match serde_json::from_str(&line) {
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
