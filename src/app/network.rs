use super::{AppConfig, NetworkInterface, Rng};
use crate::make_static;
use crate::ws;
use crate::ws::WebSocketBuffers;
use hecate_protobuf as proto;

use proto::Message;

use alloc::vec::Vec;
use anyhow::{anyhow, Error, Result};
use embassy_executor::Spawner;
use embassy_net::{Stack, StackResources};
use embassy_time::Timer;
use rand_core::RngCore;
use ringbuffer::RingBuffer;

#[embassy_executor::task]
pub async fn network_task(
    app_config: AppConfig,
    interface: NetworkInterface,
    mut rng: Rng,
    ringbuffer: super::RingBufferMutex,
) {
    // Create network stack
    let config = embassy_net::Config::dhcpv4(Default::default());
    let seed = rng.next_u64();

    let stack_resources = make_static!(StackResources<3>, StackResources::new());
    let (stack, runner) = embassy_net::new(interface, config, stack_resources, seed);

    let spawner = Spawner::for_current_executor().await;
    spawner
        .spawn(net_stack_task(runner))
        .expect("Failed to spawn network stack task");

    let mut ws_buffers = WebSocketBuffers::<4096>::new();

    loop {
        wait_for_connection(stack).await;
        if let Ok(mut websocket) =
            try_open_websocket(&mut ws_buffers, stack, &app_config, rng).await
        {
            log::info!("Websocket open!");

            if let Err(e) = websocket.send_text(app_config.device_id).await {
                log::error!("Failed to send device ID: {:?}", e);
                continue;
            }

            // This keeps looping until an error occurs.
            if let Err(e) = send_data_from_ringbuffer(ringbuffer.clone(), &mut websocket).await {
                log::error!("{:?}", e);
                continue;
            }
        }
    }
}

async fn wait_for_connection(stack: Stack<'_>) {
    log::info!("Waiting for network link");
    loop {
        if stack.is_link_up() {
            log::info!("Link is up");
            break;
        }
        Timer::after_secs(1).await;
    }

    log::info!("Waiting for IP address");
    loop {
        if let Some(config) = stack.config_v4() {
            log::info!("Got IP: {}", config.address);
            break;
        }
        Timer::after_secs(1).await;
    }
}

/// Tries to open a websocket connection according to the device configuration.
async fn try_open_websocket<'a, const S: usize>(
    ws_buffers: &'a mut ws::WebSocketBuffers<S>,
    stack: Stack<'a>,
    app_config: &AppConfig,
    rng: Rng,
) -> Result<ws::WebSocket<'a, Rng, S>, Error> {
    ws::WebSocket::new(
        app_config.ws_host,
        app_config.ws_port,
        app_config.ws_endpoint,
        stack,
        rng,
        ws_buffers,
    )
    .await
    .map_err(|e| anyhow!("Websocket error: {:?}", e))
}

#[doc(hidden)]
/// Tries to get data from a ringbuffer and sends it via provided websocket. Keeps grabbing data
/// until an error occurs.
async fn send_data_from_ringbuffer<const S: usize>(
    ringbuffer: super::RingBufferMutex,
    websocket: &mut ws::WebSocket<'_, Rng, S>,
) -> Result<(), Error> {
    loop {

        let samples = {
            let mut ringbuffer = ringbuffer.lock().await;
            ringbuffer.drain().take(32).collect::<Vec<_>>()
        };

        let data = proto::SensorData { samples };

        match encode_data(data) {
            Ok(encoded_data) => send_data_over_websocket(&encoded_data, websocket).await?,
            Err(e) => log::error!("{:?}", e),
        };

        Timer::after_millis(100).await;
    }
}

#[doc(hidden)]
/// Takes some data and tries to encode it. Maps any error that might occur to the anyhow one.
fn encode_data<T>(data: T) -> Result<Vec<u8>, Error>
where
    T: Message,
{
    let len = data.encoded_len();
    let mut buffer = Vec::try_with_capacity(len)
        .map_err(|_| anyhow!("Failed to allocate {len} bytes for encoded data"))?;
    data.encode(&mut buffer)
        .map_err(|_| anyhow!("Failed to encode data"))?;
    Ok(buffer)
}

#[doc(hidden)]
/// Takes a buffer of bytes and tries to send them via a websocket connection. Maps any error to
/// the anyhow one.
use crate::ws::WebSocket;
async fn send_data_over_websocket<T, const N: usize>(
    data: &[u8],
    websocket: &mut WebSocket<'_, T, N>,
) -> Result<(), Error>
where
    T: RngCore,
{
    websocket
        .send_binary(data)
        .await
        .map_err(|e| anyhow!("Websocket error: {:?}", e))
}

/// Task that the network stack is running on.
#[embassy_executor::task]
async fn net_stack_task(mut runner: embassy_net::Runner<'static, NetworkInterface>) {
    runner.run().await
}
