use core::time;
use interprocess::local_socket::tokio::LocalSocketListener;
use std::path::PathBuf;

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Message {
    RouteReceived(String),
    None,
}

impl From<String> for Message {
    fn from(url: String) -> Self {
        Self::RouteReceived(url)
    }
}

enum State {
    Uninitialized,
    Waiting(LocalSocketListener),
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(windows)]
fn server_path() -> String {
    use std::time;

    let nonce = time::SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    format!("halloy-{}", nonce)
}

#[cfg(windows)]
pub fn server_path_register_path() -> PathBuf {
    data::environment::data_dir().join("ipc.txt")
}

#[cfg(not(windows))]
pub fn socket_directory() -> PathBuf {
    data::environment::data_dir()
}

#[cfg(not(windows))]
pub async fn with_socket_path<T, Fut>(f: impl FnOnce(PathBuf) -> Fut) -> T
where
    Fut: futures::Future<Output = T>,
{
    let file = socket_directory().join("urlserver.sock");
    f(file).await
}

#[cfg(not(windows))]
pub async fn spawn_server() -> Result<LocalSocketListener, Error> {
    with_socket_path(|path| async {
        let _ = tokio::fs::remove_file(path.clone()).await;
        Ok(LocalSocketListener::bind(path)?)
    })
    .await
}

#[cfg(windows)]
async fn spawn_server() -> Result<LocalSocketListener, Error> {
    let path = server_path();
    let named_pipe_addr_file = server_path_register_path();

    tokio::fs::write(named_pipe_addr_file, &path).await?;
    Ok(LocalSocketListener::bind(path)?)
}

pub fn run() -> futures::stream::BoxStream<'static, Message> {
    use futures::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use futures::stream::StreamExt;

    futures::stream::unfold(State::Uninitialized {}, move |state| async move {
        match state {
            State::Uninitialized => match spawn_server().await {
                Ok(server) => Some((Message::None, State::Waiting(server))),
                Err(err) => {
                    println!("error: {:?}", err);
                    None
                }
            },
            State::Waiting(server) => {
                let conn = server.accept().await;

                let Ok(conn) = conn else {
                    return Some((Message::None, State::Waiting(server)));
                };

                let mut conn = BufReader::new(conn);
                let mut buffer = String::new();

                let msg = tokio::time::timeout(
                    time::Duration::from_millis(1_000),
                    conn.read_line(&mut buffer),
                )
                .await;

                let _ = conn.close().await;

                match msg {
                    Ok(Ok(_)) => Some((Message::RouteReceived(buffer), State::Waiting(server))),
                    Err(_) | Ok(Err(_)) => Some((Message::None, State::Waiting(server))),
                }
            }
        }
    })
    .boxed()
}
