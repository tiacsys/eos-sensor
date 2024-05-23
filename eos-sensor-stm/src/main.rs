#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]

use {defmt_rtt as _, panic_probe as _};

use eos_sensor_app::{AppConfig, AppPeripherals, app};

use platform_stm::hal as hal;
use hal::peripherals;
use hal::time::Hertz;
use hal::eth::{self, generic_smi::GenericSMI, Ethernet, PacketQueue};
use hal::rng::{self, Rng};

use static_cell::make_static;
use lsm9ds1::LSM9DS1Init;
use embassy_executor::Spawner;

extern crate alloc;
use embedded_alloc::Heap;

#[global_allocator]
static HEAP: Heap = Heap::empty();
fn init_heap() -> () {
    use core::mem::MaybeUninit;
    const HEAP_SIZE: usize = 32 * 1024;
    static mut HEAP_MEM: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
    unsafe { HEAP.init(HEAP_MEM.as_ptr() as usize, HEAP_SIZE) }
}

hal::bind_interrupts!(struct Irqs {
    ETH => eth::InterruptHandler;
    RNG => rng::InterruptHandler<peripherals::RNG>;
});

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

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    
    init_heap();

    let p = {
        use hal::rcc::*;
        let mut config = hal::Config::default();
        config.rcc.hse = Some(Hse {
            freq: Hertz(8_000_000),
            mode: HseMode::Bypass,
        });
        config.rcc.pll_src = PllSource::HSE;
        config.rcc.pll = Some(Pll {
            prediv: PllPreDiv::DIV4,
            mul: PllMul::MUL216,
            divp: Some(PllPDiv::DIV2), // 8mhz / 4 * 216 / 2 = 216Mhz
            divq: None,
            divr: None,
        });
        config.rcc.ahb_pre = AHBPrescaler::DIV1;
        config.rcc.apb1_pre = APBPrescaler::DIV4;
        config.rcc.apb2_pre = APBPrescaler::DIV2;
        config.rcc.sys = Sysclk::PLL1_P;
        embassy_stm32::init(config)
    };
    
    let rng = Rng::new(p.RNG, Irqs);

    let mac_addr = [0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF];
    let eth = Ethernet::new(
        make_static!(PacketQueue::<4, 4>::new()),
        p.ETH,
        Irqs,
        p.PA1,
        p.PA2,
        p.PC1,
        p.PA7,
        p.PC4,
        p.PC5,
        p.PG13,
        p.PB13,
        p.PG11,
        GenericSMI::new(0),
        mac_addr
    );

    let i2c1 = hal::i2c::I2c::new_blocking(p.I2C1, p.PB8, p.PB9, Hertz::khz(100), Default::default());
    let ag_addr = lsm9ds1::interface::i2c::AgAddress::_2;
    let mag_addr = lsm9ds1::interface::i2c::MagAddress::_2;
    let sensor_interface = lsm9ds1::interface::I2cInterface::init(i2c1, ag_addr, mag_addr);
    let sensor = LSM9DS1Init::default().with_interface(sensor_interface);

    let config = AppConfig {
        device_id: CONFIG.device_id,
        ws_endpoint: CONFIG.ws_endpoint,
        ws_host: CONFIG.ws_host,
        ws_port: CONFIG.ws_port,
    };

    let p = AppPeripherals {
        rng,
        sensor,
        network_device: eth,
    };

    spawner.spawn(app(p, config))
        .expect("Failed to spawn application task");
}
