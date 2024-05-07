#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

mod ws;

use esp_backtrace as _;

use esp_hal::prelude::*;
use esp_hal::rng::Rng;
use esp_hal::{
    clock::ClockControl,
    embassy::{
        self,
        executor::Executor,
    },
    peripherals::Peripherals,
    timer::TimerGroup,
};
use esp_wifi::wifi::{self, WifiController, WifiDevice, WifiEvent, WifiStaDevice, WifiState};

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use embassy_net::{
    Stack,
    StackResources,
};

use static_cell::make_static;

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

#[entry]
fn main() -> ! {
    // Get peripherals
    let peripherals = Peripherals::take();
    let system = peripherals.SYSTEM.split();

    // Initialize clocks
    let clocks = ClockControl::max(system.clock_control).freeze();
    let timg0 = TimerGroup::new_async(peripherals.TIMG0, &clocks);

    // Initialize embassy (but not the executor itself)
    embassy::init(&clocks, timg0);
    
    // Set up logging
    esp_println::logger::init_logger_from_env();

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

    // Set up embassy executor
    let executor = make_static!(Executor::new());

    executor.run(|spawner| {
    _ = spawner.spawn(networking_task(wifi_interface, wifi_controller, rng));
    });
}

#[embassy_executor::task]
async fn net_stack_task(stack: &'static Stack<WifiDevice<'static, WifiStaDevice>>) {
    stack.run().await
}

#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>) -> () {

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
            Ok(_) => log::info!("WiFi connected"),
            Err(e) => {
                log::error!("Failed to connect WiFi: {e:?}");
                Timer::after(Duration::from_secs(5)).await;
            },
        }
    }
}

#[embassy_executor::task]
async fn networking_task(interface: WifiDevice<'static, WifiStaDevice>, controller: WifiController<'static>, rng: Rng) {

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
    _ = spawner.spawn(connection_task(controller));
    

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
}
