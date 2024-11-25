#![no_std]

use esp_hal::{
    peripherals::I2C0,
    rng::Rng, Blocking,
    i2c::master::I2c,
};
use esp_wifi::wifi::{WifiDevice, WifiStaDevice};
use lsm9ds1::{Lsm9ds1, interface::I2cInterface};

pub type NetworkDevice = WifiDevice<'static, WifiStaDevice>;
pub type RngDevice = Rng;
pub type Sensor = Lsm9ds1<I2cInterface<I2c<'static, Blocking>>>;

#[cfg(feature="type-checks")]
mod type_checks {
    #![allow(dead_code)]
    use rand_core::RngCore;
    use embassy_net_driver::Driver;
    
    fn implements_rngcore<T: RngCore>() {}
    fn implements_driver<T: Driver>() {}
    
    fn rng_implements_rngcore() {
        implements_rngcore::<super::RngDevice>();
    }

    fn network_device_implements_driver() {
        implements_driver::<super::NetworkDevice>();
    }
}
