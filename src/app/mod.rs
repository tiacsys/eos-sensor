use hecate_protobuf as proto;

mod network;

use network::network_task;

use anyhow::Result;
use alloc::sync::Arc;
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Instant, Timer};
use esp_hal::{i2c::master::I2c, Blocking};
use esp_wifi::wifi::{WifiDevice, WifiStaDevice};
use lsm9ds1::{interface::I2cInterface, Lsm9ds1};
use ringbuffer::{AllocRingBuffer, RingBuffer};

type NetworkInterface = WifiDevice<'static, WifiStaDevice>;
type Rng = esp_hal::rng::Rng;
type Sensor = Lsm9ds1<I2cInterface<I2c<'static, Blocking>>>;
type RingBufferMutex = Arc<Mutex<CriticalSectionRawMutex, AllocRingBuffer<proto::SensorDataSample>>>;

pub struct AppConfig {
    pub ws_host: &'static str,
    pub ws_port: u16,
    pub ws_endpoint: &'static str,
    pub device_id: &'static str,
}

pub struct AppPeripherals {
    pub network_interface: NetworkInterface,
    pub rng: Rng,
    pub sensor: Sensor,
}

#[embassy_executor::task]
pub async fn app(peripherals: AppPeripherals, config: AppConfig) {
    let spawner = Spawner::for_current_executor().await;

    // Set up ringbuffer for sensor data
    let ringbuffer = AllocRingBuffer::<proto::SensorDataSample>::new(512);
    let ringbuffer = Arc::new(Mutex::<CriticalSectionRawMutex, _>::new(ringbuffer));

    let mut sensor = peripherals.sensor;
    let temp = sensor.temperature_c().expect("Error reading temperature");
    log::info!("Temp: {temp}°C");

    // Spawn network task
    _ = spawner.spawn(network_task(
        config,
        peripherals.network_interface,
        peripherals.rng,
        ringbuffer.clone(),
    ));

    // Spawn sensor sampling task
    _ = spawner.spawn(sensor_sampling_task(sensor, ringbuffer.clone()));
}

#[embassy_executor::task]
async fn sensor_sampling_task(
    mut sensor: Sensor,
    ringbuffer: RingBufferMutex,
) {
    let start_time = Instant::now();

    loop {
        let timer = Timer::after_millis(100);
        if let Ok((acc, gyro, mag)) = sample_data(&mut sensor).await {
            let time = Instant::now() - start_time;
            let sample = proto::SensorDataSample {
                time: time.as_millis() as f32 / 1000.0,
                acceleration: acc,
                gyroscope: gyro,
                magnetometer: mag,
            };

            {
                let mut ringbuffer = ringbuffer.lock().await;
                ringbuffer.push(sample);
            }
        }
        
        timer.await;
    }
}

async fn sample_data(sensor: &mut Sensor) -> Result<(proto::Acceleration, proto::GyroscopeData, proto::MagnetometerData)> {
        let acc = sensor
            .get_accelerometer_data()
            .map(|(x, y, z)| proto::Acceleration { x, y, z })?;
        let gyro = sensor
            .get_gyroscope_data()
            .map(|(x, y, z)| proto::GyroscopeData { x, y, z })?;
        let mag = sensor
            .get_magnetometer_data()
            .map(|(x, y, z)| proto::MagnetometerData { x, y, z })?;

        Ok((acc, gyro, mag))
}
