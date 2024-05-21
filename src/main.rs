#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

mod ws;

use esp_backtrace as _;

use alloc::sync::Arc;
use alloc::vec::Vec;

use esp_hal::gpio::{AnyPin, Output, PushPull, InputOutputAnalogPinType};
use esp_hal::prelude::*;
use esp_hal::rng::Rng;
use esp_hal::{
    clock::ClockControl,
    gpio::IO,
    embassy::{
        self,
        executor::Executor,
    },
    peripherals::Peripherals,
    peripherals::I2C0,
    timer::TimerGroup,
    i2c::I2C,
    Blocking,
};
use esp_wifi::wifi::{self, WifiController, WifiDevice, WifiEvent, WifiStaDevice, WifiState, WifiError};

use embassy_executor::Spawner;
use embassy_sync::mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex; // We only run tasks on the same executor
use embassy_time::{Duration, Instant, Timer};
use embassy_net::{
    Stack,
    StackResources,
};

use lsm9ds1::{LSM9DS1Init, LSM9DS1};
use lsm9ds1::interface::I2cInterface;

use hecate_protobuf as proto;
use proto::Message;

use static_cell::make_static;
use ringbuffer::{AllocRingBuffer, RingBuffer};

#[toml_cfg::toml_config]
struct Config {
    #[default("Free WiFi")]
    wifi_ssid: &'static str,
    #[default("BiBiBiBiBi")]
    wifi_psk: &'static str,
    #[default("echo.websocket.org")]
    ws_host: &'static str,
    #[default(8000)]
    ws_port: u16,
    #[default("/")]
    ws_endpoint: &'static str,
    #[default("Eos")]
    device_id: &'static str,
}

#[macro_use]
extern crate alloc;
use core::mem::MaybeUninit;

type SensorType = LSM9DS1<I2cInterface<I2C<'static, I2C0, Blocking>>>;

#[global_allocator]
static ALLOCATOR: esp_alloc::EspHeap = esp_alloc::EspHeap::empty();

fn init_heap() {
    const HEAP_SIZE: usize = 32 * 1024;
    static mut HEAP: MaybeUninit<[u8; HEAP_SIZE]> = MaybeUninit::uninit();

    unsafe {
        ALLOCATOR.init(HEAP.as_mut_ptr() as *mut u8, HEAP_SIZE);
    }
}

#[entry]
fn main() -> ! {
    // Get peripherals
    let peripherals = Peripherals::take();
    let system = peripherals.SYSTEM.split();

    init_heap();

    // Initialize clocks
    let clocks = ClockControl::max(system.clock_control).freeze();
    let timg0 = TimerGroup::new_async(peripherals.TIMG0, &clocks);

    // Initialize embassy (but not the executor itself)
    embassy::init(&clocks, timg0);
    
    // Set up logging
    esp_println::logger::init_logger_from_env();

    // Initialize LED
    let io = IO::new(peripherals.GPIO, peripherals.IO_MUX);
    let led = io.pins.gpio13.into_push_pull_output().degrade();

    // Sensor setup
    let mut qwiic_power = io.pins.gpio2.into_push_pull_output();
    qwiic_power.set_high();
    let ag_addr = lsm9ds1::interface::i2c::AgAddress::_2;
    let mag_addr = lsm9ds1::interface::i2c::MagAddress::_2;
    let sensor_i2c = I2C::new(peripherals.I2C0, io.pins.gpio22, io.pins.gpio20, 100.kHz(), &clocks, None);
    let sensor_interface = lsm9ds1::interface::I2cInterface::init(sensor_i2c, ag_addr, mag_addr);
    let sensor = LSM9DS1Init::default().with_interface(sensor_interface);

    // Initialize WiFi
    let timer = esp_hal::timer::TimerGroup::new(peripherals.TIMG1, &clocks, None).timer0;
    let rng = esp_hal::rng::Rng::new(peripherals.RNG);
    let wifi_init = esp_wifi::initialize(
        esp_wifi::EspWifiInitFor::Wifi,
        timer,
        rng.clone(),
        system.radio_clock_control,
        &clocks,
    )
    .expect("Failed to initialize WiFi");
    let (wifi_interface, wifi_controller) = esp_wifi::wifi::new_with_mode(&wifi_init, peripherals.WIFI, WifiStaDevice)
        .expect("Failed to create WiFi interface");

    // Set up ringbuffer for sensor data
    let ringbuffer = AllocRingBuffer::<proto::SensorDataSample>::new(512);
    let ringbuffer = Arc::new(Mutex::<NoopRawMutex, _>::new(ringbuffer));

    // Set up embassy executor
    let executor = make_static!(Executor::new());

    executor.run(|spawner| {
    _ = spawner.spawn(networking_task(wifi_interface, wifi_controller, rng, led, ringbuffer.clone()));
    _ = spawner.spawn(sensor_task(sensor, ringbuffer.clone()));
    });
}

