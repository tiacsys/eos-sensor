use core::str::FromStr;

use embassy_net::{tcp::TcpSocket, Ipv4Address, Stack};
use embedded_websocket as ews;
use rand_core::RngCore;

pub struct WebSocket<'a, R, const BUFSIZE: usize>
where
    R: RngCore,
{
    tcp_socket: TcpSocket<'a>,
    ws_client: ews::WebSocketClient<R>,
    ws_frame_buffer: &'a mut [u8],
}

impl<'a, R, const BUFSIZE: usize> WebSocket<'a, R, BUFSIZE>
where
    R: RngCore,
{
    pub async fn new(
        host: &'a str,
        port: u16,
        endpoint: &'a str,
        net_stack: Stack<'a>,
        rng: R,
        buffers: &'a mut WebSocketBuffers<BUFSIZE>,
    ) -> Result<Self, WebSocketError> {
        // Open TCP socket
        let mut tcp_socket = TcpSocket::new(net_stack, &mut buffers.tcp_rx, &mut buffers.tcp_tx);
        let remote = (
            Ipv4Address::from_str(host).map_err(|_| WebSocketError::InvalidAddress)?,
            port,
        );
        tcp_socket.connect(remote).await?;

        let ws_options = ews::WebSocketOptions {
            path: endpoint,
            host,
            origin: host,
            additional_headers: None,
            sub_protocols: None,
        };

        let mut ws_client = ews::WebSocketClient::new_client(rng);

        let (len, key) = ws_client.client_connect(&ws_options, &mut buffers.ws_frame)?;

        tcp_socket.write(&buffers.ws_frame[..len]).await?;
        tcp_socket.flush().await?;

        let len = tcp_socket.read(&mut buffers.ws_frame).await?;
        ws_client.client_accept(&key, &buffers.ws_frame[..len])?;

        Ok(Self {
            tcp_socket,
            ws_client,
            ws_frame_buffer: &mut buffers.ws_frame,
        })
    }

    pub async fn send_text(&mut self, text: &str) -> Result<(), WebSocketError> {
        let len = self.ws_client.write(
            ews::WebSocketSendMessageType::Text,
            true,
            text.as_bytes(),
            self.ws_frame_buffer,
        )?;
        self.tcp_socket.write(&self.ws_frame_buffer[..len]).await?;
        self.tcp_socket.flush().await?;
        Ok(())
    }

    pub async fn send_binary(&mut self, buf: &[u8]) -> Result<(), WebSocketError> {
        let len = self.ws_client.write(
            ews::WebSocketSendMessageType::Binary,
            true,
            buf,
            self.ws_frame_buffer,
        )?;
        self.tcp_socket.write(&self.ws_frame_buffer[..len]).await?;
        self.tcp_socket.flush().await?;
        Ok(())
    }
}

pub struct WebSocketBuffers<const BUFSIZE: usize> {
    tcp_rx: [u8; BUFSIZE],
    tcp_tx: [u8; BUFSIZE],
    ws_frame: [u8; BUFSIZE],
}

impl<const BUFSIZE: usize> WebSocketBuffers<BUFSIZE> {
    pub fn new() -> Self {
        Self {
            tcp_rx: [0; BUFSIZE],
            tcp_tx: [0; BUFSIZE],
            ws_frame: [0; BUFSIZE],
        }
    }
}

#[derive(Debug)]
pub enum WebSocketError {
    InvalidAddress,
    TcpError,
    Underlying(ews::Error),
}

impl core::fmt::Display for WebSocketError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> Result<(), core::fmt::Error> {
        match self {
            Self::Underlying(e) => e.fmt(f),
            _ => write!(f, "{:?}", self),
        }
    }
}

impl core::error::Error for WebSocketError {}

impl From<embassy_net::tcp::Error> for WebSocketError {
    fn from(_value: embassy_net::tcp::Error) -> Self {
        WebSocketError::TcpError
    }
}

impl From<embassy_net::tcp::ConnectError> for WebSocketError {
    fn from(_value: embassy_net::tcp::ConnectError) -> Self {
        WebSocketError::TcpError
    }
}

impl From<ews::Error> for WebSocketError {
    fn from(value: ews::Error) -> Self {
        WebSocketError::Underlying(value)
    }
}
