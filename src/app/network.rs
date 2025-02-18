use super::{AppConfig, NetworkInterface, Rng};
use crate::make_static;
use crate::ws;
use hecate_protobuf as proto;

use proto::Message;

use alloc::sync::Arc;
use alloc::vec::Vec;
use embassy_executor::Spawner;
use embassy_net::StackResources;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, mutex::Mutex};
use embassy_time::Timer;
use rand_core::RngCore;
use ringbuffer::{AllocRingBuffer, RingBuffer};

#[embassy_executor::task]
pub async fn network_task(
    app_config: AppConfig,
    interface: NetworkInterface,
    mut rng: Rng,
    ringbuffer: Arc<Mutex<NoopRawMutex, AllocRingBuffer<proto::SensorDataSample>>>,
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

        let data = proto::SensorData { samples }.encode_to_vec();
        _ = websocket.send_binary(&data).await;

        Timer::after_millis(10).await;
    }
}

#[embassy_executor::task]
async fn net_stack_task(mut runner: embassy_net::Runner<'static, NetworkInterface>) {
    runner.run().await
}
