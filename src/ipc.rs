use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream, prelude::*,
};
use std::env;
use std::io::{self, BufRead, BufReader, Write};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

#[derive(Debug)]
pub enum AcquireError {
    /// Another instance is running. We signaled it to show itself.
    /// The application should exit gracefully.
    AlreadyRunning,
    /// A fatal IO error occurred.
    Io(std::io::Error),
}

pub enum IpcEvent {
    Show,
}

pub struct SingleInstance {
    // We keep the listener options/name logic encapsulated
    listener: Option<interprocess::local_socket::Listener>,
}

impl SingleInstance {
    /// Attempts to become the primary instance.
    pub fn acquire(app_id: &str) -> Result<Self, AcquireError> {
        let (token, name) = create_socket_name(app_id);
        let name = name.map_err(AcquireError::Io)?;

        // Configure the listener using the Builder pattern (Reference style)
        let opts = ListenerOptions::new().name(name.clone());

        // 1. Try to create the listener (Bind)
        match opts.create_sync() {
            Ok(listener) => Ok(Self {
                listener: Some(listener),
            }),
            Err(e)
                if e.kind() == io::ErrorKind::AddrInUse
                    || e.kind() == io::ErrorKind::PermissionDenied =>
            {
                // 2. Address in use: Is it a live process or a "corpse socket"?

                // Try to connect to it.
                match connect_and_signal(&name) {
                    Ok(_) => {
                        // Connection worked -> The other process is alive.
                        Err(AcquireError::AlreadyRunning)
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "Failed to connect to existing instance");
                        // Connection failed - might be a corpse socket.
                        // If this is a filesystem socket (Unix/macOS), try to clean it up.
                        if name.is_path() && !cfg!(windows) {
                            tracing::info!(socket_path = %token, "Removing stale socket file");
                            let _ = std::fs::remove_file(&token);
                            // Retry binding with new options
                            let retry_opts = ListenerOptions::new().name(name.clone());
                            match retry_opts.create_sync() {
                                Ok(listener) => {
                                    return Ok(Self {
                                        listener: Some(listener),
                                    });
                                }
                                Err(retry_err) => return Err(AcquireError::Io(retry_err)),
                            }
                        }
                        // On Windows (Namespaced), AddrInUse + ConnectionFailed usually
                        // implies a permission issue or a race condition.
                        Err(AcquireError::AlreadyRunning)
                    }
                }
            }
            Err(e) => Err(AcquireError::Io(e)),
        }
    }

    /// Spawns the background IPC server loop.
    pub fn start_ipc_server(&mut self, ipc_events_tx: Sender<IpcEvent>) {
        let Some(listener) = self.listener.take() else {
            return;
        };

        if let Err(err) = thread::Builder::new()
            .name("ipc-server".to_string())
            .spawn(move || {
                tracing::info!("IPC server thread started");
                // Reference style: filter_map to handle initial connection errors
                for conn in listener.incoming().filter_map(handle_incoming_error) {
                    // Wrap in BufReader immediately
                    let mut reader = BufReader::new(conn);
                    let mut buffer = String::new();

                    // Read a line (blocking until \n is received or connection closes)
                    if let Ok(_) = reader.read_line(&mut buffer) {
                        tracing::debug!(cmd = %buffer.trim(), "Received IPC command");
                        // Check protocol
                        if buffer.trim() == "SHOW" {
                            let _ = ipc_events_tx.send(IpcEvent::Show);
                        }
                    }
                }
            })
        {
            tracing::error!(error = %err, "Failed to spawn IPC server thread");
        }
    }
}

// --- Helper Functions ---

/// Filter function from the official reference
fn handle_incoming_error(conn: io::Result<Stream>) -> Option<Stream> {
    match conn {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!(error = %e, "Incoming IPC connection failed");
            None
        }
    }
}

fn connect_and_signal(name: &Name) -> io::Result<()> {
    // Retry strategy for the client side (in case server is currently binding)
    let mut retries = 5;
    while retries > 0 {
        match Stream::connect(name.clone()) {
            Ok(mut stream) => {
                stream.write_all(b"SHOW\n")?;
                stream.flush()?;
                tracing::info!("Signaled existing instance to show");
                return Ok(());
            }
            Err(e) => {
                let is_waitable = matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                );
                if !is_waitable {
                    return Err(e);
                }
            }
        }
        thread::sleep(Duration::from_millis(50));
        retries -= 1;
    }

    // Final attempt
    let mut stream = Stream::connect(name.clone())?;
    stream.write_all(b"SHOW\n")?;
    stream.flush()?;
    tracing::info!("Signaled existing instance to show");
    Ok(())
}

fn create_socket_name(app_id: &str) -> (String, io::Result<Name<'static>>) {
    let user = sanitize(&current_user_token());
    let safe_id = sanitize(app_id);
    let token = format!("{}-{}", safe_id, user);

    // Platform selection strategy:
    // Windows -> Named Pipes (GenericNamespaced)
    // Unix/macOS -> File Paths (GenericFilePath)
    if cfg!(windows) {
        (token.clone(), token.to_ns_name::<GenericNamespaced>())
    } else {
        let mut path = env::temp_dir();
        path.push(format!("{}.sock", token));
        // Must convert PathBuf string to FS Name
        let token = path.to_string_lossy().to_string();
        (token.clone(), token.to_fs_name::<GenericFilePath>())
    }
}

fn current_user_token() -> String {
    env::var("USERNAME")
        .or_else(|_| env::var("USER"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
