#![no_std]
#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]

mod ws;

#[cfg(feature = "platform-esp")]
use platform_esp as platform;

#[cfg(feature = "platform-esp")]
use log::info;

use platform::{
    Sensor,
    NetworkDevice,
    RngDevice,
};

extern crate alloc;
use alloc::sync::Arc;
use alloc::vec::Vec;
use rand::RngCore;
use ringbuffer::{AllocRingBuffer, RingBuffer};
use static_cell::make_static;

use embassy_executor::Spawner;
use embassy_sync::mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex; // We only run tasks on the same executor
use embassy_time::{Instant, Timer};
use embassy_net::{
    Stack,
    StackResources,
};

use hecate_protobuf as proto;
use proto::Message;

pub struct AppConfig {
    pub ws_host: &'static str,
    pub ws_port: u16,
    pub ws_endpoint: &'static str,
    pub device_id: &'static str,
}

pub struct AppPeripherals {
    pub sensor: Sensor,
    pub network_device: NetworkDevice,
    pub rng: RngDevice,
}

#[embassy_executor::task]
pub async fn app(p: AppPeripherals, config: AppConfig) -> () {
    let spawner = embassy_executor::Spawner::for_current_executor().await;

    let AppPeripherals {
        sensor,
        network_device,
        rng
    } = p;

    let ringbuffer = AllocRingBuffer::<proto::SensorDataSample>::new(512);
    let ringbuffer = Arc::new(Mutex::<NoopRawMutex, _>::new(ringbuffer));

    spawner.spawn(sensor_task(sensor, ringbuffer.clone()))
        .expect("Failed to spawn sensor task");

    spawner.spawn(network_task(config, network_device, rng, ringbuffer.clone()))
        .expect("Failed to spawn network task");
}

#[embassy_executor::task]
async fn network_task(
    app_config: AppConfig,
    interface: NetworkDevice,
    mut rng: RngDevice,
    ringbuffer: Arc<Mutex<NoopRawMutex, AllocRingBuffer<proto::SensorDataSample>>>,
) {

    // Create network stack
    let config = embassy_net::Config::dhcpv4(Default::default());
    let seed = rng.next_u64();

    let stack = make_static!(Stack::new(
        interface,
        config,
        make_static!(StackResources::<3>::new()),
        seed
    ));

    let spawner = Spawner::for_current_executor().await;
    _ = spawner.spawn(net_stack_task(stack));
    

    info!("Waiting for network link");
    loop {
        if stack.is_link_up() {
            info!("Link is up");
            break;
        }
        Timer::after_secs(1).await;
    }

    info!("Waiting for IP address");
    loop {
        if let Some(config) = stack.config_v4() {
            info!("Got IP: {}", config.address);
            break;
        }
        Timer::after_secs(1).await;
    }

    let mut ws_buffers = ws::WebSocketBuffers::<2048>::new();
    let mut websocket = ws::WebSocket::new(
        app_config.ws_host,
        app_config.ws_port,
        app_config.ws_endpoint,
        &stack,
        rng.clone(),
        &mut ws_buffers,
    ).await.expect("Failed to connect");

    info!("Websocket open");

    if let Ok(()) = websocket.send_text(app_config.device_id).await {
        info!("Sent id");
    }

    loop {

        let samples = {
            let mut ringbuffer = ringbuffer.lock().await;
            ringbuffer.drain().take(50).collect::<Vec<_>>()
        };

        let data = proto::SensorData {
            samples,
        }.encode_to_vec();
        _ = websocket.send_binary(&data).await;

        Timer::after_millis(100).await;
    }
}

#[embassy_executor::task]
async fn net_stack_task(stack: &'static Stack<NetworkDevice>) {
    stack.run().await
}

#[embassy_executor::task]
async fn sensor_task(
    mut sensor: Sensor,
    ringbuffer_mut: Arc<Mutex<NoopRawMutex, AllocRingBuffer<proto::SensorDataSample>>>,
) {

    sensor.begin_accel().expect("Failed to initialize accelerometer");
    sensor.begin_gyro().expect("Failed to initialize gyroscope");
    sensor.begin_mag().expect("Failed to initialize magnetometer");

    let start = Instant::now();

    loop {
        let acc = sensor.read_accel();
        let gyro = sensor.read_gyro();
        let mag = sensor.read_mag();

        if let (Ok((ax, ay, az)), Ok((gx, gy, gz)), Ok((mx, my, mz))) = (acc, gyro, mag) {
            let timer = Timer::after_millis(10);
            let time = Instant::now() - start;
            let sample = proto::SensorDataSample {
                time: time.as_millis() as f32 / 1000.0,
                acceleration: proto::Acceleration{ x: ax, y: ay, z: az },
                magnetometer: proto::MagnetometerData { x: mx, y: my, z: mz },
                gyroscope: proto::GyroscopeData { x: gx, y: gy, z: gz },
            };

            {
                let mut ringbuffer = ringbuffer_mut.lock().await;
                ringbuffer.push(sample);
            }

            timer.await;
        }
    }
}
