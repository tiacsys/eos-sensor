#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]
#![feature(try_with_capacity)]

use esp_backtrace as _;
use esp_println as _;
extern crate alloc;

mod app;
mod ws;

use app::{app, AppConfig, AppPeripherals};

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::{
    clock::CpuClock,
    gpio,
    i2c::master::{Config as I2cConfig, I2c},
    rng,
};
use esp_wifi::wifi::{self, WifiController, WifiError, WifiEvent, WifiStaDevice, WifiState};
use esp_wifi::EspWifiController;

use fugit::HertzU32;

use lsm9ds1::{
    config::{
        accel_gyro::{AccelGyroSamplingRate, AccelSamplingRate},
        magnetometer::SamplingRate,
    },
    interface::i2c::{AddressAg, AddressM},
    Lsm9ds1Builder,
};

#[macro_export]
macro_rules! make_static {
    ($t:ty, $val:expr) => ($crate::make_static!($t, $val,));
    ($t:ty, $val:expr, $(#[$m:meta])*) => {{
        $(#[$m])*
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        STATIC_CELL.init_with(|| $val)
    }};
}

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

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(72 * 1024);

    esp_println::logger::init_logger_from_env();

    let timer0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG1);
    esp_hal_embassy::init(timer0.timer0);

    log::info!("Embassy initialized!");

    // Initialize Wifi
    let rng = rng::Rng::new(peripherals.RNG);
    let timer1 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let wifi_init = &*make_static!(
        EspWifiController<'static>,
        esp_wifi::init(timer1.timer0, rng, peripherals.RADIO_CLK,).unwrap()
    );

    let (wifi_interface, wifi_controller) =
        esp_wifi::wifi::new_with_mode(wifi_init, peripherals.WIFI, WifiStaDevice)
            .expect("Failed to create WiFi interface");

    log::info!("Wifi initialized!");

    // Initialize I2C power pin
    let _neopixel_i2c_power = gpio::Output::new(peripherals.GPIO2, gpio::Level::High);

    // Set up I2C for sensor
    let i2c_config = I2cConfig::default().with_frequency(HertzU32::kHz(100));
    let sensor_i2c = I2c::new(peripherals.I2C0, i2c_config)
        .expect("I2C Config error")
        .with_scl(peripherals.GPIO20)
        .with_sda(peripherals.GPIO22);

    // Initialize misc peripherals
    let wifi_led = gpio::Output::new(peripherals.GPIO13, gpio::Level::Low);

    // Delay to let peripherals power up
    Timer::after(Duration::from_millis(100)).await;

    // Set up sensor
    let sensor_i2c_config = lsm9ds1::interface::i2c::Config {
        addr_ag: AddressAg::_0x6b,
        addr_m: AddressM::_0x1e,
    };
    let sensor_interface = lsm9ds1::interface::I2cInterface::new(sensor_i2c, sensor_i2c_config);
    let sensor = Lsm9ds1Builder::new()
        .with_accelerometer_enabled(true)
        .with_accel_sampling_rate(AccelSamplingRate::_119Hz)
        .with_magnetometer_enabled(true)
        .with_magnetometer_sampling_rate(SamplingRate::_80Hz)
        .with_gyroscope_enabled(true)
        .with_accel_gyro_sampling_rate(AccelGyroSamplingRate::_119Hz)
        .init_on(sensor_interface)
        .expect("Error during sensor init");

    // Set up application
    let app_peripherals = AppPeripherals {
        network_interface: wifi_interface,
        rng,
        sensor,
    };

    let app_config = AppConfig {
        ws_host: CONFIG.ws_host,
        ws_port: CONFIG.ws_port,
        ws_endpoint: CONFIG.ws_endpoint,
        device_id: CONFIG.device_id,
    };

    // Spawn tasks
    let _ = spawner.spawn(wifi_task(wifi_controller, wifi_led));
    let _ = spawner.spawn(app(app_peripherals, app_config));
}

#[embassy_executor::task]
async fn wifi_task(mut controller: WifiController<'static>, mut led: gpio::Output<'static>) {
    loop {
        if wifi::wifi_state() == WifiState::StaConnected {
            controller.wait_for_event(WifiEvent::StaDisconnected).await;
            Timer::after(Duration::from_secs(5)).await;
        }

        if !matches!(controller.is_started(), Ok(true)) {
            let config = wifi::Configuration::Client(wifi::ClientConfiguration {
                ssid: CONFIG.wifi_ssid.try_into().unwrap(),
                password: CONFIG.wifi_psk.try_into().unwrap(),
                ..Default::default()
            });

            _ = controller
                .set_configuration(&config)
                .inspect_err(|e| log::error!("Failed to set WiFi configuration: {:?}", e));

            log::info!("Starting WiFi");
            _ = controller
                .start_async()
                .await
                .inspect_err(|e| log::error!("Failed to start WiFi: {:?}", e));
        }

        match controller.connect_async().await {
            Ok(_) => {
                log::info!("WiFi connected");
                led.set_high();
            }
            Err(e) => {
                match e {
                    WifiError::Disconnected => {
                        log::info!("WiFi not connected. Retry to connect in 5 s.");
                        led.set_low();
                    }
                    _ => {
                        log::error!("Failed to connect WiFi: {:?}", e);
                    }
                }

                Timer::after(Duration::from_secs(5)).await;
            }
        }
    }
}
