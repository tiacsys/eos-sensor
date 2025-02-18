use super::{AppConfig, NetworkInterface, Rng};
use crate::make_static;
use crate::ws;
use hecate_protobuf as proto;

use proto::Message;

use alloc::vec::Vec;
use anyhow::{anyhow, Error, Result};
use embassy_executor::Spawner;
use embassy_net::StackResources;
use embassy_time::Timer;
use futures::future::TryFutureExt;
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
    _ = spawner.spawn(net_stack_task(runner));

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

    let mut ws_buffers = ws::WebSocketBuffers::<2048>::new();
    let mut websocket = ws::WebSocket::new(
        app_config.ws_host,
        app_config.ws_port,
        app_config.ws_endpoint,
        stack,
        rng,
        &mut ws_buffers,
    )
    .await
    .expect("Failed to connect");

    log::info!("Websocket open");

    if let Ok(()) = websocket.send_text(app_config.device_id).await {
        log::info!("Sent id");
    }

    loop {
        let samples = {
            let mut ringbuffer = ringbuffer.lock().await;
            ringbuffer.drain().take(50).collect::<Vec<_>>()
        };

        let data = proto::SensorData { samples };

        // By the Twelve I wish I could just >>= here instead of juggling that async move block
        _ = encode_data(data)
            .and_then({
                // Need to borrow `websocket` outside of the closure so the move doesn't consume it:
                let websocket = &mut websocket;
                |encoded_data| async move { send_data(&encoded_data, websocket).await }
            })
            .await
            .inspect_err(|e| log::warn!("{:?}", e));

        Timer::after_millis(100).await;
    }
}

#[doc(hidden)]
/// Takes some data and tries to encode it. Async to allow chaining via TryFutureExt's .and_then()
async fn encode_data<T>(data: T) -> Result<Vec<u8>, Error>
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
/// Takes a buffer of bytes and tries to send them via a websocket connection.
use crate::ws::WebSocket;
async fn send_data<T, const N: usize>(
    data: &[u8],
    websocket: &mut WebSocket<'_, T, N>,
) -> Result<(), Error>
where
    T: RngCore,
{
    websocket
        .send_binary(data)
        .map_err(|e| anyhow!("Websocket error: {:?}", e))
        .await
}

/// Task that the network stack is running on.
#[embassy_executor::task]
async fn net_stack_task(mut runner: embassy_net::Runner<'static, NetworkInterface>) {
    runner.run().await
}