#[embassy_executor::task]
async fn net_stack_task(stack: &'static Stack<WifiDevice<'static, WifiStaDevice>>) {
    stack.run().await
}

#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>, mut led: AnyPin<Output<PushPull>, InputOutputAnalogPinType>) -> () {

    loop {

        if wifi::get_wifi_state() == WifiState::StaConnected {
            controller.wait_for_event(WifiEvent::StaDisconnected).await;
            Timer::after(Duration::from_secs(5)).await;
        }
        
        if !matches!(controller.is_started(), Ok(true)) {
    
            let config = wifi::Configuration::Client(wifi::ClientConfiguration {
                ssid: CONFIG.wifi_ssid.try_into().unwrap(),
                password: CONFIG.wifi_psk.try_into().unwrap(),
                ..Default::default()
            });
        
            _ = controller.set_configuration(&config)
                .inspect_err(|e| log::error!("Failed to set WiFi configuration: {e:?}"));
        
            log::info!("Starting WiFi");
            _ = controller.start().await.inspect_err(|e| log::error!("Failed to start WiFi: {e:?}"));
            
        }

        match controller.connect().await {
            Ok(_) => {
                log::info!("WiFi connected");
                led.set_high();
            },
            Err(e) => {
                match e  {
                    WifiError::Disconnected => {
                        log::info!("WiFi not connected. Retry to connect in 5 s.");
                        led.set_low();
                    },
                    _ => {
                        log::error!("Failed to connect WiFi: {e:?}");
                    }
                }

                Timer::after(Duration::from_secs(5)).await;
            },
        }
    }
}

#[embassy_executor::task]
async fn networking_task(
    interface: WifiDevice<'static, WifiStaDevice>,
    controller: WifiController<'static>,
    rng: Rng,
    led: AnyPin<Output<PushPull>, InputOutputAnalogPinType>,
    ringbuffer: Arc<Mutex<NoopRawMutex, AllocRingBuffer<proto::SensorDataSample>>>,
) {

    // Create network stack
    let config = embassy_net::Config::dhcpv4(Default::default());
    let seed = 42;

    let stack = &*make_static!(Stack::new(
        interface,
        config,
        make_static!(StackResources::<3>::new()),
        seed
    ));

    let spawner = Spawner::for_current_executor().await;
    _ = spawner.spawn(net_stack_task(&stack));
    _ = spawner.spawn(connection_task(controller, led));
    

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
        CONFIG.ws_host,
        CONFIG.ws_port,
        CONFIG.ws_endpoint,
        &stack,
        rng.clone(),
        &mut ws_buffers,
    ).await.expect("Failed to connect");

    log::info!("Websocket open");

    if let Ok(()) = websocket.send_text(CONFIG.device_id).await {
        log::info!("Sent id");
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
async fn sensor_task(
    mut sensor: SensorType,
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
