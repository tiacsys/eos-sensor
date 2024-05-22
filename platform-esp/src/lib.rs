#![no_std]

use esp_hal::{
    Blocking,
    i2c::I2C,
    peripherals::I2C0,
};
use esp_wifi::wifi::{WifiDevice, WifiStaDevice};
use lsm9ds1::{LSM9DS1, interface::I2cInterface};

pub type Sensor = LSM9DS1<I2cInterface<I2C<'static, I2C0, Blocking>>>;
pub type NetworkDevice = WifiDevice<'static, WifiStaDevice>;
pub use esp_hal::rng::Rng;

#[cfg(feature="type-checks")]
mod type_checks {
    #![allow(dead_code)]
    use rand_core::RngCore;
    use embassy_net_driver::Driver;
    
    fn implements_rngcore<T: RngCore>() {}
    fn implements_driver<T: Driver>() {}
    
    fn rng_implements_rngcore() {
        implements_rngcore::<super::Rng>();
    }

    fn network_device_implements_driver() {
        implements_driver::<super::NetworkDevice>();
    }
}
