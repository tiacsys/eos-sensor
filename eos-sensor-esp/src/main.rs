#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]

pub use platform_esp as platform;
use eos_sensor_app as app;

use app::{AppConfig, AppPeripherals, app};

use esp_backtrace as _;

use esp_hal::gpio::{AnyPin, Output, PushPull, InputOutputAnalogPinType};
use esp_hal::prelude::*;
use esp_hal::{
    clock::ClockControl,
    gpio::IO,
    embassy::{
        self,
        executor::Executor,
    },
    peripherals::Peripherals,
    timer::TimerGroup,
    i2c::I2C,
};

use esp_wifi::wifi::{self, WifiController, WifiEvent, WifiStaDevice, WifiState, WifiError};

use embassy_time::{Timer, Duration};
use static_cell::make_static;

use lsm9ds1::LSM9DS1Init;

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

extern crate alloc;
use core::mem::MaybeUninit;

type LedType = AnyPin<Output<PushPull>, InputOutputAnalogPinType>;

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

    let p = AppPeripherals {
        sensor,
        network_device: wifi_interface,
        rng
    };

    let config = AppConfig {
        ws_host: CONFIG.ws_host,
        ws_port: CONFIG.ws_port,
        ws_endpoint: CONFIG.ws_endpoint,
        device_id: CONFIG.device_id,
    };

    // Set up embassy executor
    let executor = make_static!(Executor::new());

    executor.run(|spawner| {
        spawner.spawn(wifi_task(wifi_controller, led))
            .expect("Failed to spawn wifi task");
        spawner.spawn(app(p, config))
            .expect("Failed to spawn application task");
    });
}


#[embassy_executor::task]
async fn wifi_task(mut controller: WifiController<'static>, mut led: LedType) -> () {

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
                .inspect_err(|e| log::error!("Failed to set WiFi configuration: {:?}", e));
        
            log::info!("Starting WiFi");
            _ = controller.start().await.inspect_err(|e| log::error!("Failed to start WiFi: {:?}", e));
            
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
                        log::error!("Failed to connect WiFi: {:?}", e);
                    }
                }

                Timer::after(Duration::from_secs(5)).await;
            },
        }
    }
}
